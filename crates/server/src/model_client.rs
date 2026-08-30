//! The model client: any LLM through one provider table.
//!
//! Mimics pi's provider mechanism: a provider is a name, an
//! OpenAI-compatible chat endpoint, and the environment variables that
//! hold the user's own API key. `SWIFT_DESIGN_PROVIDER` picks the
//! provider (default `google`), `SWIFT_DESIGN_MODEL` the model, and
//! `SWIFT_DESIGN_PROVIDER_URL` adds a custom endpoint. Swift Design
//! sends requests only with the user's own keys, only when a run
//! starts. The briefing and generation engines share this client.

use std::sync::Arc;

use crate::settings::SettingsStore;

/// Longest time to wait for one model response.
/// How long a connection to the provider may take to open.
pub(crate) const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// How long a reply may go silent before the run gives up. The cap is
/// on silence, not on the whole reply: a model that reasons for minutes
/// sends nothing meanwhile, and a long deck streams for longer than any
/// fixed total. A total cap surfaced as `error decoding response body`.
pub(crate) const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// One LLM provider: a chat endpoint plus its key environment variables.
struct Provider {
    name: &'static str,
    chat_url: &'static str,
    api_key_environment_variables: &'static [&'static str],
    default_model: &'static str,
}

/// Known providers, all speaking the OpenAI chat-completions format.
const PROVIDERS: &[Provider] = &[
    Provider {
        name: "google",
        chat_url: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        api_key_environment_variables: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        default_model: "gemini-2.5-flash",
    },
    Provider {
        name: "openai",
        chat_url: "https://api.openai.com/v1/chat/completions",
        api_key_environment_variables: &["OPENAI_API_KEY"],
        default_model: "gpt-5.4-mini",
    },
    Provider {
        name: "anthropic",
        chat_url: "https://api.anthropic.com/v1/chat/completions",
        api_key_environment_variables: &["ANTHROPIC_API_KEY"],
        default_model: "claude-sonnet-5",
    },
    Provider {
        name: "groq",
        chat_url: "https://api.groq.com/openai/v1/chat/completions",
        api_key_environment_variables: &["GROQ_API_KEY"],
        default_model: "llama-3.3-70b-versatile",
    },
    Provider {
        name: "openrouter",
        chat_url: "https://openrouter.ai/api/v1/chat/completions",
        api_key_environment_variables: &["OPENROUTER_API_KEY"],
        default_model: "google/gemini-2.5-flash",
    },
    Provider {
        name: "ollama",
        chat_url: "http://127.0.0.1:11434/v1/chat/completions",
        api_key_environment_variables: &[],
        default_model: "llama3.1",
    },
];

/// How requests authenticate.
#[derive(Clone)]
pub enum ProviderAuth {
    /// No credential (local endpoints like ollama).
    None,
    /// The user's API key as a bearer token.
    ApiKey(String),
    /// A Claude login access token.
    AnthropicLogin(String),
    /// A ChatGPT login access token plus the account id.
    ChatGptLogin {
        /// Bearer token from the login.
        access_token: String,
        /// ChatGPT account id from the token claims.
        account_id: String,
    },
}

/// The request format of the endpoint.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    /// OpenAI chat completions.
    OpenAiChat,
    /// Anthropic messages API (used by Claude logins).
    AnthropicMessages,
    /// ChatGPT responses API (used by ChatGPT logins).
    ChatGptResponses,
}

/// The resolved model configuration for built-in runs.
#[derive(Clone)]
pub struct ModelConfiguration {
    /// Provider name, for logs and the studio.
    pub provider: String,
    /// Chat endpoint.
    pub chat_url: String,
    /// Credential for requests.
    pub auth: ProviderAuth,
    /// Request format of the endpoint.
    pub wire: WireFormat,
    /// Model identifier sent to the provider.
    pub model: String,
}

