//! The built-in generation engine: any LLM through one provider table.
//!
//! Mimics pi's provider mechanism: a provider is a name, an
//! OpenAI-compatible chat endpoint, and the environment variables that
//! hold the user's own API key. `SWIFT_DESIGN_PROVIDER` picks the
//! provider (default `google`), `SWIFT_DESIGN_MODEL` the model, and
//! `SWIFT_DESIGN_PROVIDER_URL` adds a custom endpoint. Swift Design sends
//! requests only with the user's own keys, only when the user starts a
//! built-in run.
//!
//! The loop: read the brief, ask the model for each candidate design,
//! validate, feed every validation error back for a fix round, and
//! save the result. The studio watches it all through `/events`.

use std::sync::Arc;

use design_model::Design;

use crate::briefs::{Brief, BriefStore, ChatMessage};
use crate::concepts::{Concept, concept_input, concept_note, concept_prompt, parse_concepts};
use crate::designs::DesignStore;
use crate::events::ChangeNotifier;
use crate::instructions::CONTENT_RULES;

/// Fix rounds per candidate before giving up, by effort level.
fn fix_round_limit(effort: &str) -> usize {
    match effort {
        "low" => 2,
        "high" => 4,
        _ => 3,
    }
}

/// Longest time to wait for one model response.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

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

/// The built-in engine: model configuration plus the stores it writes.
#[derive(Clone)]
pub struct GenerationEngine {
    configuration: ModelConfiguration,
    designs: DesignStore,
    briefs: BriefStore,
    questions: crate::questions::QuestionStore,
    settings: Option<crate::settings::SettingsStore>,
    notifier: ChangeNotifier,
    usage_sink: Option<UsageSink>,
    progress_sink: Option<ProgressSink>,
    design_progress_sink: Option<DesignProgressSink>,
    templates: Option<crate::templates::TemplateStore>,
    uploads: Option<crate::uploads::UploadStore>,
}

/// A line sink for run progress, shared with the agent-run log.
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

/// A sink for how far the current turn is, 0 to 100, shared with the
/// run status. A turn starts at 0.
pub type ProgressSink = Arc<dyn Fn(u8) + Send + Sync>;

/// A sink for how far one design the turn writes is, 0 to 100, by design
/// id: a candidate being written, a design being continued or edited.
pub type DesignProgressSink = Arc<dyn Fn(&str, u8) + Send + Sync>;

/// One item's share of a turn's progress, 0.0 to 1.0. Several items
/// (candidates, continued designs) each report their own share; the mean
/// goes to the `ProgressSink`.
type ShareSink = Arc<dyn Fn(f32) + Send + Sync>;

/// The screens each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type ChunkBoard = Arc<std::sync::Mutex<Vec<Vec<design_model::Screen>>>>;

/// The share of a design request that the first valid draft completes;
/// the polish rounds fill the rest.
const DRAFT_SHARE: f32 = 0.6;

/// Screens the engine asks for in one continuation request. Small
/// chunks keep each reply short, and the design grows on the canvas
/// after every chunk.
const CONTINUE_CHUNK_SCREENS: usize = 3;

/// The share of a continuation that writing the screens completes; the
/// polish rounds fill the rest.
const CONTINUE_DRAFT_SHARE: f32 = 0.85;

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

impl GenerationEngine {
    /// Creates an engine over the given stores. `settings` enables
    /// login-token refresh for Claude logins.
    pub fn new(
        configuration: ModelConfiguration,
        designs: DesignStore,
        briefs: BriefStore,
        questions: crate::questions::QuestionStore,
        settings: Option<crate::settings::SettingsStore>,
        notifier: ChangeNotifier,
    ) -> Self {
        Self {
            configuration,
            designs,
            briefs,
            questions,
            settings,
            notifier,
            usage_sink: None,
            progress_sink: None,
            design_progress_sink: None,
            templates: None,
            uploads: None,
        }
    }

    /// Lets the engine read the saved templates, so a brief that names
    /// one can style its candidates from it.
    pub fn with_templates(mut self, templates: crate::templates::TemplateStore) -> Self {
        self.templates = Some(templates);
        self
    }

    /// Lets the engine attach the user's uploads to the requests that
    /// write content.
    pub fn with_uploads(mut self, uploads: crate::uploads::UploadStore) -> Self {
        self.uploads = Some(uploads);
        self
    }

    /// The uploads to attach to a content request: every stored file
    /// under the size caps. Empty without an upload store. A file that
    /// cannot be read is logged and skipped.
    async fn load_attachments(&self, log: &LogSink) -> Attachments {
        let mut attachments = Attachments::default();
        let Some(uploads) = &self.uploads else {
            return attachments;
        };
        let summaries = match uploads.list().await {
            Ok(summaries) => summaries,
            Err(error) => {
                log(&format!("listing uploads failed: {error}"));
                return attachments;
            }
        };
        let mut total = 0usize;
        for summary in summaries {
            let size = summary.size_bytes as usize;
            if size > ATTACHMENT_FILE_LIMIT_BYTES || total + size > ATTACHMENT_TOTAL_LIMIT_BYTES {
                attachments
                    .skipped
                    .push(format!("{} ({})", summary.name, describe_size(size)));
                continue;
            }
            match uploads.read(&summary.name).await {
                Ok(Some(bytes)) => {
                    total += bytes.len();
                    attachments.files.push(UploadAttachment {
                        name: summary.name,
                        content_type: summary.content_type.to_owned(),
                        bytes,
                    });
                }
                Ok(None) => {}
                Err(error) => log(&format!(
                    "reading upload `{}` failed: {error}",
                    summary.name
                )),
            }
        }
        if !attachments.files.is_empty() || !attachments.skipped.is_empty() {
            log(&format!(
                "attaching {} upload{} ({} skipped over the size caps)",
                attachments.files.len(),
                if attachments.files.len() == 1 {
                    ""
                } else {
                    "s"
                },
                attachments.skipped.len()
            ));
        }
        attachments
    }

    /// A user message with `text` and the attachments as content parts.
    /// Image parts go only to models that can see them.
    fn user_message(&self, text: &str, attachments: &Attachments) -> serde_json::Value {
        let can_see_images = crate::screenshots::supports_vision(&self.configuration.model);
        serde_json::json!({
            "role": "user",
            "content": user_content_with_attachments(text, attachments, can_see_images),
        })
    }

    /// The templates the brief names, in the order it names them. A
    /// template the brief names that was deleted is skipped, so the run
    /// still writes the rest.
    async fn brief_templates(
        &self,
        brief: &Brief,
        log: &LogSink,
    ) -> Vec<crate::templates::Template> {
        let ids = brief.template_ids();
        if ids.is_empty() {
            return Vec::new();
        }
        let Some(templates) = self.templates.as_ref() else {
            return Vec::new();
        };
        let mut loaded = Vec::new();
        for id in ids {
            match templates.load(&id).await {
                Ok(Some(template)) => {
                    log(&format!("styling candidates from template `{id}`"));
                    loaded.push(template);
                }
                Ok(None) => log(&format!("template `{id}` no longer exists; ignoring it")),
                Err(error) => log(&format!("reading template `{id}` failed: {error}")),
            }
        }
        loaded
    }

    /// Renews an expiring login and persists the new tokens.
    async fn refresh_login_if_needed(&mut self, log: &LogSink) -> Result<(), String> {
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

    /// Runs one turn of the conversation. The model reads the brief and
    /// the chat, then asks questions, replies, or writes candidates.
    /// User turns that arrive during the run start another turn.
    pub async fn run(mut self, log: LogSink) -> Result<(), String> {
        self.refresh_login_if_needed(&log).await?;
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        loop {
            let brief = self.load_brief().await?;
            let seen_turns = brief.messages.len();
            log(&format!(
                "built-in engine · {} · {} · effort {}",
                self.label(),
                match brief.variations {
                    Some(count) => format!("{count} variations"),
                    None => "variations not chosen".to_owned(),
                },
                brief.effort,
            ));
            self.report_progress(0);
            let appended = self.take_turn(&client, &brief, &log).await?;
            self.report_progress(100);
            let latest = self.load_brief().await?;
            if latest.messages.len() <= seen_turns + appended {
                break;
            }
            log("new message during the run; taking another turn");
        }
        log("done");
        Ok(())
    }

    /// Reads the brief, or fails when none exists.
    async fn load_brief(&self) -> Result<Brief, String> {
        self.briefs
            .read()
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "no brief exists: write one in the studio first".to_owned())
    }

