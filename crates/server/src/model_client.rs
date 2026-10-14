//! The model client: any LLM through one provider table.
//!
//! Mimics pi's provider mechanism: a provider is a name, an
//! OpenAI-compatible chat endpoint, and the environment variables that
//! hold the user's own API key. `SWIFT_DESIGN_PROVIDER` picks the

//! `SWIFT_DESIGN_PROVIDER_URL` adds a custom endpoint. Swift Design
//! sends requests only with the user's own keys, only when a run
//! starts. The briefing and generation engines share this client.

#[cfg(test)]
mod tests;
mod wire;
use wire::{
    StreamState, anthropic_messages_body, chatgpt_responses_body, http_failure, parse_usage,
    stream_line, without_file_parts,
};

use std::sync::Arc;

use crate::model_retry::{
    RequestFailure, is_account_failure, retry_after, stream_failure, with_retries,
};
use crate::settings::{
    CHATGPT_DEFAULT_MODEL, SettingsStore, StoredSettings, is_login_expiring, refresh_chatgpt_login,
    refresh_login,
};

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
pub fn configuration_from_settings(settings: &StoredSettings) -> Option<ModelConfiguration> {
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
            CHATGPT_DEFAULT_MODEL.to_owned()
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
    retry_log: Option<LogSink>,
}

impl ModelClient {
    /// Creates a client. `settings` enables login-token refresh for
    /// Claude and ChatGPT logins.
    pub fn new(configuration: ModelConfiguration, settings: Option<SettingsStore>) -> Self {
        Self {
            configuration,
            settings,
            usage_sink: None,
            retry_log: None,
        }
    }

    /// Reports each request's token usage to `sink`.
    pub fn with_usage_sink(mut self, sink: UsageSink) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    /// Reports transient request retries to the active run.
    pub fn with_retry_log(mut self, log: LogSink) -> Self {
        self.retry_log = Some(log);
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
        if !is_login_expiring(&oauth) {
            return Ok(());
        }
        log("refreshing the login");
        let refreshed = match &self.configuration.auth {
            ProviderAuth::AnthropicLogin(_) => {
                let refreshed = refresh_login(&oauth.refresh_token).await?;
                self.configuration.auth =
                    ProviderAuth::AnthropicLogin(refreshed.access_token.clone());
                refreshed
            }
            _ => {
                let refreshed = refresh_chatgpt_login(&oauth.refresh_token).await?;
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
    /// Retries transient failures at most three times with bounded, cancellable backoff.
    pub async fn chat_with(
        &self,
        client: &reqwest::Client,
        body: serde_json::Value,
        on_text: Option<&TextSink>,
    ) -> Result<String, String> {
        with_retries(
            || self.chat_once(client, &body, on_text),
            self.retry_log.as_ref(),
        )
        .await
    }

    /// One send of `body` with the reply streamed. A provider that
    /// answers with one JSON document instead of an event stream is
    /// read the plain way.
    async fn chat_once(
        &self,
        client: &reqwest::Client,
        body: &serde_json::Value,
        on_text: Option<&TextSink>,
    ) -> Result<String, RequestFailure> {
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
        let mut response = request
            .send()
            .await
            .map_err(|error| RequestFailure::Temporary {
                message: format!("request to {provider} failed: {error}"),
                retry_after: None,
            })?;
        let status = response.status();
        if !status.is_success() {
            let suggested_wait = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(retry_after);
            let detail = response.text().await.unwrap_or_default();
            let is_account_error = is_account_failure(&detail);
            let detail: String = detail.chars().take(300).collect();
            if is_account_error {
                return Err(RequestFailure::Permanent(format!(
                    "{provider} returned {status}: {detail}"
                )));
            }
            return Err(http_failure(
                status.as_u16(),
                format!("{provider} returned {status}: {detail}"),
                suggested_wait,
            ));
        }
        let mut state = StreamState::default();
        let mut raw = String::new();
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let chunk = match tokio::time::timeout(IDLE_TIMEOUT, response.chunk()).await {
                Ok(Ok(Some(chunk))) => chunk,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    return Err(RequestFailure::Temporary {
                        message: format!("reading the {provider} reply failed: {error}"),
                        retry_after: None,
                    });
                }
                Err(_) => {
                    return Err(RequestFailure::Temporary {
                        message: format!(
                            "the {provider} reply went silent for {} s",
                            IDLE_TIMEOUT.as_secs()
                        ),
                        retry_after: None,
                    });
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
                .map_err(|error| RequestFailure::Permanent(error.to_string()))?;
            self.report_usage(parse_usage(&value["usage"]));
            if let Some(message) = value["error"]["message"].as_str() {
                return Err(stream_failure(
                    message.to_owned(),
                    value["error"]["code"]
                        .as_str()
                        .or_else(|| value["error"]["type"].as_str()),
                ));
            }
            let content = match self.configuration.wire {
                WireFormat::OpenAiChat => value["choices"][0]["message"]["content"].as_str(),
                WireFormat::AnthropicMessages => value["content"][0]["text"].as_str(),
                WireFormat::ChatGptResponses => None,
            };
            return content.map(str::to_owned).ok_or_else(|| {
                RequestFailure::Permanent("response has no message content".to_owned())
            });
        }
        self.report_usage(state.usage);
        // A provider error invalidates the entire attempt, even after partial text.
        if let Some(failure) = state.failure {
            return Err(stream_failure(failure, state.failure_code.as_deref()));
        }
        if state.collected.is_empty() {
            return Err(RequestFailure::Permanent(
                "the stream carried no output text".to_owned(),
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