/// The context window of `model` in tokens, from its name. Unknown
/// models get 128k, the common floor. Used only for the usage
/// percentage in the studio.
pub fn context_window(model: &str) -> u64 {
    let name = model.to_ascii_lowercase();
    if name.contains("gemini") || name.contains("gpt-4.1") {
        1_048_576
    } else if name.contains("gpt-5") {
        400_000
    } else if name.contains("claude")
        || name.starts_with("o1")
        || name.starts_with("o3")
        || name.starts_with("o4")
    {
        200_000
    } else {
        128_000
    }
}

/// The chat endpoint of a known provider.
pub(crate) fn provider_chat_url(name: &str) -> Option<&'static str> {
    PROVIDERS
        .iter()
        .find(|provider| provider.name == name)
        .map(|provider| provider.chat_url)
}

/// Reads the model configuration from the environment. `None` when the
/// chosen provider needs a key and none of its environment variables
/// are set.
pub fn configured_model() -> Option<ModelConfiguration> {
    let provider_name =
        std::env::var("SWIFT_DESIGN_PROVIDER").unwrap_or_else(|_| "google".to_owned());
    if let Ok(chat_url) = std::env::var("SWIFT_DESIGN_PROVIDER_URL") {
        return Some(ModelConfiguration {
            provider: provider_name.clone(),
            chat_url,
            auth: match std::env::var("SWIFT_DESIGN_PROVIDER_API_KEY") {
                Ok(api_key) => ProviderAuth::ApiKey(api_key),
                Err(_) => ProviderAuth::None,
            },
            wire: WireFormat::OpenAiChat,
            model: std::env::var("SWIFT_DESIGN_MODEL").unwrap_or(provider_name),
        });
    }
    let provider = PROVIDERS
        .iter()
        .find(|provider| provider.name == provider_name)?;
    let api_key = provider
        .api_key_environment_variables
        .iter()
        .find_map(|variable| std::env::var(variable).ok());
    if api_key.is_none() && !provider.api_key_environment_variables.is_empty() {
        return None;
    }
    Some(ModelConfiguration {
        provider: provider.name.to_owned(),
        chat_url: provider.chat_url.to_owned(),
        auth: match api_key {
            Some(api_key) => ProviderAuth::ApiKey(api_key),
            None => ProviderAuth::None,
        },
        wire: WireFormat::OpenAiChat,
        model: std::env::var("SWIFT_DESIGN_MODEL")
            .unwrap_or_else(|_| provider.default_model.to_owned()),
    })
}

/// Builds the configuration from the settings the user picked in the
/// studio. `None` when the stored choice has no usable credential.
pub fn configuration_from_settings(
    settings: &crate::settings::StoredSettings,
) -> Option<ModelConfiguration> {
    if let Some(oauth) = &settings.oauth
        && settings.provider == "anthropic"
    {
        return Some(ModelConfiguration {
            provider: "anthropic".to_owned(),
            chat_url: "https://api.anthropic.com/v1/messages".to_owned(),
            auth: ProviderAuth::AnthropicLogin(oauth.access_token.clone()),
            wire: WireFormat::AnthropicMessages,
            model: settings.model.clone(),
        });
    }
    if let Some(oauth) = &settings.oauth
        && settings.provider == "openai"
    {
        // The retired `gpt-5` slug 400s on the ChatGPT backend; remap
        // it to the current default and pass everything else through.
        let model = if settings.model == "gpt-5" {
            crate::settings::CHATGPT_LOGIN_MODELS[0].to_owned()
        } else {
            settings.model.clone()
        };
        return Some(ModelConfiguration {
            provider: "openai".to_owned(),
            chat_url: "https://chatgpt.com/backend-api/codex/responses".to_owned(),
            auth: ProviderAuth::ChatGptLogin {
                access_token: oauth.access_token.clone(),
                account_id: oauth.account_id.clone().unwrap_or_default(),
            },
            wire: WireFormat::ChatGptResponses,
            model,
        });
    }
    let provider = PROVIDERS
        .iter()
        .find(|provider| provider.name == settings.provider)?;
    let auth = match &settings.api_key {
        Some(api_key) => ProviderAuth::ApiKey(api_key.clone()),
        None if provider.api_key_environment_variables.is_empty() => ProviderAuth::None,
        None => return None,
    };
    Some(ModelConfiguration {
        provider: provider.name.to_owned(),
        chat_url: provider.chat_url.to_owned(),
        auth,
        wire: WireFormat::OpenAiChat,
        model: settings.model.clone(),
    })
}