    /// Plans one turn and acts on it. Returns how many turns the engine
    /// appended to the conversation.
    async fn take_turn(
        &self,
        client: &reqwest::Client,
        brief: &Brief,
        log: &LogSink,
    ) -> Result<usize, String> {
        // Continue requests name their designs; no planning is needed.
        let requests = brief.continue_requests();
        if !requests.is_empty() {
            let outcomes = self.continue_designs(client, brief, &requests, log).await;
            if outcomes.iter().all(|(_, outcome)| outcome.is_err()) {
                let failures: Vec<String> = outcomes
                    .iter()
                    .filter_map(|(id, outcome)| {
                        outcome.as_ref().err().map(|error| format!("{id}: {error}"))
                    })
                    .collect();
                return Err(failures.join("; "));
            }
            self.say(&continue_summary(&outcomes)).await?;
            return Ok(1);
        }
        let candidate_count = self.candidate_count(brief).await?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": planner_prompt() }),
            serde_json::json!({
                "role": "user",
                "content": planner_input(brief, candidate_count),
            }),
        ];
        let reply = self.chat(client, &messages, &brief.effort).await?;
        let plan = with_candidate_questions(parse_plan(&reply), brief);
        let mut appended = 0;
        if !plan.reply.trim().is_empty() {
            self.say(&plan.reply).await?;
            appended += 1;
        }
        if !plan.questions.is_empty() {
            log(&format!(
                "asked {} questions; answer them in the studio",
                plan.questions.len()
            ));
            self.questions
                .write(&plan.questions)
                .await
                .map_err(|error| error.to_string())?;
            self.notifier.notify();
            return Ok(appended);
        }
        if plan.should_edit
            && let Some(design_id) = brief.editing_design()
        {
            self.edit_design(client, brief, design_id, log).await?;
            self.say("I updated the design. Tell me what else to change.")
                .await?;
            return Ok(appended + 1);
        }
        if !plan.should_generate {
            log("replied; waiting for the user");
            return Ok(appended);
        }
        let saved = self.generate_candidates(client, brief, log).await?;
        self.say(&format!(
            "I wrote {saved} candidate{} to the canvas. Tell me what to change, or pick one.",
            if saved == 1 { "" } else { "s" }
        ))
        .await?;
        Ok(appended + 1)
    }

    /// Appends an assistant turn and wakes the studio.
    async fn say(&self, content: &str) -> Result<(), String> {
        self.briefs
            .append_message(ChatMessage::assistant(content))
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }

    /// How many candidates this brief's project already has.
    async fn candidate_count(&self, brief: &Brief) -> Result<usize, String> {
        let Some(project) = &brief.project else {
            return Ok(0);
        };
        let prefix = format!("{project}-candidate-");
        let designs = self
            .designs
            .list()
            .await
            .map_err(|error| error.to_string())?;
        Ok(designs
            .iter()
            .filter(|design| design.id.starts_with(&prefix))
            .count())
    }

    /// Writes one design per requested variation. Returns the count.
    async fn generate_candidates(
        &self,
        client: &reqwest::Client,
        brief: &Brief,
        log: &LogSink,
    ) -> Result<usize, String> {
        let count = brief.variation_count();
        let attachments = Arc::new(self.load_attachments(log).await);
        let concepts = if count > 1 {
            self.plan_concepts(client, brief, count, &attachments, log)
                .await?
        } else {
            Vec::new()
        };
        self.report_progress(10);
        let base = brief
            .project
            .clone()
            .unwrap_or_else(|| design_base_id(&prompt_words(&brief.prompt)));
        let ids: Vec<String> = (1..=count)
            .map(|candidate_number| {
                if count > 1 {
                    format!("{base}-candidate-{candidate_number}")
                } else {
                    base.clone()
                }
            })
            .collect();
        let shares = self.shared_progress(&ids, 10, 90);
        let templates = self.brief_templates(brief, log).await;
        // Every candidate runs at the same time; each saves itself as
        // soon as it is ready.
        let mut tasks = tokio::task::JoinSet::new();
        for candidate_number in 1..=count {
            let engine = self.clone();
            let client = client.clone();
            let brief = brief.clone();
            let concepts = concepts.clone();
            // One look per candidate, wrapping when the user picked
            // fewer templates than candidates.
            let template = candidate_template(&templates, candidate_number);
            let attachments = Arc::clone(&attachments);
            let share = Arc::clone(&shares[candidate_number - 1]);
            let log = Arc::clone(log);
            let id = ids[candidate_number - 1].clone();
            // The card appears at once, as a placeholder with its bar.
            share(0.0);
            tasks.spawn(async move {
                let request = CandidateRequest {
                    brief: &brief,
                    candidate_number,
                    concepts: &concepts,
                    preview_screens: brief.preview_screen_count(),
                    design_id: id.clone(),
                    template: template.as_ref(),
                };
                engine
                    .generate_candidate(&client, &request, &attachments, &share, &log)
                    .await?;
                log(&format!("candidate {candidate_number}: saved as {id}"));
                Ok::<(), String>(())
            });
        }
        let mut saved = 0;
        let mut failures = Vec::new();
        while let Some(outcome) = tasks.join_next().await {
            match outcome {
                Ok(Ok(())) => saved += 1,
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(format!("candidate task failed: {error}")),
            }
        }
        if saved == 0 {
            return Err(failures.join("; "));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(saved)
    }

    /// Asks the model for `count` distinct concepts in one call. A reply
    /// that does not parse yields no concepts, and the candidates are
    /// written without them.
    async fn plan_concepts(
        &self,
        client: &reqwest::Client,
        brief: &Brief,
        count: usize,
        attachments: &Attachments,
        log: &LogSink,
    ) -> Result<Vec<Concept>, String> {
        log(&format!("planning {count} concepts"));
        let messages = vec![
            serde_json::json!({ "role": "system", "content": concept_prompt(brief, count) }),
            self.user_message(&concept_input(brief), attachments),
        ];
        let started = std::time::Instant::now();
        let reply = self.chat(client, &messages, &brief.effort).await?;
        log(&format!(
            "concepts: reply in {:.0}s",
            started.elapsed().as_secs_f32()
        ));
        let concepts = parse_concepts(&reply);
        if concepts.is_empty() {
            log("concept reply did not parse; writing candidates without concepts");
        } else {
            let names: Vec<&str> = concepts
                .iter()
                .map(|concept| concept.name.as_str())
                .collect();
            log(&format!("concepts: {}", names.join(" · ")));
        }
        Ok(concepts)
    }

    /// Applies the latest user request to the design open in the editor:
    /// the model rewrites the design, the result is validated, polished,
    /// and saved under the same id.
    async fn edit_design(
        &self,
        client: &reqwest::Client,
        brief: &Brief,
        design_id: &str,
        log: &LogSink,
    ) -> Result<(), String> {
        let design = self
            .designs
            .load(design_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("design `{design_id}` does not exist"))?;
        let design_json = serde_json::to_string(&design).map_err(|error| error.to_string())?;
        let attachments = self.load_attachments(log).await;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(&edit_prompt(brief, &design_json), &attachments),
        ];
        let original = design.clone();
        let context = DesignRequest {
            effort: &brief.effort,
            label: format!("edit {design_id}"),
            parse: Box::new(move |content| {
                crate::patch::apply_patch(&original, crate::patch::parse_patch(content)?)
            }),
            progress: self.shared_progress(&[design_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_valid_design(client, messages, &context, log)
            .await?;
        // A polish round costs a full-design rewrite; edits get one only
        // at high effort.
        let final_design = if brief.effort == "high" {
            self.polish_design(client, edited, &context, log).await?
        } else {
            edited
        };
        self.designs
            .save(design_id, &final_design)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        log(&format!("edit {design_id}: saved"));
        Ok(())
    }

    /// Continues every requested preview design at the same time. Returns
    /// one outcome per design, in request order: the screens added, or
    /// the error.
    async fn continue_designs(
        &self,
        client: &reqwest::Client,
        brief: &Brief,
        design_ids: &[&str],
        log: &LogSink,
    ) -> Vec<(String, Result<usize, String>)> {
        let ids: Vec<String> = design_ids.iter().map(|id| (*id).to_owned()).collect();
        let shares = self.shared_progress(&ids, 5, 95);
        let attachments = Arc::new(self.load_attachments(log).await);
        let mut tasks = tokio::task::JoinSet::new();
        for (index, design_id) in design_ids.iter().enumerate() {
            let engine = self.clone();
            let client = client.clone();
            let brief = brief.clone();
            let design_id = (*design_id).to_owned();
            let attachments = Arc::clone(&attachments);
            let share = Arc::clone(&shares[index]);
            let log = Arc::clone(log);
            tasks.spawn(async move {
                let outcome = engine
                    .continue_design(&client, &brief, &design_id, &attachments, &share, &log)
                    .await;
                (index, design_id, outcome)
            });
        }
        let mut outcomes: Vec<Option<(String, Result<usize, String>)>> =
            (0..design_ids.len()).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, design_id, outcome)) => outcomes[index] = Some((design_id, outcome)),
                Err(error) => log(&format!("continue task failed: {error}")),
            }
        }
        outcomes.into_iter().flatten().collect()
    }

    /// Writes the remaining screens of the preview design `design_id` in
    /// chunks of `CONTINUE_CHUNK_SCREENS`. The design is saved after every
    /// chunk, so the canvas shows it grow, then polished once it is
    /// complete. Returns how many screens were added; 0 when the design is
    /// complete already.
    async fn continue_design(
        &self,
        client: &reqwest::Client,
        brief: &Brief,
        design_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let mut design = self
            .designs
            .load(design_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("design `{design_id}` does not exist"))?;
        // A run that stopped may have left placeholder screens behind.
        // They hold no content, so they do not count as written.
        design
            .screens
            .retain(|screen| !crate::designs::is_pending_screen(screen));
        if !design.is_preview() {
            log(&format!(
                "continue {design_id}: the design is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {design_id}");
        let start = design.screens.len();
        let planned = design.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} screens written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // Every chunk runs at the same time from the same preview. The
        // board keeps what each chunk has written so far; `shown_design`
        // turns the board into the design the canvas shows.
        let board: ChunkBoard = Arc::new(std::sync::Mutex::new(
            chunks.iter().map(|_| Vec::new()).collect(),
        ));
        let saver = LiveSaver::new(self, design_id);
        let preview = design.clone();
        let show = {
            let board = Arc::clone(&board);
            let saver = saver.clone();
            let preview = preview.clone();
            let progress = Arc::clone(progress);
            let chunks = chunks.clone();
            Arc::new(move || {
                let Ok(board) = board.lock() else {
                    return;
                };
                let written: usize = board.iter().map(Vec::len).sum();
                let done = written as f32 / (planned - start).max(1) as f32;
                progress(CONTINUE_DRAFT_SHARE * done);
                saver.offer(shown_design(&preview, &chunks, &board), written);
            })
        };
        let mut tasks = tokio::task::JoinSet::new();
        for (position, chunk) in chunks.iter().enumerate() {
            let engine = self.clone();
            let client = client.clone();
            let brief = brief.clone();
            let preview = preview.clone();
            let board = Arc::clone(&board);
            let show = Arc::clone(&show);
            let log = Arc::clone(log);
            let label = label.clone();
            let chunk = *chunk;
            let attachments = Arc::clone(attachments);
            tasks.spawn(async move {
                let design_json =
                    serde_json::to_string(&preview).map_err(|error| error.to_string())?;
                let messages = vec![
                    serde_json::json!({ "role": "system", "content": system_prompt() }),
                    engine.user_message(
                        &continue_prompt(&brief, &preview, &design_json, chunk),
                        &attachments,
                    ),
                ];
                let original = preview.clone();
                let written = preview.screens.len();
                let live_board = Arc::clone(&board);
                let live_show = Arc::clone(&show);
                let context = DesignRequest {
                    effort: &brief.effort,
                    label: format!("{label} chunk {}", position + 1),
                    parse: Box::new(move |content| apply_continuation(&original, content)),
                    progress: None,
                    live: Some(Arc::new(move |text: &str| {
                        let screens = partial_continuation_screens(written, text);
                        if let Ok(mut board) = live_board.lock()
                            && screens.len() > board[position].len()
                        {
                            board[position] = screens;
                        } else {
                            return;
                        }
                        live_show();
                    })),
                };
                let continued = engine
                    .request_valid_design(&client, messages, &context, &log)
                    .await?;
                let screens: Vec<design_model::Screen> = continued.screens[written..].to_vec();
                if let Ok(mut board) = board.lock() {
                    board[position] = screens;
                }
                show();
                Ok::<(), String>(())
            });
        }
        let mut failures = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(format!("chunk task failed: {error}")),
            }
        }
        for failure in &failures {
            log(&format!("{label}: {failure}"));
        }
        let mut continued = preview.clone();
        if let Ok(board) = board.lock() {
            for screens in board.iter() {
                continued.screens.extend(screens.iter().cloned());
            }
        }
        let added = continued.screens.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the design stays continuable.
            if let Err(error) = saver.finish(&preview).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(failures.join("; "));
        }
        // A failed chunk leaves the design continuable: the outline stays
        // until every title has a screen.
        if continued.screens.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        // The polish rounds fill the last share.
        let share = Arc::clone(progress);
        let polish_context = DesignRequest {
            effort: &brief.effort,
            label: label.clone(),
            parse: Box::new(parse_design),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_design = self
            .polish_design(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_design).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new screens"));
        Ok(added)
    }

    /// Reviews a valid design as a designer, one round per effort level.
    /// An improved design that validates replaces the original; anything
    /// else keeps the original and logs why.
    async fn polish_design(
        &self,
        client: &reqwest::Client,
        mut design: Design,
        context: &DesignRequest<'_>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &context.label;
        let rounds = crate::polish::polish_rounds(context.effort);
        if rounds == 0 {
            context.report(1.0);
        }
        for round in 1..=rounds {
            let findings = crate::polish::dom_findings(&design, &self.base_url(), label, log).await;
            let images = self.screen_images(&design, label, log).await;
            log(&format!(
                "{label}: polish round {round} ({} layout findings, {} screen images)",
                findings.len(),
                images.len()
            ));
            let design_json = serde_json::to_string(&design).map_err(|error| error.to_string())?;
            let prompt = crate::polish::polish_prompt(&design_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                serde_json::json!({
                    "role": "user",
                    "content": user_content_with_images(&prompt, &images),
                }),
            ];
            let started = std::time::Instant::now();
            let content = self
                .chat_with(
                    client,
                    self.request_body(&messages, writing_effort(context.effort)),
                    None,
                )
                .await?;
            log(&format!(
                "{label}: polish reply in {:.0}s ({} chars)",
                started.elapsed().as_secs_f32(),
                content.len()
            ));
            // The review is a patch: only the screens it changes.
            let improved = crate::patch::parse_patch(&content)
                .and_then(|patch| crate::patch::apply_patch(&design, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => design = improved,
                Ok(_) => log(&format!(
                    "{label}: polished design failed validation; keeping the previous version"
                )),
                Err(parse_error) => log(&format!(
                    "{label}: polish reply unusable ({parse_error}); keeping the previous version"
                )),
            }
            context.report(DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / rounds as f32);
        }
        Ok(design)
    }

    /// PNG screenshots of the design's screens for the polish pass, at most
    /// `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images or
    /// no Chrome is installed; a failed screenshot is logged and skipped.
    async fn screen_images(&self, design: &Design, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !crate::screenshots::supports_vision(&self.configuration.model) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for screen images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = design
            .screens
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        // One Chrome per screen, all at once.
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let design = design.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_screen(&design, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: screen {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }

    /// The server's own URL, for relative image paths in screenshots.
    fn base_url(&self) -> String {
        match &self.settings {
            Some(settings) => format!("http://{}", settings.address()),
            None => "http://127.0.0.1:3000".to_owned(),
        }
    }

    /// Asks the model for one candidate, repairs it through fix rounds
    /// until it validates, and polishes it. The design is saved under
    /// `request.design_id` while it streams in, when the draft validates,
    /// and once more after the polish.
    async fn generate_candidate(
        &self,
        client: &reqwest::Client,
        request: &CandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Design, String> {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(&candidate_prompt(request), attachments),
        ];
        let saver = LiveSaver::new(self, &request.design_id);
        let live_saver = saver.clone();
        let context = DesignRequest {
            effort: &request.brief.effort,
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_design),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(design) = partial_design(text) {
                    let rank = design.screens.len();
                    live_saver.offer(design, rank);
                }
            })),
        };
        let draft = self
            .request_valid_design(client, messages, &context, log)
            .await?;
        saver.offer(draft.clone(), draft.screens.len());
        let polished = self.polish_design(client, draft, &context, log).await?;
        saver.finish(&polished).await?;
        self.designs
            .clear_user_paths(&request.design_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(polished)
    }

    /// Sends `messages`, parses the design reply, and repairs it through
    /// fix rounds until it validates.
    async fn request_valid_design(
        &self,
        client: &reqwest::Client,
        mut messages: Vec<serde_json::Value>,
        context: &DesignRequest<'_>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &context.label;
        let fix_round_limit = fix_round_limit(context.effort);
        let effort = writing_effort(context.effort);
        for round in 0..=fix_round_limit {
            log(&format!("{label}: requesting (round {})", round + 1));
            let started = std::time::Instant::now();
            let content = self
                .chat_with(
                    client,
                    self.request_body(&messages, effort),
                    context.live.as_deref(),
                )
                .await?;
            log(&format!(
                "{label}: reply in {:.0}s ({} chars)",
                started.elapsed().as_secs_f32(),
                content.len()
            ));
            match (context.parse)(&content) {
                Ok(design) => {
                    let errors = design.validate();
                    if errors.is_empty() {
                        context.report(DRAFT_SHARE);
                        return Ok(design);
                    }
                    let error_lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                    log(&format!("{label}: {} validation errors", error_lines.len()));
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    }));
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "The result failed validation:\n{}\n\
                             Fix every error. Reply with only the corrected JSON in the same format as before.",
                            error_lines.join("\n")
                        ),
                    }));
                }
                Err(parse_error) => {
                    log(&format!("{label}: {parse_error}"));
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    }));
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "That reply could not be applied ({parse_error}). \
                             Reply with only the JSON in the same format as before.",
                        ),
                    }));
                }
            }
        }
        Err(format!(
            "{label} still fails after {fix_round_limit} fix rounds"
        ))
    }

    /// Reports each request's token usage to `sink`.
    pub fn with_usage_sink(mut self, sink: UsageSink) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    /// Reports how far each turn is to `sink`, 0 to 100.
    pub fn with_progress_sink(mut self, sink: ProgressSink) -> Self {
        self.progress_sink = Some(sink);
        self
    }

    /// Reports how far each design a turn writes is, by design id.
    pub fn with_design_progress_sink(mut self, sink: DesignProgressSink) -> Self {
        self.design_progress_sink = Some(sink);
        self
    }

    /// Reports the turn's progress, when a sink is set.
    fn report_progress(&self, percent: u8) {
        if let Some(sink) = &self.progress_sink {
            sink(percent.min(100));
        }
    }

    /// One share sink per design in `design_ids`, for designs written at the
    /// same time. Each design reports its own 0.0 to 1.0: the design sink
    /// gets it as a percent under its id, and the turn progress becomes
    /// `base` plus `span` times the mean of all shares.
    fn shared_progress(&self, design_ids: &[String], base: u8, span: u8) -> Vec<ShareSink> {
        let count = design_ids.len().max(1);
        let shares = Arc::new(std::sync::Mutex::new(vec![0.0f32; count]));
        (0..count)
            .map(|index| {
                let shares = Arc::clone(&shares);
                let sink = self.progress_sink.clone();
                let design_sink = self.design_progress_sink.clone();
                let design_id = design_ids.get(index).cloned().unwrap_or_default();
                let share: ShareSink = Arc::new(move |fraction: f32| {
                    let fraction = fraction.clamp(0.0, 1.0);
                    if let Some(design_sink) = &design_sink {
                        design_sink(&design_id, (fraction * 100.0) as u8);
                    }
                    let Ok(mut shares) = shares.lock() else {
                        return;
                    };
                    shares[index] = fraction;
                    let mean = shares.iter().sum::<f32>() / shares.len() as f32;
                    if let Some(sink) = &sink {
                        sink(base.saturating_add((f32::from(span) * mean) as u8));
                    }
                });
                share
            })
            .collect()
    }

    /// One model request in the endpoint's wire format. Returns the
    /// assistant text and reports token usage to the usage sink.
    async fn chat(
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
    fn request_body(&self, messages: &[serde_json::Value], effort: &str) -> serde_json::Value {
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
    /// A provider that answers with one JSON document instead of an
    /// event stream is read the plain way.
    async fn chat_with(
        &self,
        client: &reqwest::Client,
        body: serde_json::Value,
        on_text: Option<&TextSink>,
    ) -> Result<String, String> {
        let mut request = client.post(&self.configuration.chat_url).json(&body);
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
            .map_err(|error| format!("request to {provider} failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            let mut detail = response.text().await.unwrap_or_default();
            detail.truncate(300);
            return Err(format!("{provider} returned {status}: {detail}"));
        }
        let mut state = StreamState::default();
        let mut raw = String::new();
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(error) => return Err(format!("reading the {provider} reply failed: {error}")),
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
            let value: serde_json::Value =
                serde_json::from_str(&raw).map_err(|error| error.to_string())?;
            self.report_usage(parse_usage(&value["usage"]));
            let content = match self.configuration.wire {
                WireFormat::OpenAiChat => value["choices"][0]["message"]["content"].as_str(),
                WireFormat::AnthropicMessages => value["content"][0]["text"].as_str(),
                WireFormat::ChatGptResponses => None,
            };
            return content
                .map(str::to_owned)
                .ok_or_else(|| "response has no message content".to_owned());
        }
        self.report_usage(state.usage);
        if state.collected.is_empty() {
            return Err(state
                .failure
                .unwrap_or_else(|| "the stream carried no output text".to_owned()));
        }
        Ok(state.collected)
    }

    fn report_usage(&self, usage: Option<TokenUsage>) {
        if let (Some(sink), Some(usage)) = (&self.usage_sink, usage) {
            sink(usage);
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

/// A user message content: the text alone, or OpenAI-style parts with
/// one `image_url` data URL per PNG when images are present.
fn user_content_with_images(text: &str, images: &[Vec<u8>]) -> serde_json::Value {
    if images.is_empty() {
        return serde_json::Value::String(text.to_owned());
    }
    let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
    for (index, image) in images.iter().enumerate() {
        parts.push(serde_json::json!({ "type": "text", "text": format!("Screen {}:", index + 1) }));
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", crate::export::base64_encode(image)),
            },
        }));
    }
    serde_json::Value::Array(parts)
}