/// Token counts of one model request, as the provider reported them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Tokens the model read: the current context size.
    pub input_tokens: u64,
    /// Tokens the model wrote.
    pub output_tokens: u64,
}

/// A sink for per-request token usage, shared with the run status.
pub type UsageSink = Arc<dyn Fn(TokenUsage) + Send + Sync>;

/// A line sink for run progress, shared with the agent-run log.
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

/// A sink for the accumulated text of a streaming reply.
pub type TextSink = dyn Fn(&str) + Send + Sync;

/// Providers on the OpenAI chat wire that read `file` parts.
pub(crate) fn accepts_file_parts(provider: &str) -> bool {
    matches!(provider, "openai" | "openrouter")
}

/// One configured model behind one HTTP client: sends chat requests in
/// the endpoint's wire format, streams replies, and reports token
/// usage. Holds no store, so it can write nothing on its own.
#[derive(Clone)]
pub struct ModelClient {
    configuration: ModelConfiguration,
    settings: Option<SettingsStore>,
    usage_sink: Option<UsageSink>,
}

impl ModelClient {
    /// Creates a client. `settings` enables login-token refresh for
    /// Claude and ChatGPT logins.
    pub fn new(configuration: ModelConfiguration, settings: Option<SettingsStore>) -> Self {
        Self {
            configuration,
            settings,
            usage_sink: None,
        }
    }

    /// Reports each request's token usage to `sink`.
    pub fn with_usage_sink(mut self, sink: UsageSink) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    /// The model identifier sent to the provider.
    pub fn model(&self) -> &str {
        &self.configuration.model
    }

    /// Short label for the studio: `google/gemini-2.5-flash`.
    pub fn label(&self) -> String {
        format!(
            "{}/{}",
            self.configuration.provider, self.configuration.model
        )
    }

    /// The context window of the configured model, in tokens.
    pub fn context_window(&self) -> u64 {
        context_window(&self.configuration.model)
    }

    /// An HTTP client with the connect timeout every run shares. The
    /// reply itself is capped by silence, in `chat_with`.
    pub fn build_http_client() -> Result<reqwest::Client, String> {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())
    }

    /// Renews an expiring login and persists the new tokens.
    pub async fn refresh_login_if_needed(&mut self, log: &LogSink) -> Result<(), String> {
        let Some(settings_store) = &self.settings else {
            return Ok(());
        };
        let is_login = matches!(
            self.configuration.auth,
            ProviderAuth::AnthropicLogin(_) | ProviderAuth::ChatGptLogin { .. }
        );
        if !is_login {
            return Ok(());
        }
        let Ok(Some(mut stored)) = settings_store.read().await else {
            return Ok(());
        };
        let Some(oauth) = stored.oauth.clone() else {
            return Ok(());
        };
        if !crate::settings::is_login_expiring(&oauth) {
            return Ok(());
        }
        log("refreshing the login");
        let refreshed = match &self.configuration.auth {
            ProviderAuth::AnthropicLogin(_) => {
                let refreshed = crate::settings::refresh_login(&oauth.refresh_token).await?;
                self.configuration.auth =
                    ProviderAuth::AnthropicLogin(refreshed.access_token.clone());
                refreshed
            }
            _ => {
                let refreshed =
                    crate::settings::refresh_chatgpt_login(&oauth.refresh_token).await?;
                self.configuration.auth = ProviderAuth::ChatGptLogin {
                    access_token: refreshed.access_token.clone(),
                    account_id: refreshed
                        .account_id
                        .clone()
                        .or(oauth.account_id)
                        .unwrap_or_default(),
                };
                refreshed
            }
        };
        stored.oauth = Some(refreshed);
        settings_store
            .write(&stored)
            .await
            .map_err(|error| error.to_string())
    }

    /// One model request in the endpoint's wire format. Returns the
    /// assistant text and reports token usage to the usage sink.
    pub async fn chat(
        &self,
        client: &reqwest::Client,
        messages: &[serde_json::Value],
        effort: &str,
    ) -> Result<String, String> {
        self.chat_with(client, self.request_body(messages, effort), None)
            .await
    }

    /// The request body for `messages` in the endpoint's wire format.
    /// Every format streams, so partial replies can be shown.
    pub fn request_body(&self, messages: &[serde_json::Value], effort: &str) -> serde_json::Value {
        match self.configuration.wire {
            WireFormat::OpenAiChat => {
                // Only OpenAI and OpenRouter read `file` parts on this
                // wire; other providers get a note instead.
                let messages = if accepts_file_parts(&self.configuration.provider) {
                    messages.to_vec()
                } else {
                    without_file_parts(messages)
                };
                let mut body = serde_json::json!({
                    "model": self.configuration.model,
                    "messages": messages,
                    "stream": true,
                });
                // Reasoning effort and usage-in-stream exist on OpenAI
                // itself; other compatible providers reject unknown
                // fields.
                if self.configuration.provider == "openai" {
                    body["stream_options"] = serde_json::json!({ "include_usage": true });
                    if self.configuration.model.starts_with("gpt-5") {
                        body["reasoning_effort"] = serde_json::json!(effort);
                    }
                }
                body
            }
            WireFormat::AnthropicMessages => {
                anthropic_messages_body(&self.configuration.model, messages)
            }
            WireFormat::ChatGptResponses => {
                chatgpt_responses_body(&self.configuration.model, messages, effort)
            }
        }
    }

    /// Sends `body` and streams the reply. The accumulated assistant
    /// text goes to `on_text` after every delta, so callers can show
    /// partial results. Returns the full text and reports token usage.
    /// A reply the connection dropped is sent once more.
    pub async fn chat_with(
        &self,
        client: &reqwest::Client,
        body: serde_json::Value,
        on_text: Option<&TextSink>,
    ) -> Result<String, String> {
        with_one_retry(|| self.chat_once(client, &body, on_text)).await
    }

    /// One send of `body` with the reply streamed. A provider that
    /// answers with one JSON document instead of an event stream is
    /// read the plain way.
    async fn chat_once(
        &self,
        client: &reqwest::Client,
        body: &serde_json::Value,
        on_text: Option<&TextSink>,
    ) -> Result<String, ChatFailure> {
        let mut request = client.post(&self.configuration.chat_url).json(body);
        match &self.configuration.auth {
            ProviderAuth::None => {}
            ProviderAuth::ApiKey(api_key) => request = request.bearer_auth(api_key),
            ProviderAuth::AnthropicLogin(access_token) => {
                request = request
                    .bearer_auth(access_token)
                    .header("anthropic-version", "2023-06-01")
                    .header("anthropic-beta", "oauth-2025-04-20");
            }
            ProviderAuth::ChatGptLogin {
                access_token,
                account_id,
            } => {
                request = request
                    .bearer_auth(access_token)
                    .header("chatgpt-account-id", account_id)
                    .header("OpenAI-Beta", "responses=experimental")
                    .header("originator", "codex_cli_rs")
                    .header("accept", "text/event-stream");
            }
        }
        let provider = &self.configuration.provider;
        let mut response = request.send().await.map_err(|error| {
            ChatFailure::Dropped(format!("request to {provider} failed: {error}"))
        })?;
        let status = response.status();
        if !status.is_success() {
            let mut detail = response.text().await.unwrap_or_default();
            detail.truncate(300);
            return Err(ChatFailure::Final(format!(
                "{provider} returned {status}: {detail}"
            )));
        }
        let mut state = StreamState::default();
        let mut raw = String::new();
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let chunk = match tokio::time::timeout(IDLE_TIMEOUT, response.chunk()).await {
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    return Err(ChatFailure::Dropped(format!(
                        "reading the {provider} reply failed: {error}"
                    )));
                }
                Err(_) => {
                    return Err(ChatFailure::Final(format!(
                        "the {provider} reply went silent for {} s",
                        IDLE_TIMEOUT.as_secs()
                    )));
                }
            };
            pending.extend_from_slice(&chunk);
            while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=position).collect();
                let line = String::from_utf8_lossy(&line);
                raw.push_str(&line);
                if stream_line(self.configuration.wire, line.trim_end(), &mut state)
                    && let Some(on_text) = on_text
                {
                    on_text(&state.collected);
                }
            }
        }
        let tail = String::from_utf8_lossy(&pending).into_owned();
        raw.push_str(&tail);
        stream_line(self.configuration.wire, tail.trim_end(), &mut state);
        if !state.saw_event {
            // One JSON document, not an event stream.
            let value: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|error| ChatFailure::Final(error.to_string()))?;
            self.report_usage(parse_usage(&value["usage"]));
            let content = match self.configuration.wire {
                WireFormat::OpenAiChat => value["choices"][0]["message"]["content"].as_str(),
                WireFormat::AnthropicMessages => value["content"][0]["text"].as_str(),
                WireFormat::ChatGptResponses => None,
            };
            return content
                .map(str::to_owned)
                .ok_or_else(|| ChatFailure::Final("response has no message content".to_owned()));
        }
        self.report_usage(state.usage);
        if state.collected.is_empty() {
            return Err(ChatFailure::Final(
                state
                    .failure
                    .unwrap_or_else(|| "the stream carried no output text".to_owned()),
            ));
        }
        Ok(state.collected)
    }

    fn report_usage(&self, usage: Option<TokenUsage>) {
        if let (Some(sink), Some(usage)) = (&self.usage_sink, usage) {
            sink(usage);
        }
    }
}