/// One upload attached to a model request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadAttachment {
    /// Stored upload name, as in `/uploads/{name}`.
    pub name: String,
    /// Content type from the extension.
    pub content_type: String,
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// The uploads a request carries, plus the ones the size caps left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachments {
    /// Files sent as content parts, in listing order.
    pub files: Vec<UploadAttachment>,
    /// `name (size)` of every file skipped over the caps.
    pub skipped: Vec<String>,
}

/// Largest single upload attached to a request.
const ATTACHMENT_FILE_LIMIT_BYTES: usize = 20 * 1024 * 1024;

/// Largest total of uploads attached to one request.
const ATTACHMENT_TOTAL_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// Longest text file inlined into a request; the rest is cut with a
/// note.
const ATTACHMENT_TEXT_LIMIT_BYTES: usize = 100 * 1024;

/// Providers on the OpenAI chat wire that read `file` parts.
fn accepts_file_parts(provider: &str) -> bool {
    matches!(provider, "openai" | "openrouter")
}

/// A short human size: `512 B`, `3.4 KB`, `1.2 MB`.
fn describe_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// A user message content: the text alone when nothing is attached, or
/// OpenAI-style parts: the text, then each file. Images become
/// `image_url` data URLs when `can_see_images`, PDFs become `file`
/// parts, text files are inlined, and anything else is named only.
fn user_content_with_attachments(
    text: &str,
    attachments: &Attachments,
    can_see_images: bool,
) -> serde_json::Value {
    if attachments.files.is_empty() && attachments.skipped.is_empty() {
        return serde_json::Value::String(text.to_owned());
    }
    let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
    parts.push(serde_json::json!({
        "type": "text",
        "text": "The user's source files follow. Use their content for the design. \
                 Use an image file as <img src='/uploads/{name}'>.",
    }));
    for file in &attachments.files {
        parts.extend(attachment_parts(file, can_see_images));
    }
    if !attachments.skipped.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": format!(
                "Not attached, over the size cap: {}.",
                attachments.skipped.join(", ")
            ),
        }));
    }
    serde_json::Value::Array(parts)
}