/// Why one send of a request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ChatFailure {
    /// The connection failed or the reply was cut off before its end.
    /// A provider or a proxy closes a stream that runs for minutes,
    /// and reqwest reports the cut as `error decoding response body`.
    /// The same request may succeed when sent again.
    Dropped(String),
    /// The provider answered, and the answer is the failure: an error
    /// status, a silent reply, or a body with no text.
    Final(String),
}

/// Runs `attempt`, and once more when the first reply was dropped.
/// A second drop reports both, so the log shows the retry.
async fn with_one_retry<F, Future>(mut attempt: F) -> Result<String, String>
where
    F: FnMut() -> Future,
    Future: std::future::Future<Output = Result<String, ChatFailure>>,
{
    let first = match attempt().await {
        Ok(text) => return Ok(text),
        Err(ChatFailure::Final(message)) => return Err(message),
        Err(ChatFailure::Dropped(message)) => message,
    };
    match attempt().await {
        Ok(text) => Ok(text),
        Err(ChatFailure::Final(message)) => Err(message),
        Err(ChatFailure::Dropped(message)) => {
            Err(format!("{message} (sent twice; the first try: {first})"))
        }
    }
}

/// Reads a `usage` object in OpenAI or Anthropic shape. Cached input
/// tokens count toward the context size. `None` when absent.
fn parse_usage(usage: &serde_json::Value) -> Option<TokenUsage> {
    let input = usage["prompt_tokens"]
        .as_u64()
        .or_else(|| usage["input_tokens"].as_u64())?;
    let cached = usage["cache_read_input_tokens"].as_u64().unwrap_or(0)
        + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let output = usage["completion_tokens"]
        .as_u64()
        .or_else(|| usage["output_tokens"].as_u64())
        .unwrap_or(0);
    Some(TokenUsage {
        input_tokens: input + cached,
        output_tokens: output,
    })
}

/// Replaces every `file` part in `messages` with a text note, for
/// providers that reject file parts.
pub(crate) fn without_file_parts(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|message| {
            let Some(parts) = message["content"].as_array() else {
                return message.clone();
            };
            let replaced: Vec<serde_json::Value> = parts
                .iter()
                .map(|part| match part["type"].as_str() {
                    Some("file") => serde_json::json!({
                        "type": "text",
                        "text": format!(
                            "(the file {} cannot be sent to this provider)",
                            part["file"]["filename"].as_str().unwrap_or("?")
                        ),
                    }),
                    _ => part.clone(),
                })
                .collect();
            let mut message = message.clone();
            message["content"] = serde_json::Value::Array(replaced);
            message
        })
        .collect()
}