/// The content parts for one attached file.
fn attachment_parts(file: &UploadAttachment, can_see_images: bool) -> Vec<serde_json::Value> {
    let size = describe_size(file.bytes.len());
    let label = |detail: &str| {
        serde_json::json!({
            "type": "text",
            "text": format!("File {} ({}, {size}){detail}", file.name, file.content_type),
        })
    };
    if file.content_type.starts_with("image/") {
        if !can_see_images {
            return vec![label(&format!(
                ": an image this model cannot see here. Use it as <img src='/uploads/{}'> when it fits.",
                file.name
            ))];
        }
        return vec![
            label(":"),
            serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!(
                        "data:{};base64,{}",
                        file.content_type,
                        crate::export::base64_encode(&file.bytes)
                    ),
                },
            }),
        ];
    }
    if file.content_type == "application/pdf" {
        return vec![
            label(":"),
            serde_json::json!({
                "type": "file",
                "file": {
                    "filename": file.name,
                    "file_data": format!(
                        "data:application/pdf;base64,{}",
                        crate::export::base64_encode(&file.bytes)
                    ),
                },
            }),
        ];
    }
    if file.content_type.starts_with("text/") || file.content_type == "application/json" {
        let text = String::from_utf8_lossy(&file.bytes);
        let (shown, note) = if text.len() > ATTACHMENT_TEXT_LIMIT_BYTES {
            let mut end = ATTACHMENT_TEXT_LIMIT_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            (&text[..end], "\n[cut: the file continues]")
        } else {
            (&text[..], "")
        };
        return vec![serde_json::json!({
            "type": "text",
            "text": format!("File {} ({}, {size}):\n{shown}{note}", file.name, file.content_type),
        })];
    }
    vec![label(
        ": a file this request cannot carry. Tell the user if you need its content.",
    )]
}

/// Replaces every `file` part in `messages` with a text note, for
/// providers that reject file parts.
fn without_file_parts(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
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
fn anthropic_messages_body(model: &str, messages: &[serde_json::Value]) -> serde_json::Value {
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
fn chatgpt_responses_body(
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

/// A sink for the accumulated text of a streaming reply.
type TextSink = dyn Fn(&str) + Send + Sync;

/// The reasoning effort for requests that write screen HTML: one level
/// under the brief's effort. Writing markup gains little from long
/// reasoning, and the effort level still sets the fix and polish
/// rounds.
fn writing_effort(effort: &str) -> &'static str {
    match effort {
        "low" => "minimal",
        "high" => "medium",
        _ => "low",
    }
}

/// The complete objects of the JSON array under `key` in a partial
/// JSON text: the index of the array's `[`, and each complete `{…}`
/// item. `None` until the array has started.
fn complete_array_items<'text>(text: &'text str, key: &str) -> Option<(usize, Vec<&'text str>)> {
    let quoted = format!("\"{key}\"");
    let key_position = text.find(&quoted)?;
    let after_key = &text[key_position + quoted.len()..];
    let bracket_offset = after_key.find('[')?;
    // Only a colon and spaces may sit between the key and its array.
    if !after_key[..bracket_offset]
        .trim()
        .trim_start_matches(':')
        .trim()
        .is_empty()
    {
        return None;
    }
    let array_start = key_position + quoted.len() + bracket_offset;
    let mut items = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut is_escaped = false;
    let mut item_start = None;
    for (offset, character) in text[array_start + 1..].char_indices() {
        let position = array_start + 1 + offset;
        if in_string {
            if is_escaped {
                is_escaped = false;
            } else if character == '\\' {
                is_escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    item_start = Some(position);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0
                    && let Some(start) = item_start.take()
                {
                    items.push(&text[start..=position]);
                }
            }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    Some((array_start, items))
}

/// The design a streaming reply has written so far: everything before
/// the screens plus every complete screen. `None` until the first screen
/// is complete, or when the text before the screens is not a design.
fn partial_design(text: &str) -> Option<Design> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "screens")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new screens a streaming continuation reply has completed so far.
fn partial_continuation_screens(written: usize, text: &str) -> Vec<design_model::Screen> {
    let Some((_, items)) = complete_array_items(text, "screens") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"screens\":[{}]}}", items.join(","));
    continuation_screens(written, &json).unwrap_or_default()
}

/// The design to show while the chunks run: the preview, then every chunk
/// up to the last one that has screens. A chunk with screens still to come
/// is padded with one placeholder per outline title it owes, so the
/// screens of a later chunk appear in their real place instead of waiting
/// for the chunks before them.
///
/// Nothing is padded past the last chunk with screens. The shown design
/// therefore stays shorter than its outline until the run finishes, so a
/// run that dies leaves a design the user can still continue.
fn shown_design(
    preview: &Design,
    chunks: &[ContinueChunk],
    board: &[Vec<design_model::Screen>],
) -> Design {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|screens| !screens.is_empty()) else {
        return shown;
    };
    for (chunk, screens) in chunks.iter().zip(board).take(last) {
        shown.screens.extend(screens.iter().cloned());
        for offset in screens.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.screens.push(placeholder_screen(title));
        }
    }
    // The last chunk with screens ends the design. Padding after it would
    // fill every outline slot and make the design look finished.
    shown.screens.extend(board[last].iter().cloned());
    shown
}