/// Splits a `data:{media};base64,{data}` URL into its media type and
/// data. `None` for any other URL.
fn split_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (media_type, data) = rest.split_once(";base64,")?;
    Some((media_type, data))
}

/// Converts OpenAI-style content (a string or parts) to Anthropic
/// content blocks.
fn anthropic_content(content: &serde_json::Value) -> serde_json::Value {
    let Some(parts) = content.as_array() else {
        return content.clone();
    };
    let blocks: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part["type"].as_str() {
            Some("text") => Some(serde_json::json!({ "type": "text", "text": part["text"] })),
            Some("image_url") => {
                let (media_type, data) = split_data_url(part["image_url"]["url"].as_str()?)?;
                Some(serde_json::json!({
                    "type": "image",
                    "source": { "type": "base64", "media_type": media_type, "data": data },
                }))
            }
            Some("file") => {
                let (media_type, data) = split_data_url(part["file"]["file_data"].as_str()?)?;
                Some(serde_json::json!({
                    "type": "document",
                    "source": { "type": "base64", "media_type": media_type, "data": data },
                    "title": part["file"]["filename"],
                }))
            }
            _ => None,
        })
        .collect();
    serde_json::Value::Array(blocks)
}

/// Converts OpenAI-style content (a string or parts) to ChatGPT
/// responses-API input parts. `text_type` is `input_text` or
/// `output_text`.
fn chatgpt_content(content: &serde_json::Value, text_type: &str) -> serde_json::Value {
    let Some(parts) = content.as_array() else {
        let text = content.as_str().unwrap_or_default();
        return serde_json::json!([{ "type": text_type, "text": text }]);
    };
    let items: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part["type"].as_str() {
            Some("text") => Some(serde_json::json!({ "type": text_type, "text": part["text"] })),
            Some("image_url") => Some(serde_json::json!({
                "type": "input_image",
                "image_url": part["image_url"]["url"],
                "detail": "auto",
            })),
            Some("file") => Some(serde_json::json!({
                "type": "input_file",
                "filename": part["file"]["filename"],
                "file_data": part["file"]["file_data"],
            })),
            _ => None,
        })
        .collect();
    serde_json::Value::Array(items)
}

/// Builds an Anthropic messages-API body from OpenAI-style messages.
/// Claude login tokens require the Claude Code identity as the first
/// system block.
pub(crate) fn anthropic_messages_body(
    model: &str,
    messages: &[serde_json::Value],
) -> serde_json::Value {
    let mut system_blocks = vec![serde_json::json!({
        "type": "text",
        "text": "You are Claude Code, Anthropic's official CLI for Claude.",
    })];
    let mut chat_messages = Vec::new();
    for message in messages {
        if message["role"] == "system" {
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": message["content"],
            }));
        } else {
            chat_messages.push(serde_json::json!({
                "role": message["role"],
                "content": anthropic_content(&message["content"]),
            }));
        }
    }
    serde_json::json!({
        "model": model,
        "max_tokens": 16000,
        "stream": true,
        "system": system_blocks,
        "messages": chat_messages,
    })
}

/// Builds a ChatGPT responses-API body from OpenAI-style messages.
pub(crate) fn chatgpt_responses_body(
    model: &str,
    messages: &[serde_json::Value],
    effort: &str,
) -> serde_json::Value {
    let mut instructions = String::new();
    let mut input_items = Vec::new();
    for message in messages {
        if message["role"] == "system" {
            instructions.push_str(message["content"].as_str().unwrap_or_default());
            instructions.push('\n');
        } else {
            let content_type = if message["role"] == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            input_items.push(serde_json::json!({
                "type": "message",
                "role": message["role"],
                "content": chatgpt_content(&message["content"], content_type),
            }));
        }
    }
    serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": input_items,
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
        "reasoning": { "effort": effort },
    })
}

/// What one event stream has delivered so far.
#[derive(Debug, Default)]
struct StreamState {
    /// The assistant text, in order.
    collected: String,
    /// Token usage, when the stream reported it.
    usage: Option<TokenUsage>,
    /// The failure the stream reported, when it did.
    failure: Option<String>,
    /// True once one `data:` event parsed: the reply is a stream.
    saw_event: bool,
}