/// A screen that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a design that does not.
fn placeholder_screen(title: &str) -> design_model::Screen {
    design_model::Screen {
        html: format!(
            "<div class=\"{pending} pending\"><p class=\"pending-label\">Writing</p>\
             <h2 class=\"pending-title\">{title}</h2></div>",
            pending = crate::designs::PENDING_SCREEN_CLASS,
            title = crate::render::escape_html(title),
        ),
        css: Some(
            ".pending { display: flex; flex-direction: column; align-items: center; \
             justify-content: center; height: 100%; gap: 24px; opacity: 0.55; }\n\
             .pending-label { margin: 0; font-size: 26px; letter-spacing: 0.3em; \
             text-transform: uppercase; color: var(--muted); }\n\
             .pending-title { margin: 0; max-width: 1400px; text-align: center; \
             font-size: 64px; color: var(--text); }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Saves a design while it streams in, so the canvas shows the screens
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct LiveSaver {
    designs: DesignStore,
    notifier: ChangeNotifier,
    design_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl LiveSaver {
    fn new(engine: &GenerationEngine, design_id: &str) -> Self {
        Self {
            designs: engine.designs.clone(),
            notifier: engine.notifier.clone(),
            design_id: design_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Offers a partial design. It is saved when it validates and its
    /// `rank` is above the last saved rank. `rank` counts the real
    /// progress behind the design, which is not always its screen count: a
    /// continuation pads the gaps with placeholder screens, so only the
    /// screens the model wrote may raise it.
    fn offer(&self, design: Design, rank: usize) {
        if !design.validate().is_empty() {
            return;
        }
        {
            let Ok(mut saved) = self.saved_rank.lock() else {
                return;
            };
            if saved.is_some_and(|saved| rank <= saved) {
                return;
            }
            *saved = Some(rank);
        }
        let saver = self.clone();
        tokio::spawn(async move {
            let _guard = saver.write_lock.lock().await;
            if saver.designs.save(&saver.design_id, &design).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final design after every partial save landed.
    async fn finish(&self, design: &Design) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.designs
            .save(&self.design_id, design)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// One continuation chunk: the zero-based outline index of its first
/// title and how many titles it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContinueChunk {
    first: usize,
    count: usize,
}

/// The chunks that cover outline titles `start..planned`, each at most
/// `CONTINUE_CHUNK_SCREENS` long.
fn continue_chunks(start: usize, planned: usize) -> Vec<ContinueChunk> {
    (start..planned)
        .step_by(CONTINUE_CHUNK_SCREENS)
        .map(|first| ContinueChunk {
            first,
            count: (planned - first).min(CONTINUE_CHUNK_SCREENS),
        })
        .collect()
}

/// The planner's system prompt: one turn of the design conversation.
fn planner_prompt() -> String {
    format!(
        "You plan screen designs with the user.\n\
         Read the brief and the conversation. Reply with only this JSON:\n\
         {{\"reply\":\"text for the user\",\"questions\":[{{\"question\":\"...\",\"options\":[\"...\"]}}],\"generate\":false,\"edit\":false}}\n\
         Use questions when you need a choice from the user. Ask at most 3. Give 2 to 4 short options for each.\n\
         The app asks four questions itself: the scenario, the design length in screens, the number of candidates, and how different the candidates are. Never ask these. The input shows their answers, or `not chosen yet` when the app has not asked them yet.\n\
         After {limit} answered questions, do not ask more.\n\
         Set generate to true when you know enough to write the design. Then say in reply what you will write.\n\
         When the input names a design open in the editor and the user asks for a change, set edit to true and generate to false. Say in reply what you will change. The app applies the change to that design.\n\
         When no design is open and candidates exist and the user asks for changes, set generate to true to write new candidates.\n\
         When the user only chats, set generate to false and answer in reply.\n\
         Keep reply to 1 to 3 sentences. Reply with only the JSON.",
        limit = crate::questions::QUESTION_LIMIT
    )
}

/// The planner's user input: brief, conversation, and canvas state.
fn planner_input(brief: &Brief, candidate_count: usize) -> String {
    let mut input = format!(
        "Brief:\n{}\nScenario: {}\nLength in screens: {}\nVariations requested: {}\nEffort: {}\nVariety: {}\n",
        brief.prompt,
        brief.scenario.as_deref().unwrap_or("not chosen yet"),
        brief.length.as_deref().unwrap_or("not chosen yet"),
        match brief.variations {
            Some(count) => count.to_string(),
            None => "not chosen yet".to_owned(),
        },
        brief.effort,
        brief.variety.as_deref().unwrap_or("not chosen yet")
    );
    input.push_str(&format!("Candidates on the canvas: {candidate_count}\n"));
    input.push_str(&format!(
        "Design open in the editor: {}\n",
        brief.editing_design().unwrap_or("none")
    ));
    input.push_str(&format!(
        "Questions answered so far: {}\n",
        brief.answers.len()
    ));
    if brief.messages.is_empty() {
        input.push_str("Conversation: none yet.\n");
    } else {
        input.push_str("Conversation, oldest first:\n");
        for message in &brief.messages {
            match &message.design {
                Some(design) => input.push_str(&format!(
                    "{} (editing {design}): {}\n",
                    message.role, message.content
                )),
                None => input.push_str(&format!("{}: {}\n", message.role, message.content)),
            }
        }
    }
    input.push_str("Reply with only the JSON.");
    input
}

/// One planned turn, parsed from the model's reply.
#[derive(Debug, Default)]
struct Plan {
    /// Text for the user. Empty means nothing to say.
    reply: String,
    /// Questions for the studio, at most `QUESTION_LIMIT`.
    questions: Vec<crate::questions::Question>,
    /// True when the model wants to write candidates now.
    should_generate: bool,
    /// True when the model wants to apply the request to the design open
    /// in the editor.
    should_edit: bool,
}

/// Parses a planner reply. Prose that is not JSON becomes the reply
/// text, so the user still sees it.
fn parse_plan(content: &str) -> Plan {
    #[derive(serde::Deserialize)]
    struct PlanReply {
        #[serde(default)]
        reply: String,
        #[serde(default)]
        questions: Vec<crate::questions::Question>,
        #[serde(default)]
        generate: bool,
        #[serde(default)]
        edit: bool,
    }
    let parsed = content
        .find('{')
        .zip(content.rfind('}'))
        .filter(|(start, end)| end > start)
        .and_then(|(start, end)| serde_json::from_str::<PlanReply>(&content[start..=end]).ok());
    let Some(parsed) = parsed else {
        return Plan {
            reply: content.trim().to_owned(),
            questions: Vec::new(),
            should_generate: false,
            should_edit: false,
        };
    };
    let mut questions: Vec<_> = parsed
        .questions
        .into_iter()
        .filter(|question| !question.question.trim().is_empty())
        .collect();
    questions.truncate(crate::questions::QUESTION_LIMIT);
    Plan {
        reply: parsed.reply,
        questions,
        should_generate: parsed.generate,
        should_edit: parsed.edit,
    }
}

/// The system prompt: role, rules, schema, and one example design.
fn system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Design)).unwrap_or_default();
    format!(
        "You build screen designs as JSON documents. Each screen is one HTML fragment plus its own CSS, \
         for a 1920 by 1080 px canvas.\n\
         Follow these rules:\n{rules}\n\
         The design must conform to this JSON Schema:\n{schema}\n\
         Example design:\n{example}\n\
         Always reply with only one design JSON document. No prose, no code fences.",
        rules = CONTENT_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-design.json"),
    )
}

/// Adds the app's candidate questions to a plan that asks or would
/// generate: the scenario when none is chosen, the length when none is
/// chosen, the count when none is chosen, and the variety when none is
/// chosen and the count is not one. A plan that would generate asks
/// first instead.
fn with_candidate_questions(mut plan: Plan, brief: &Brief) -> Plan {
    if !plan.should_generate && plan.questions.is_empty() {
        return plan;
    }
    let needs_scenario = brief.scenario.is_none();
    let needs_length = brief.length.is_none();
    let needs_count = brief.variations.is_none();
    let needs_variety = brief.variations != Some(1) && brief.variety.is_none();
    if !needs_scenario && !needs_length && !needs_count && !needs_variety {
        return plan;
    }
    let has_scenario = plan
        .questions
        .iter()
        .any(|question| crate::candidate_questions::is_scenario_question(&question.question));
    let has_length = plan
        .questions
        .iter()
        .any(|question| crate::candidate_questions::is_length_question(&question.question));
    let has_count = plan
        .questions
        .iter()
        .any(|question| crate::candidate_questions::is_variation_question(&question.question));
    let has_variety = plan
        .questions
        .iter()
        .any(|question| crate::candidate_questions::is_variety_question(&question.question));
    if needs_scenario && !has_scenario {
        plan.questions
            .push(crate::candidate_questions::scenario_question());
    }
    if needs_length && !has_length {
        plan.questions
            .push(crate::candidate_questions::length_question());
    }
    if needs_count && !has_count {
        plan.questions
            .push(crate::candidate_questions::variation_question());
    }
    if needs_variety && !has_variety {
        plan.questions
            .push(crate::candidate_questions::variety_question());
    }
    plan.should_generate = false;
    plan
}

/// Turns a model reply into a design: a whole design, or a patch applied
/// to an existing one.
type ReplyParser<'request> = Box<dyn Fn(&str) -> Result<Design, String> + Send + Sync + 'request>;

/// Effort, log label, and reply parser for one design request: a
/// candidate (the reply is a design) or an edit (the reply is a patch).
struct DesignRequest<'request> {
    effort: &'request str,
    label: String,
    parse: ReplyParser<'request>,
    /// Where this request reports its share of the turn: `DRAFT_SHARE`
    /// once the draft validates, then up to 1.0 over the polish rounds.
    progress: Option<ShareSink>,
    /// Gets the accumulated reply text while it streams, so the caller
    /// can show partial results.
    live: Option<Arc<TextSink>>,
}

impl DesignRequest<'_> {
    /// Reports this request's share, when it has a sink.
    fn report(&self, fraction: f32) {
        if let Some(progress) = &self.progress {
            progress(fraction);
        }
    }
}

/// The user prompt for an edit: the design as it is, the conversation,
/// and the latest request.
fn edit_prompt(brief: &Brief, design_json: &str) -> String {
    let mut prompt = format!("Here is the design the user is editing:\n{design_json}\n");
    if !brief.messages.is_empty() {
        prompt.push_str("The conversation so far, oldest first:\n");
        for message in &brief.messages {
            prompt.push_str(&format!("- {}: {}\n", message.role, message.content));
        }
    }
    let latest = brief
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .unwrap_or("");
    prompt.push_str(&format!(
        "Apply this request to the design: {latest}\n\
         A reference like [screen 3, node 0/1 <h2.title>: What Swift Design does] names a screen \
         (1-based) and one element in that screen's html by its index path from the screen root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [upload chart.png] names one of the user's \
         source files; an image file goes into a screen as <img src='/uploads/chart.png'>. \
         Change only what the request asks for. Keep every other screen and \
         value as it is. Return every changed screen complete: html, css, and notes.\n{}",
        crate::patch::PATCH_FORMAT
    ));
    prompt
}

/// The first words of a prompt, for a design id when no project is set.
fn prompt_words(prompt: &str) -> String {
    prompt
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What one candidate call needs: the brief, the candidate number,
/// every concept, so the prompt can name its own and the others, and
/// the preview length when the candidate is a preview.
struct CandidateRequest<'request> {
    brief: &'request Brief,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` screens plus the outline.
    preview_screens: Option<usize>,
    /// The id the candidate is saved under.
    design_id: String,
    /// The template the candidate takes its look from, when the brief
    /// names one.
    template: Option<&'request crate::templates::Template>,
}

/// The prompt lines for a preview candidate: write `count` screens and
/// the full outline. The length wording replaces `length_note`, which
/// would ask for the complete design.
fn preview_note(count: usize, length: Option<(usize, usize)>) -> String {
    let outline_length = match length {
        Some((min, max)) if min == max => format!("{min} titles"),
        Some((min, max)) => format!("between {min} and {max} titles"),
        None => "every screen title of the complete design".to_owned(),
    };
    format!(
        "Write a preview: only the first {count} screens of the design, in order, starting with the \
         title screen. Put the screen titles of the complete design in `outline`, in order, {outline_length}, \
         counting the title screen. The app asks you for the remaining screens later. Make these {count} \
         screens show the theme, the layout language, and the text density of the whole design.\n"
    )
}

/// The reply for the user after continue requests: one sentence per
/// design. Designs that were complete already are left out unless nothing
/// else happened.
fn continue_summary(outcomes: &[(String, Result<usize, String>)]) -> String {
    let mut lines: Vec<String> = outcomes
        .iter()
        .filter_map(|(design_id, outcome)| match outcome {
            Ok(0) => None,
            Ok(1) => Some(format!("I wrote the last screen of `{design_id}`.")),
            Ok(added) => Some(format!(
                "I wrote the remaining {added} screens of `{design_id}`."
            )),
            Err(error) => Some(format!("I could not continue `{design_id}`: {error}.")),
        })
        .collect();
    if lines.is_empty() {
        lines = outcomes
            .iter()
            .map(|(design_id, _)| format!("`{design_id}` is complete already."))
            .collect();
    }
    lines.push("Tell me what to change, or continue another candidate.".to_owned());
    lines.join(" ")
}

/// The user prompt for one continuation chunk: the preview design, the
/// conversation, and the chunk's screens to add, as a patch of inserts.
fn continue_prompt(
    brief: &Brief,
    design: &Design,
    design_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = design.screens.len();
    let planned = design.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = design
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a design in progress: its theme, its first {written} screens, and `outline`, the \
         screen titles of the complete design:\n{design_json}\n\
         The design is for this brief:\n{}\n",
        brief.prompt
    );
    if !brief.messages.is_empty() {
        prompt.push_str("The conversation so far, oldest first. Follow every request in it:\n");
        for message in &brief.messages {
            prompt.push_str(&format!("- {}: {}\n", message.role, message.content));
        }
    }
    if let Some(scenario) = &brief.scenario {
        prompt.push_str(&crate::candidate_questions::scenario_note(scenario));
    }
    prompt.push_str(&format!(
        "Write {} screens: outline titles {} to {last} of {planned}, in order, one screen per \
         title:\n{}\n\
         Keep the theme. Match the existing screens in CSS style, font sizes, spacing, colors, \
         and visual language, so the design reads as one design. Do not change or repeat the \
         existing screens.\n",
        next_titles.len(),
        first + 1,
        next_titles.join("\n")
    ));
    if first > written || last < planned {
        prompt.push_str(
            "Other requests write the other outline titles at the same time. Write only these.\n",
        );
    }
    match brief.effort.as_str() {
        "low" => prompt.push_str("Keep the text on each screen short.\n"),
        "high" => prompt.push_str(
            "Work carefully: complete content, strong structure, thorough presenter notes.\n",
        ),
        _ => {}
    }
    prompt.push_str(&format!(
        "Reply with only a JSON patch that appends the new screens, not the whole design:\n\
         {{\"screens\":[{{\"index\":{written},\"insert\":true,\"screen\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new screen index {written} and insert true, in presentation order. Each screen \
         carries html, css, and notes. Omit title, theme, outline, and the existing screens."
    ));
    prompt
}

/// The new screens in a continuation reply, in order. Accepts a patch
/// (the screens of its operations at or past the existing screens) and,
/// as a fallback, a whole design (its screens past the existing ones).
/// Existing screens are never touched.
fn continuation_screens(
    written: usize,
    content: &str,
) -> Result<Vec<design_model::Screen>, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    let value: serde_json::Value = serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid JSON: {error}"))?;
    let items = value
        .get("screens")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no screens array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("screen").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("screen"))
            .filter(|screen| screen.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, screen)| {
            serde_json::from_value::<design_model::Screen>(screen.clone()).map_err(|error| {
                format!(
                    "new screen {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new screens to the design in progress. The outline
/// stays until every title has a screen, so a short reply leaves the
/// design continuable.
fn apply_continuation(original: &Design, content: &str) -> Result<Design, String> {
    let new_screens = continuation_screens(original.screens.len(), content)?;
    if new_screens.is_empty() {
        return Err(
            "the reply adds no screens: reply with a patch of inserts, one per new screen"
                .to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.screens.extend(new_screens);
    if continued.screens.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The user prompt for one candidate: brief, conversation, scenario,
/// variety note, and concept. Answered questions are part of the
/// conversation.
fn candidate_prompt(request: &CandidateRequest<'_>) -> String {
    let brief = request.brief;
    let candidate_number = request.candidate_number;
    let mut prompt = format!("Build a design for this brief:\n{}\n", brief.prompt);
    if !brief.messages.is_empty() {
        prompt.push_str("The conversation so far, oldest first. Follow every request in it:\n");
        for message in &brief.messages {
            prompt.push_str(&format!("- {}: {}\n", message.role, message.content));
        }
    }
    if let Some(scenario) = &brief.scenario {
        prompt.push_str(&crate::candidate_questions::scenario_note(scenario));
    }
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
    }
    let has_length = brief.length_bounds().is_some();
    match (request.preview_screens, &brief.length) {
        (Some(count), _) => prompt.push_str(&preview_note(count, brief.length_bounds())),
        (None, Some(length)) => prompt.push_str(&crate::candidate_questions::length_note(length)),
        (None, None) => {}
    }
    let count = brief.variation_count();
    if count > 1 {
        prompt.push_str(&format!(
            "This is candidate {candidate_number} of {count}. {}\n",
            crate::candidate_questions::variety_note(
                brief
                    .variety
                    .as_deref()
                    .unwrap_or(crate::candidate_questions::DEFAULT_VARIETY)
            )
        ));
        prompt.push_str(&concept_note(request.concepts, candidate_number - 1));
    }
    match brief.effort.as_str() {
        "low" if has_length => prompt.push_str("Keep the text on each screen short.\n"),
        "low" => prompt.push_str("Keep the design concise: fewer screens, short text.\n"),
        "high" => prompt.push_str(
            "Work carefully: complete content, strong structure, thorough presenter notes.\n",
        ),
        _ => {}
    }
    prompt.push_str("Reply with only the design JSON.");
    prompt
}

/// The template candidate `candidate_number` takes its look from.
/// Candidates are numbered from one and wrap around the chosen
/// templates, so three looks across five candidates run 1, 2, 3, 1, 2.
fn candidate_template(
    templates: &[crate::templates::Template],
    candidate_number: usize,
) -> Option<crate::templates::Template> {
    if templates.is_empty() {
        return None;
    }
    templates
        .get((candidate_number - 1) % templates.len())
        .cloned()
}

/// The prompt lines for a template: the theme to copy and the screens to
/// match. The template gives the look. The brief gives the content.
fn template_note(template: &crate::templates::Template) -> String {
    let mut note = format!(
        "Use the saved template `{name}` for the look of this design.\n\
         Copy this theme into the design exactly. Use it instead of any palette or fonts \
         named elsewhere in this prompt:\n{theme}\n",
        name = template.name,
        theme = serde_json::to_string(&template.theme).unwrap_or_default(),
    );
    note.push_str(
        "These screens show the template style. Match their CSS: the same font sizes, \
         spacing, alignment, colors, and layout language. Write new content for the \
         brief. Do not copy their text.\n",
    );
    for (index, screen) in template.screens.iter().enumerate() {
        let Ok(json) = serde_json::to_string(screen) else {
            continue;
        };
        note.push_str(&format!("Template screen {}: {json}\n", index + 1));
    }
    note
}

/// Extracts and parses the design JSON from a model reply.
fn parse_design(content: &str) -> Result<Design, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid design: {error}"))
}

/// A valid design id derived from the design title.
fn design_base_id(title: &str) -> String {
    let mut id = String::new();
    let mut previous_was_hyphen = true;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
            previous_was_hyphen = false;
        } else if !previous_was_hyphen {
            id.push('-');
            previous_was_hyphen = true;
        }
        if id.len() >= 40 {
            break;
        }
    }
    let id = id.trim_matches('-').to_owned();
    let id = match id.find("-candidate-") {
        Some(position) => id[..position].to_owned(),
        None => id,
    };
    if crate::designs::is_valid_design_id(&id) {
        id
    } else {
        "design".to_owned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::briefs::{Brief, BriefAnswer, ChatMessage};
    use crate::concepts::Concept;
    use crate::generation::{
        Attachments, CandidateRequest, Plan, TokenUsage, UploadAttachment, anthropic_messages_body,
        candidate_prompt, candidate_template, chatgpt_responses_body, context_window,
        describe_size, design_base_id, edit_prompt, parse_design, parse_plan, parse_usage,
        planner_input, system_prompt, user_content_with_attachments, user_content_with_images,
        with_candidate_questions, without_file_parts,
    };

    fn attachment(name: &str, content_type: &str, bytes: &[u8]) -> UploadAttachment {
        UploadAttachment {
            name: name.to_owned(),
            content_type: content_type.to_owned(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn attachments_become_image_file_and_text_parts() {
        let attachments = Attachments {
            files: vec![
                attachment("chart.png", "image/png", &[1, 2, 3]),
                attachment("brief.pdf", "application/pdf", b"%PDF"),
                attachment(
                    "notes.md",
                    "text/markdown; charset=utf-8",
                    b"# Plan\nShip it.",
                ),
                attachment("blob.bin", "application/octet-stream", &[0]),
            ],
            skipped: vec!["huge.pdf (40.0 MB)".to_owned()],
        };
        let content = user_content_with_attachments("Write.", &attachments, true);
        let parts = content.as_array().unwrap();
        assert_eq!(parts[0]["text"], "Write.");
        assert!(
            parts[1]["text"]
                .as_str()
                .unwrap()
                .contains("source files follow")
        );
        assert!(
            parts[2]["text"]
                .as_str()
                .unwrap()
                .starts_with("File chart.png (image/png, 3 B)")
        );
        assert_eq!(parts[3]["type"], "image_url");
        assert!(
            parts[3]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,AQID")
        );
        assert_eq!(parts[5]["type"], "file");
        assert_eq!(parts[5]["file"]["filename"], "brief.pdf");
        assert!(
            parts[5]["file"]["file_data"]
                .as_str()
                .unwrap()
                .starts_with("data:application/pdf;base64,")
        );
        assert!(
            parts[6]["text"]
                .as_str()
                .unwrap()
                .contains("# Plan\nShip it.")
        );
        assert!(parts[7]["text"].as_str().unwrap().contains("blob.bin"));
        assert!(parts[7]["text"].as_str().unwrap().contains("cannot carry"));
        assert!(
            parts[8]["text"]
                .as_str()
                .unwrap()
                .contains("huge.pdf (40.0 MB)")
        );
        // Without vision, an image is named but not sent.
        let blind = user_content_with_attachments("Write.", &attachments, false);
        let blind_parts = blind.as_array().unwrap();
        assert!(blind_parts.iter().all(|part| part["type"] != "image_url"));
        assert!(
            blind_parts[2]["text"]
                .as_str()
                .unwrap()
                .contains("<img src='/uploads/chart.png'>")
        );
        // Nothing attached keeps the plain string.
        assert_eq!(
            user_content_with_attachments("x", &Attachments::default(), true),
            "x"
        );
    }

    #[test]
    fn file_parts_convert_to_documents_and_input_files() {
        let attachments = Attachments {
            files: vec![attachment("brief.pdf", "application/pdf", b"%PDF")],
            skipped: Vec::new(),
        };
        let content = user_content_with_attachments("Write.", &attachments, true);
        let messages = vec![serde_json::json!({ "role": "user", "content": content })];
        let anthropic = anthropic_messages_body("claude", &messages);
        let blocks = anthropic["messages"][0]["content"].as_array().unwrap();
        let document = blocks
            .iter()
            .find(|block| block["type"] == "document")
            .unwrap();
        assert_eq!(document["source"]["media_type"], "application/pdf");
        assert_eq!(document["source"]["data"], "JVBERg==");
        assert_eq!(document["title"], "brief.pdf");
        let chatgpt = chatgpt_responses_body("gpt-5", &messages, "low");
        let items = chatgpt["input"][0]["content"].as_array().unwrap();
        let file = items
            .iter()
            .find(|item| item["type"] == "input_file")
            .unwrap();
        assert_eq!(file["filename"], "brief.pdf");
        assert!(
            file["file_data"]
                .as_str()
                .unwrap()
                .starts_with("data:application/pdf;base64,")
        );
        // Providers without file support get a note.
        let stripped = without_file_parts(&messages);
        let parts = stripped[0]["content"].as_array().unwrap();
        assert!(parts.iter().all(|part| part["type"] == "text"));
        assert!(parts.iter().any(|part| {
            part["text"]
                .as_str()
                .unwrap()
                .contains("brief.pdf cannot be sent")
        }));
        let plain = without_file_parts(&[serde_json::json!({ "role": "user", "content": "hi" })]);
        assert_eq!(plain[0]["content"], "hi");
    }

    #[test]
    fn sizes_describe_bytes_kilobytes_and_megabytes() {
        assert_eq!(describe_size(512), "512 B");
        assert_eq!(describe_size(3 * 1024 + 410), "3.4 KB");
        assert_eq!(describe_size(40 * 1024 * 1024), "40.0 MB");
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
    fn parses_a_design_wrapped_in_prose_and_fences() {
        let sample = include_str!("../../../fixtures/sample-design.json");
        let wrapped = format!("Here is the design:\n```json\n{sample}\n```\nDone.");
        let design = parse_design(&wrapped).unwrap();
        assert_eq!(design.title, "Swift Design Overview");
        assert!(parse_design("no json here").is_err());
    }

    #[test]
    fn design_ids_come_from_titles_and_stay_valid() {
        assert_eq!(design_base_id("Q3 Sales — Review!"), "q3-sales-review");
        assert_eq!(design_base_id("???"), "design");
        assert_eq!(design_base_id("My Talk Candidate 1"), "my-talk");
        assert_eq!(design_base_id("render"), "design");
    }

    #[test]
    fn chatgpt_bodies_split_instructions_from_input() {
        let messages = vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "hello"}),
            serde_json::json!({"role": "assistant", "content": "draft"}),
        ];
        let body = crate::generation::chatgpt_responses_body("gpt-5.5", &messages, "high");
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
        let (content, usage) = crate::generation::parse_chatgpt_stream(stream).unwrap();
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
        assert!(
            crate::generation::parse_chatgpt_stream(failed)
                .unwrap_err()
                .contains("quota")
        );
    }

    #[test]
    fn prompts_carry_rules_brief_and_conversation() {
        assert!(system_prompt().contains("\"html\""));
        assert!(system_prompt().contains("1920 by 1080"));
        let brief = Brief {
            prompt: "A schema talk.".to_owned(),
            variations: Some(3),
            project: Some("talk".to_owned()),
            effort: "medium".to_owned(),
            preview: false,
            variety: Some("high".to_owned()),
            scenario: Some("Finance".to_owned()),
            length: Some("10-15".to_owned()),
            templates: Vec::new(),
            template: None,
            answers: vec![BriefAnswer {
                question: "How long?".to_owned(),
                answer: "10 min".to_owned(),
            }],
            messages: vec![
                ChatMessage::assistant("How long?"),
                ChatMessage::user("10 min", Some("talk-candidate-1")),
            ],
        };
        let concepts = vec![
            Concept {
                name: "One".to_owned(),
                ..Concept::default()
            },
            Concept {
                name: "Two".to_owned(),
                angle: "second".to_owned(),
                ..Concept::default()
            },
        ];
        let request = CandidateRequest {
            brief: &brief,
            candidate_number: 2,
            concepts: &concepts,
            preview_screens: None,
            design_id: "talk-candidate-2".to_owned(),
            template: None,
        };
        let prompt = candidate_prompt(&request);
        assert!(prompt.contains("\"name\":\"Two\""));
        assert!(prompt.contains("- One:"));
        assert!(prompt.contains("A schema talk."));
        assert!(prompt.contains("user: 10 min"));
        assert!(prompt.contains("candidate 2 of 3"));
        assert!(prompt.contains("different outline"));
        assert!(prompt.contains("scenario: Finance"));
        assert!(prompt.contains("between 10 and 15 screens"));
        assert!(!prompt.contains("Write a preview"));
        let preview = candidate_prompt(&CandidateRequest {
            preview_screens: Some(3),
            ..request
        });
        assert!(preview.contains("only the first 3 screens"));
        assert!(preview.contains("between 10 and 15 titles"));
        assert!(!preview.contains("between 10 and 15 screens"));
        let input = planner_input(&brief, 3);
        assert!(input.contains("Variety: high"));
        assert!(input.contains("Length in screens: 10-15"));
        assert!(input.contains("Design open in the editor: talk-candidate-1"));
        assert!(input.contains("user (editing talk-candidate-1): 10 min"));
        let edit = edit_prompt(&brief, "{}");
        assert!(edit.contains("Apply this request to the design: 10 min"));
        assert!(edit.contains("JSON patch"));
        assert!(edit.contains("node 0/1"));
        assert_eq!(
            super::prompt_words("A long talk about many things"),
            "A long talk about"
        );
        assert!(input.contains("Scenario: Finance"));
        assert!(input.contains("Candidates on the canvas: 3"));
        assert!(input.contains("assistant: How long?"));
    }

    #[test]
    fn plans_ask_for_count_and_variety_before_they_generate() {
        let ready = || Plan {
            should_generate: true,
            ..Plan::default()
        };
        let brief = Brief {
            prompt: "A talk.".to_owned(),
            ..Brief::default()
        };
        // Nothing chosen: all four questions, no generation.
        let asked = with_candidate_questions(ready(), &brief);
        assert!(!asked.should_generate);
        assert_eq!(asked.questions.len(), 4);
        assert!(crate::candidate_questions::is_scenario_question(
            &asked.questions[0].question
        ));
        assert!(crate::candidate_questions::is_length_question(
            &asked.questions[1].question
        ));
        assert!(crate::candidate_questions::is_variation_question(
            &asked.questions[2].question
        ));
        assert!(crate::candidate_questions::is_variety_question(
            &asked.questions[3].question
        ));
        // A chat-only reply asks nothing.
        let chat = with_candidate_questions(Plan::default(), &brief);
        assert!(chat.questions.is_empty());
        // A scenario, length, and count without a variety asks only for
        // the variety.
        let counted = Brief {
            scenario: Some("Business".to_owned()),
            length: Some("any".to_owned()),
            variations: Some(3),
            ..brief.clone()
        };
        let asked = with_candidate_questions(ready(), &counted);
        assert_eq!(asked.questions.len(), 1);
        assert!(crate::candidate_questions::is_variety_question(
            &asked.questions[0].question
        ));
        // Both chosen, or a single candidate, generates.
        let chosen = Brief {
            variety: Some("low".to_owned()),
            ..counted
        };
        assert!(with_candidate_questions(ready(), &chosen).should_generate);
        let single = Brief {
            scenario: Some("general".to_owned()),
            length: Some("5-8".to_owned()),
            variations: Some(1),
            ..brief
        };
        assert!(with_candidate_questions(ready(), &single).should_generate);
    }

    fn preview_design() -> design_model::Design {
        let mut design: design_model::Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        design.outline = vec![
            "Swift Design".to_owned(),
            "How agents build designs".to_owned(),
            "Results".to_owned(),
            "Roadmap".to_owned(),
            "Thanks".to_owned(),
        ];
        design
    }

    #[test]
    fn preview_notes_ask_for_a_few_screens_and_the_full_outline() {
        let note = super::preview_note(3, Some((10, 15)));
        assert!(note.contains("only the first 3 screens"));
        assert!(note.contains("between 10 and 15 titles"));
        assert!(super::preview_note(3, Some((12, 12))).contains("12 titles"));
        assert!(super::preview_note(3, None).contains("every screen title of the complete design"));
    }

    #[test]
    fn continue_prompts_list_the_remaining_titles_and_ask_for_inserts() {
        let design = preview_design();
        let brief = Brief {
            prompt: "A schema talk.".to_owned(),
            effort: "high".to_owned(),
            scenario: Some("Finance".to_owned()),
            messages: vec![ChatMessage::user("Use British spelling.", None)],
            ..Brief::default()
        };
        let whole = super::ContinueChunk { first: 3, count: 2 };
        let prompt = super::continue_prompt(&brief, &design, "{design}", whole);
        assert!(prompt.contains("its first 3 screens"));
        assert!(prompt.contains("Write 2 screens: outline titles 4 to 5 of 5"));
        assert!(prompt.contains("4. Roadmap\n5. Thanks"));
        assert!(!prompt.contains("1. Swift Design"));
        assert!(!prompt.contains("Other requests write"));
        assert!(prompt.contains("user: Use British spelling."));
        assert!(prompt.contains("scenario: Finance"));
        assert!(prompt.contains("thorough presenter notes"));
        assert!(prompt.contains("\"index\":3,\"insert\":true"));
        let second = super::ContinueChunk { first: 4, count: 1 };
        let chunk = super::continue_prompt(&brief, &design, "{design}", second);
        assert!(chunk.contains("Write 1 screens: outline titles 5 to 5 of 5"));
        assert!(chunk.contains("5. Thanks"));
        assert!(!chunk.contains("4. Roadmap"));
        assert!(chunk.contains("Other requests write"));
        assert_eq!(
            super::continue_chunks(3, 8),
            vec![
                super::ContinueChunk { first: 3, count: 3 },
                super::ContinueChunk { first: 6, count: 2 },
            ]
        );
        assert!(super::continue_chunks(5, 5).is_empty());
    }

    #[test]
    fn a_later_chunk_shows_over_placeholders_for_the_chunks_before_it() {
        let mut preview: design_model::Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        preview.screens.truncate(1);
        preview.outline = (1..=7).map(|number| format!("Title {number}")).collect();
        let chunks = super::continue_chunks(1, 7);
        assert_eq!(chunks.len(), 2);
        let written = preview.screens[0].clone();

        // Nothing written yet: the preview shows as it is.
        let empty: Vec<Vec<design_model::Screen>> = vec![Vec::new(), Vec::new()];
        assert_eq!(
            super::shown_design(&preview, &chunks, &empty).screens.len(),
            1
        );

        // The second chunk lands first. The first chunk's three titles
        // become placeholders, so the real screen sits at index 4.
        let board = vec![Vec::new(), vec![written.clone()]];
        let shown = super::shown_design(&preview, &chunks, &board);
        assert_eq!(shown.screens.len(), 5);
        assert!(shown.validate().is_empty());
        let pending: Vec<bool> = shown
            .screens
            .iter()
            .map(crate::designs::is_pending_screen)
            .collect();
        assert_eq!(pending, vec![false, true, true, true, false]);
        assert!(shown.screens[1].html.contains("Title 2"));
        // The design stays a preview, so a stopped run leaves it continuable.
        assert!(shown.is_preview());

        // Nothing is padded past the last chunk that has screens.
        let head_only = vec![vec![written], Vec::new()];
        assert_eq!(
            super::shown_design(&preview, &chunks, &head_only)
                .screens
                .len(),
            2
        );
    }

    #[test]
    fn a_template_prompt_carries_the_theme_and_the_example_screens() {
        let design: design_model::Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        let template = crate::templates::Template {
            id: "midnight-finance-1".to_owned(),
            name: "Midnight Finance".to_owned(),
            saved_at: "2026-01-01T00:00:00Z".to_owned(),
            source_design: "talk".to_owned(),
            theme: design.theme.clone(),
            screens: design.screens[..2].to_vec(),
        };
        let brief = Brief {
            prompt: "A talk".to_owned(),
            ..Brief::default()
        };
        let request = super::CandidateRequest {
            brief: &brief,
            candidate_number: 1,
            concepts: &[],
            preview_screens: None,
            design_id: "talk-candidate-1".to_owned(),
            template: Some(&template),
        };
        let prompt = super::candidate_prompt(&request);
        assert!(prompt.contains("Midnight Finance"));
        assert!(prompt.contains(&design.theme.colors.accent));
        assert!(prompt.contains("Template screen 1:"));
        assert!(prompt.contains("Template screen 2:"));
        assert!(!prompt.contains("Template screen 3:"));
        assert!(prompt.contains("Do not copy their text."));

        // Without a template the prompt says nothing about one.
        let plain = super::candidate_prompt(&super::CandidateRequest {
            template: None,
            ..request
        });
        assert!(!plain.contains("Template screen"));
    }

    #[test]
    fn writing_requests_reason_one_level_under_the_brief_effort() {
        assert_eq!(super::writing_effort("low"), "minimal");
        assert_eq!(super::writing_effort("medium"), "low");
        assert_eq!(super::writing_effort("high"), "medium");
    }

    #[test]
    fn partial_designs_grow_screen_by_screen_while_the_reply_streams() {
        let full = include_str!("../../../fixtures/sample-design.json");
        let (array_start, items) = super::complete_array_items(full, "screens").unwrap();
        assert_eq!(&full[array_start..=array_start], "[");
        assert_eq!(items.len(), 3);
        // Cut inside the second screen: one complete screen so far.
        let second_start = full.find(items[1]).unwrap();
        let cut = &full[..second_start + items[1].len() / 2];
        let partial = super::partial_design(cut).unwrap();
        assert_eq!(partial.screens.len(), 1);
        assert_eq!(partial.title, "Swift Design Overview");
        assert!(partial.validate().is_empty());
        // Before the first screen closes there is nothing to show.
        let early = &full[..second_start - items[0].len() / 2 - 1];
        assert!(super::partial_design(early).is_none());
        assert!(super::partial_design("Sure, here").is_none());
        // Braces inside strings do not end a screen.
        let tricky = "{\"screens\":[{\"html\":\"<p>}</p>\",\"css\":\"a{}\"},{\"html\":\"x";
        let (_, items) = super::complete_array_items(tricky, "screens").unwrap();
        assert_eq!(items, vec!["{\"html\":\"<p>}</p>\",\"css\":\"a{}\"}"]);
        // Continuation replies: complete inserts only.
        let screen = serde_json::to_string(&partial.screens[0]).unwrap();
        let streaming = format!(
            "{{\"screens\":[{{\"index\":3,\"insert\":true,\"screen\":{screen}}},{{\"index\":3,\"insert\":true,\"screen\":{{\"html\":\"<h1>"
        );
        assert_eq!(super::partial_continuation_screens(3, &streaming).len(), 1);
        assert!(super::partial_continuation_screens(3, "{\"screens\":[").is_empty());
    }

    #[test]
    fn stream_lines_collect_text_and_usage_per_wire_format() {
        let mut state = super::StreamState::default();
        assert!(super::stream_line(
            super::WireFormat::OpenAiChat,
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}",
            &mut state
        ));
        assert!(!super::stream_line(
            super::WireFormat::OpenAiChat,
            "data: {\"choices\":[{\"delta\":{}}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}",
            &mut state
        ));
        assert!(!super::stream_line(
            super::WireFormat::OpenAiChat,
            "data: [DONE]",
            &mut state
        ));
        assert_eq!(state.collected, "Hel");
        assert_eq!(state.usage.unwrap().input_tokens, 9);
        assert!(state.saw_event);
        let mut anthropic = super::StreamState::default();
        super::stream_line(
            super::WireFormat::AnthropicMessages,
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":30,\"output_tokens\":1}}}",
            &mut anthropic,
        );
        assert!(super::stream_line(
            super::WireFormat::AnthropicMessages,
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"{\\\"a\\\"\"}}",
            &mut anthropic,
        ));
        super::stream_line(
            super::WireFormat::AnthropicMessages,
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
        let mut plain = super::StreamState::default();
        assert!(!super::stream_line(
            super::WireFormat::OpenAiChat,
            "{\"choices\":[]}",
            &mut plain
        ));
        assert!(!plain.saw_event);
    }

    #[test]
    fn continue_summaries_name_each_design_and_skip_complete_ones() {
        let outcomes = vec![
            ("a".to_owned(), Ok(0)),
            ("b".to_owned(), Ok(4)),
            ("c".to_owned(), Err("timed out".to_owned())),
        ];
        let summary = super::continue_summary(&outcomes);
        assert!(!summary.contains("`a`"));
        assert!(summary.contains("I wrote the remaining 4 screens of `b`."));
        assert!(summary.contains("I could not continue `c`: timed out."));
        assert!(summary.ends_with("continue another candidate."));
        let complete = super::continue_summary(&[("a".to_owned(), Ok(0))]);
        assert!(complete.starts_with("`a` is complete already."));
    }

    #[test]
    fn continuations_append_new_screens_and_keep_the_outline_until_complete() {
        let design = preview_design();
        let screen = serde_json::to_string(&design.screens[2]).unwrap();
        // A short reply is kept; the design stays a preview.
        let short =
            format!("{{\"screens\":[{{\"index\":3,\"insert\":true,\"screen\":{screen}}}]}}");
        let partial = super::apply_continuation(&design, &short).unwrap();
        assert_eq!(partial.screens.len(), 4);
        assert!(partial.is_preview());
        // Indexes may count up; order is kept.
        let mut numbered = design.screens[2].clone();
        numbered.notes = Some("last".to_owned());
        let numbered_json = serde_json::to_string(&numbered).unwrap();
        let full = format!(
            "Here: {{\"screens\":[{{\"index\":3,\"insert\":true,\"screen\":{screen}}},\
             {{\"index\":4,\"screen\":{numbered_json}}}]}}"
        );
        let continued = super::apply_continuation(&design, &full).unwrap();
        assert_eq!(continued.screens.len(), 5);
        assert!(continued.outline.is_empty());
        assert!(!continued.is_preview());
        assert_eq!(continued.screens[0], design.screens[0]);
        assert_eq!(continued.screens[4].notes.as_deref(), Some("last"));
        // Existing screens are never replaced; a reply with nothing new fails.
        let replacing = format!("{{\"screens\":[{{\"index\":0,\"screen\":{screen}}}]}}");
        assert!(
            super::apply_continuation(&design, &replacing)
                .unwrap_err()
                .contains("adds no screens")
        );
        // A whole design reply contributes its screens past the existing ones.
        let mut whole = design.clone();
        whole.screens.push(numbered.clone());
        whole.screens.push(numbered);
        let whole_json = serde_json::to_string(&whole).unwrap();
        let from_design = super::apply_continuation(&design, &whole_json).unwrap();
        assert_eq!(from_design.screens.len(), 5);
        assert!(!from_design.is_preview());
        assert!(super::apply_continuation(&design, "no json").is_err());
    }

    #[test]
    fn the_planner_is_told_not_to_ask_the_app_questions() {
        let prompt = super::planner_prompt();
        assert!(prompt.contains("Never ask these"));
        assert!(prompt.contains("number of candidates"));
        assert!(prompt.contains("design length"));
    }

    #[test]
    fn image_parts_convert_to_every_wire_format() {
        let content = user_content_with_images("Review.", &[vec![1, 2, 3]]);
        let parts = content.as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert!(
            parts[2]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,AQID")
        );
        let messages = vec![
            serde_json::json!({ "role": "system", "content": "sys" }),
            serde_json::json!({ "role": "user", "content": content }),
        ];
        let anthropic = anthropic_messages_body("claude", &messages);
        let blocks = anthropic["messages"][0]["content"].as_array().unwrap();
        assert_eq!(blocks[2]["type"], "image");
        assert_eq!(blocks[2]["source"]["media_type"], "image/png");
        assert_eq!(blocks[2]["source"]["data"], "AQID");
        let chatgpt = chatgpt_responses_body("gpt-5", &messages, "low");
        let items = chatgpt["input"][0]["content"].as_array().unwrap();
        assert_eq!(items[0]["type"], "input_text");
        assert_eq!(items[2]["type"], "input_image");
        // Plain strings stay plain.
        assert_eq!(user_content_with_images("x", &[]), "x");
        let plain = anthropic_messages_body(
            "claude",
            &[serde_json::json!({ "role": "user", "content": "hi" })],
        );
        assert_eq!(plain["messages"][0]["content"], "hi");
    }

    #[test]
    fn context_windows_follow_the_model_family() {
        assert_eq!(context_window("gemini-2.5-pro"), 1_048_576);
        assert_eq!(context_window("gpt-5-mini"), 400_000);
        assert_eq!(context_window("claude-sonnet-5"), 200_000);
        assert_eq!(context_window("llama3.1"), 128_000);
    }

    #[test]
    fn plans_parse_json_and_fall_back_to_prose() {
        let plan = parse_plan(
            "{\"reply\":\"Two questions first.\",\
             \"questions\":[{\"question\":\"How long?\",\"options\":[\"5 min\",\"20 min\"]}],\
             \"generate\":false}",
        );
        assert_eq!(plan.reply, "Two questions first.");
        assert_eq!(plan.questions.len(), 1);
        assert!(!plan.should_generate);
        let ready = parse_plan("{\"reply\":\"Writing now.\",\"generate\":true}");
        assert!(ready.should_generate);
        assert!(!ready.should_edit);
        let editing = parse_plan("{\"reply\":\"Changing the title.\",\"edit\":true}");
        assert!(editing.should_edit);
        assert!(ready.questions.is_empty());
        let prose = parse_plan("Sure, what audience?");
        assert_eq!(prose.reply, "Sure, what audience?");
        assert!(!prose.should_generate);
    }

    #[test]
    fn candidates_take_one_template_look_each_and_wrap() {
        let looks: Vec<crate::templates::Template> = ["warm", "cool", "mono"]
            .iter()
            .map(|name| {
                serde_json::from_value(serde_json::json!({
                    "id": name,
                    "name": name,
                    "saved_at": "",
                    "source_design": "",
                    "theme": {
                        "name": name,
                        "colors": {
                            "background": "#ffffff",
                            "text": "#000000",
                            "accent": "#0e6e63",
                            "muted": "#6c7178",
                        },
                        "fonts": { "heading": "Inter", "body": "Inter", "mono": "JetBrains Mono" },
                    },
                    "screens": [],
                }))
                .unwrap()
            })
            .collect();
        let chosen: Vec<String> = (1..=5)
            .map(|number| candidate_template(&looks, number).unwrap().id)
            .collect();
        assert_eq!(chosen, ["warm", "cool", "mono", "warm", "cool"]);
    }

    #[test]
    fn no_templates_leaves_every_candidate_without_a_look() {
        assert!(candidate_template(&[], 1).is_none());
        assert!(candidate_template(&[], 7).is_none());
    }
}