/// Folds one line of an event stream into `state`. Returns true when
/// the line added assistant text.
fn stream_line(wire: WireFormat, line: &str, state: &mut StreamState) -> bool {
    let Some(data) = line.strip_prefix("data:") else {
        return false;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return false;
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    state.saw_event = true;
    let (text, usage, failure) = match wire {
        WireFormat::OpenAiChat => (
            event["choices"][0]["delta"]["content"].as_str(),
            parse_usage(&event["usage"]),
            event["error"]["message"].as_str(),
        ),
        WireFormat::AnthropicMessages => match event["type"].as_str() {
            Some("content_block_delta") => (event["delta"]["text"].as_str(), None, None),
            Some("message_start") => (None, parse_usage(&event["message"]["usage"]), None),
            // The closing delta carries output tokens only.
            Some("message_delta") => (
                None,
                event["usage"]["output_tokens"]
                    .as_u64()
                    .map(|output_tokens| TokenUsage {
                        input_tokens: event["usage"]["input_tokens"].as_u64().unwrap_or(0),
                        output_tokens,
                    }),
                None,
            ),
            Some("error") => (None, None, event["error"]["message"].as_str()),
            _ => (None, None, None),
        },
        WireFormat::ChatGptResponses => match event["type"].as_str() {
            Some("response.output_text.delta") => (event["delta"].as_str(), None, None),
            Some("response.completed") => (None, parse_usage(&event["response"]["usage"]), None),
            Some("response.failed") | Some("error") => (
                None,
                None,
                Some(
                    event["response"]["error"]["message"]
                        .as_str()
                        .or_else(|| event["message"].as_str())
                        .unwrap_or("the stream reported a failure"),
                ),
            ),
            _ => (None, None, None),
        },
    };
    if let Some(usage) = usage {
        // Anthropic reports input tokens at the start and output
        // tokens at the end; keep the larger of each.
        let previous = state.usage.unwrap_or_default();
        state.usage = Some(TokenUsage {
            input_tokens: previous.input_tokens.max(usage.input_tokens),
            output_tokens: previous.output_tokens.max(usage.output_tokens),
        });
    }
    if let Some(failure) = failure {
        state.failure = Some(failure.to_owned());
    }
    match text {
        Some(text) if !text.is_empty() => {
            state.collected.push_str(text);
            true
        }
        _ => false,
    }
}

/// Collects the assistant text and token usage from a whole ChatGPT
/// responses event stream.
#[cfg(test)]
fn parse_chatgpt_stream(stream_text: &str) -> Result<(String, Option<TokenUsage>), String> {
    let mut state = StreamState::default();
    for line in stream_text.lines() {
        stream_line(WireFormat::ChatGptResponses, line, &mut state);
    }
    if state.collected.is_empty() {
        return Err(state
            .failure
            .unwrap_or_else(|| "the stream carried no output text".to_owned()));
    }
    Ok((state.collected, state.usage))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::cell::Cell;

    use super::{
        ChatFailure, StreamState, TokenUsage, WireFormat, chatgpt_responses_body, context_window,
        parse_chatgpt_stream, parse_usage, stream_line, with_one_retry,
    };

    #[tokio::test]
    async fn a_dropped_reply_is_sent_once_more() {
        let calls = Cell::new(0);
        let result = with_one_retry(|| {
            calls.set(calls.get() + 1);
            let call = calls.get();
            async move {
                if call == 1 {
                    Err(ChatFailure::Dropped("reading the reply failed".to_owned()))
                } else {
                    Ok("done".to_owned())
                }
            }
        })
        .await;
        assert_eq!(result, Ok("done".to_owned()));
        assert_eq!(calls.get(), 2);
    }

    #[tokio::test]
    async fn a_second_drop_reports_both_and_a_final_failure_is_not_retried() {
        let calls = Cell::new(0);
        let result = with_one_retry(|| {
            calls.set(calls.get() + 1);
            async { Err(ChatFailure::Dropped("cut off".to_owned())) }
        })
        .await;
        assert_eq!(
            result,
            Err("cut off (sent twice; the first try: cut off)".to_owned())
        );
        assert_eq!(calls.get(), 2);
        let calls = Cell::new(0);
        let result = with_one_retry(|| {
            calls.set(calls.get() + 1);
            async { Err(ChatFailure::Final("fake returned 400".to_owned())) }
        })
        .await;
        assert_eq!(result, Err("fake returned 400".to_owned()));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn usage_parses_openai_and_anthropic_shapes() {
        let openai = serde_json::json!({"prompt_tokens": 50, "completion_tokens": 5});
        assert_eq!(
            parse_usage(&openai),
            Some(TokenUsage {
                input_tokens: 50,
                output_tokens: 5
            })
        );
        let anthropic = serde_json::json!({
            "input_tokens": 10, "cache_read_input_tokens": 30, "output_tokens": 4
        });
        assert_eq!(
            parse_usage(&anthropic),
            Some(TokenUsage {
                input_tokens: 40,
                output_tokens: 4
            })
        );
        assert_eq!(parse_usage(&serde_json::json!(null)), None);
    }

    #[test]
    fn chatgpt_bodies_split_instructions_from_input() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "draft"}),
        ];
        let body = chatgpt_responses_body("gpt-5.5", &messages, "high");
        assert_eq!(body["instructions"], "sys\n");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
        assert_eq!(body["stream"], true);
        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn chatgpt_streams_collect_deltas_and_report_failures() {
        let stream = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"a\\\"\"}\n\
                      data: {\"type\":\"response.output_text.delta\",\"delta\":\":1}\"}\n\
                      data: {\"type\":\"response.completed\",\
                      \"response\":{\"usage\":{\"input_tokens\":120,\"output_tokens\":7}}}\n";
        let (content, usage) = parse_chatgpt_stream(stream).unwrap();
        assert_eq!(content, "{\"a\":1}");
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 7
            })
        );
        let failed = "data: {\"type\":\"response.failed\",\
                      \"response\":{\"error\":{\"message\":\"quota\"}}}\n";
        assert!(parse_chatgpt_stream(failed).unwrap_err().contains("quota"));
    }

    #[test]
    fn stream_lines_collect_text_and_usage_per_wire_format() {
        let mut state = StreamState::default();
        assert!(stream_line(
            WireFormat::OpenAiChat,
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}",
            &mut state
        ));
        assert!(!stream_line(
            WireFormat::OpenAiChat,
            "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}",
            &mut state
        ));
        assert!(!stream_line(
            WireFormat::OpenAiChat,
            "data: [DONE]",
            &mut state
        ));
        assert_eq!(state.collected, "Hel");
        assert_eq!(state.usage.unwrap().input_tokens, 9);
        assert!(state.saw_event);
        let mut anthropic = StreamState::default();
        stream_line(
            WireFormat::AnthropicMessages,
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":30,\"output_tokens\":1}}}",
            &mut anthropic,
        );
        assert!(stream_line(
            WireFormat::AnthropicMessages,
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"{\\\"a\\\"\"}}",
            &mut anthropic,
        ));
        stream_line(
            WireFormat::AnthropicMessages,
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":12}}",
            &mut anthropic,
        );
        assert_eq!(anthropic.collected, "{\"a\"");
        assert_eq!(
            anthropic.usage,
            Some(TokenUsage {
                input_tokens: 30,
                output_tokens: 12
            })
        );
        let mut plain = StreamState::default();
        assert!(!stream_line(
            WireFormat::OpenAiChat,
            "{\"choices\":[]}",
            &mut plain
        ));
        assert!(!plain.saw_event);
    }

    #[test]
    fn context_windows_follow_the_model_family() {
        assert_eq!(context_window("gemini-2.5-pro"), 1_048_576);
        assert_eq!(context_window("gpt-5-mini"), 400_000);
        assert_eq!(context_window("claude-sonnet-5"), 200_000);
        assert_eq!(context_window("llama3.1"), 128_000);
    }
}
