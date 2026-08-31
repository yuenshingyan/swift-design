//! The built-in generation engine.
//!
//! Talks to the model through `model_client`, with the user's own keys
//! and only when a run starts. The loop: read the brief, ask the model
//! for each candidate design, validate, feed every validation error
//! back for a fix round, and save the result. The studio watches it all
//! through `/events`.

use std::sync::Arc;

use design_model::{BriefQuestionSet, Design, QuestionSetError, validate_question_set};

use crate::candidates::{candidate_id, next_candidate_number};
use crate::concepts::{Concept, concept_input, concept_note, concept_prompt, parse_concepts};
use crate::designs::DesignStore;
use crate::edit_focus::{
    EditFix, EditInput, EditOrder, MergeInput, findings_for, findings_note, fix_instruction,
    focus_note, fresh_note, merge_note, merge_sources, referenced_indexes, touched_indexes,
};
use crate::events::ChangeNotifier;
use crate::instructions::DEMO_RULES;
use crate::model_client::{LogSink, ModelClient, ModelConfiguration, TextSink, UsageSink};
use crate::planner::{
    ANSWERED_QUESTION_LIMIT, app_question_set, parse_plan, planner_input, planner_prompt,
};
use crate::request::{SessionRequest, answered_questions_from_answers, request_input};
use crate::sessions::session_id_of_artifact;
use crate::sessions::{ChatMessage, RunMode, RunOptions, RunRecord, SessionStore};

/// Fix rounds per candidate before giving up, by effort level.
pub(crate) fn fix_round_limit(effort: &str) -> usize {
    match effort {
        "low" => 2,
        "high" => 4,
        _ => 3,
    }
}

/// Screens a preview candidate writes before the rest are continued.
pub(crate) const PREVIEW_SCREEN_COUNT: usize = 3;

/// What a generation run did.
#[derive(Debug)]
pub enum GenerationOutcome {
    /// The run wrote these design ids.
    Wrote {
        /// The design ids written, in candidate order.
        design_ids: Vec<String>,
    },
    /// The run needs the user to answer a blocking question set. The
    /// engine has written the set and moved the session to clarifying.
    NeedsClarification {
        /// The set number.
        question_set: u32,
    },
    /// The planner answered in the chat and wrote nothing.
    Replied,
}

/// What the model must do this run.
pub(crate) enum GenerationTask {
    /// Write the requested candidates.
    Candidates,
    /// Apply the user's latest request to each artifact named.
    Edit {
        /// The artifact ids to edit: the candidates the user pinned,
        /// the artifact open in the editor, or the chosen one.
        designs: Vec<String>,
        /// What to change, in the user's words.
        instruction: String,
    },
    /// Continue the preview designs named here.
    Continue(Vec<String>),
    /// Combine parts of the named candidates into one new candidate.
    Merge {
        /// The candidates to take parts from, in the order pinned.
        sources: Vec<String>,
        /// Which parts to take from each, in the user's words.
        instruction: String,
    },
    /// Write the units the instruction names anew, in one artifact.
    Regenerate {
        /// The artifact whose units are rewritten.
        design: String,
        /// The request, with the `[screen N]` or `[slide N]` references.
        instruction: String,
    },
}

/// The request, the answers, and the options one run works from.
#[derive(Clone)]
pub(crate) struct GenerationContext {
    pub(crate) request: SessionRequest,
    pub(crate) options: RunOptions,
    pub(crate) session_id: String,
}

impl GenerationContext {
    /// The effort level for this run.
    pub(crate) fn effort(&self) -> &str {
        &self.options.effort
    }

    /// The preview screen count, or `None` for complete candidates.
    pub(crate) fn preview_screens(&self) -> Option<usize> {
        self.options.preview.then_some(PREVIEW_SCREEN_COUNT)
    }
}

/// Why a generation run stopped without writing a design.
pub(crate) enum GenerationStop {
    /// The run failed with this message.
    Failed(String),
    /// The model asked for a blocking detail. The engine writes the set
    /// and returns the session to clarifying.
    NeedsClarification(BriefQuestionSet),
}

/// What the planner decided for this turn.
enum PlanStep {
    /// Write or edit: run this task.
    Task(GenerationTask),
    /// A question set was written; its number.
    Asked(u32),
    /// A chat reply only.
    Replied,
}

/// The artifacts the latest user turn is about: the candidates it
/// pinned, else the artifact it was sent from in the editor, else the
/// chosen one. Empty when it names none.
fn edit_targets(messages: &[ChatMessage], chosen: Option<&str>) -> Vec<String> {
    let latest = messages.iter().rev().find(|message| message.role == "user");
    if let Some(message) = latest {
        if !message.pinned.is_empty() {
            return message.pinned.clone();
        }
        if let Some(design) = &message.design {
            return vec![design.clone()];
        }
    }
    chosen.map(str::to_owned).into_iter().collect()
}

/// How many questions the user answered since the last turn that
/// wrote artifacts: the answers that belong to the current request.
/// Every answer counts while nothing was written yet.
pub(crate) fn answers_since_last_write(
    messages: &[ChatMessage],
    records: &[crate::sessions::AnswerRecord],
) -> usize {
    let last_write = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.artifacts.is_empty())
        .and_then(|message| message.at.clone());
    records
        .iter()
        .filter(|record| {
            last_write
                .as_deref()
                .is_none_or(|written| record.at.as_str() > written)
        })
        .map(|record| record.answers.len())
        .sum()
}

/// The text of the latest user turn.
fn latest_user_text(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
}

impl From<String> for GenerationStop {
    fn from(message: String) -> Self {
        GenerationStop::Failed(message)
    }
}

/// The built-in engine: the model client plus the stores it writes.
#[derive(Clone)]
pub struct GenerationEngine {
    pub(crate) model: ModelClient,
    pub(crate) designs: DesignStore,
    /// The deck store, for deck sessions. `None` refuses deck runs.
    pub(crate) decks: Option<crate::decks::DeckStore>,
    /// The document store, for document sessions. `None` refuses
    /// document runs.
    pub(crate) documents: Option<crate::documents::DocumentStore>,
    /// The social store, for social sessions. `None` refuses social
    /// runs.
    pub(crate) socials: Option<crate::socials::SocialStore>,
    /// The print store, for print sessions. `None` refuses print runs.
    pub(crate) prints: Option<crate::prints::PrintStore>,
    pub(crate) sessions: SessionStore,
    address: String,
    pub(crate) notifier: ChangeNotifier,
    progress_sink: Option<ProgressSink>,
    design_progress_sink: Option<DesignProgressSink>,
    templates: Option<crate::templates::TemplateStore>,
    uploads: Option<crate::uploads::UploadStore>,
}

/// A sink for how far the current turn is, 0 to 100, shared with the
/// run status. A turn starts at 0.
pub type ProgressSink = Arc<dyn Fn(u8) + Send + Sync>;

/// A sink for how far one design the turn writes is, 0 to 100, by design
/// id: a candidate being written, a design being continued or edited.
pub type DesignProgressSink = Arc<dyn Fn(&str, u8) + Send + Sync>;

/// One item's share of a turn's progress, 0.0 to 1.0. Several items
/// (candidates, continued designs) each report their own share; the mean
/// goes to the `ProgressSink`.
pub(crate) type ShareSink = Arc<dyn Fn(f32) + Send + Sync>;

/// What every continue of one turn shares.
struct ContinueShared<'a> {
    /// The HTTP client.
    client: &'a reqwest::Client,
    /// The run's context.
    context: &'a GenerationContext,
    /// The user's source files, loaded once.
    attachments: Arc<Attachments>,
    /// The progress group the continues report into.
    group: ProgressGroup,
    /// The run log.
    log: &'a LogSink,
}

/// The continues in flight and the ids started so far.
#[derive(Default)]
struct ContinueBatch {
    /// The running continues.
    tasks: tokio::task::JoinSet<(String, Result<usize, String>)>,
    /// Every id started, in order.
    started: Vec<String>,
}

/// The progress of a set of artifacts that grows while the run works.
pub(crate) struct ProgressGroup {
    /// One fraction per share, in start order.
    shares: Arc<std::sync::Mutex<Vec<f32>>>,
    /// The run's progress sink.
    sink: Option<ProgressSink>,
    /// The per-artifact progress sink.
    design_sink: Option<DesignProgressSink>,
    /// The percentage the group starts from.
    base: u8,
    /// The percentage span the group fills.
    span: u8,
}

impl ProgressGroup {
    /// Adds an artifact and returns its share.
    pub(crate) fn share(&self, design_id: &str) -> ShareSink {
        let index = match self.shares.lock() {
            Ok(mut shares) => {
                shares.push(0.0);
                shares.len() - 1
            }
            Err(_) => 0,
        };
        let shares = Arc::clone(&self.shares);
        let sink = self.sink.clone();
        let design_sink = self.design_sink.clone();
        let design_id = design_id.to_owned();
        let (base, span) = (self.base, self.span);
        Arc::new(move |fraction: f32| {
            let fraction = fraction.clamp(0.0, 1.0);
            if let Some(design_sink) = &design_sink {
                design_sink(&design_id, (fraction * 100.0) as u8);
            }
            let Ok(mut shares) = shares.lock() else {
                return;
            };
            if let Some(slot) = shares.get_mut(index) {
                *slot = fraction;
            }
            let mean = shares.iter().sum::<f32>() / shares.len().max(1) as f32;
            if let Some(sink) = &sink {
                sink(base.saturating_add((f32::from(span) * mean) as u8));
            }
        })
    }
}

/// The screens each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type ChunkBoard = Arc<std::sync::Mutex<Vec<Vec<design_model::Screen>>>>;

/// The share of a design request that the first valid draft completes;
/// the polish rounds fill the rest.
pub(crate) const DRAFT_SHARE: f32 = 0.6;

/// Screens the engine asks for in one continuation request. Small
/// chunks keep each reply short, and the design grows on the canvas
/// after every chunk.
pub(crate) const CONTINUE_CHUNK_SCREENS: usize = 3;

/// The share of a continuation that writing the screens completes; the
/// polish rounds fill the rest.
pub(crate) const CONTINUE_DRAFT_SHARE: f32 = 0.85;

/// The artifact ids of the continue requests at the end of `messages`,
/// newest last, without repeats.
///
/// The walk stops at the first user message that is not a continue: a
/// request from an earlier turn is that turn's business. Pressing Finish
/// on three candidates in a row therefore continues all three.
pub(crate) fn trailing_continue_ids(messages: &[ChatMessage]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for message in messages.iter().rev() {
        if message.role != "user" {
            continue;
        }
        if !message.is_continue {
            break;
        }
        if let Some(id) = &message.design
            && !ids.contains(id)
        {
            ids.push(id.clone());
        }
    }
    ids.reverse();
    ids
}

/// The artifact and the instruction of a regenerate request, when the
/// latest user turn is one.
fn trailing_regenerate(messages: &[ChatMessage]) -> Option<(String, String)> {
    let latest = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")?;
    if !latest.is_regenerate {
        return None;
    }
    let design = latest.design.clone()?;
    Some((design, latest.content.clone()))
}

impl GenerationEngine {
    /// Creates an engine over the given stores. `settings` enables
    /// login-token refresh for Claude logins.
    pub fn new(
        configuration: ModelConfiguration,
        designs: DesignStore,
        sessions: SessionStore,
        settings: Option<crate::settings::SettingsStore>,
        address: String,
        notifier: ChangeNotifier,
    ) -> Self {
        Self {
            model: ModelClient::new(configuration, settings),
            designs,
            decks: None,
            documents: None,
            socials: None,
            prints: None,
            sessions,
            address,
            notifier,
            progress_sink: None,
            design_progress_sink: None,
            templates: None,
            uploads: None,
        }
    }

    /// Lets the engine write decks, for deck sessions.
    pub fn with_decks(mut self, decks: crate::decks::DeckStore) -> Self {
        self.decks = Some(decks);
        self
    }

    /// Lets the engine write documents, for document sessions.
    pub fn with_documents(mut self, documents: crate::documents::DocumentStore) -> Self {
        self.documents = Some(documents);
        self
    }

    /// Lets the engine write socials, for social sessions.
    pub fn with_socials(mut self, socials: crate::socials::SocialStore) -> Self {
        self.socials = Some(socials);
        self
    }

    /// Lets the engine write prints, for print sessions.
    pub fn with_prints(mut self, prints: crate::prints::PrintStore) -> Self {
        self.prints = Some(prints);
        self
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

    /// The uploads to attach to a content request: the files of
    /// `session_id` under the size caps. Empty without an upload store.
    /// A file that cannot be read is logged and skipped.
    ///
    /// The session decides the list. A file attached to one project must
    /// never reach another project's prompt.
    pub(crate) async fn load_attachments(&self, session_id: &str, log: &LogSink) -> Attachments {
        let mut attachments = Attachments::default();
        let Some(uploads) = &self.uploads else {
            return attachments;
        };
        let summaries = match uploads.list(session_id).await {
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
    pub(crate) fn user_message(&self, text: &str, attachments: &Attachments) -> serde_json::Value {
        let can_see_images = crate::screenshots::supports_vision(self.model.model());
        serde_json::json!({
            "role": "user",
            "content": user_content_with_attachments(text, attachments, can_see_images),
        })
    }

    /// The templates the options name, in order. A template that was
    /// deleted is skipped, so the run still writes the rest.
    pub(crate) async fn brief_templates(
        &self,
        options: &RunOptions,
        log: &LogSink,
    ) -> Vec<crate::templates::Template> {
        let ids = options.templates.clone();
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
        self.model.refresh_login_if_needed(log).await
    }

    /// Short label for the studio: `google/gemini-2.5-flash`.
    pub fn label(&self) -> String {
        self.model.label()
    }

    /// The context window of the configured model, in tokens.
    pub fn context_window(&self) -> u64 {
        self.model.context_window()
    }

    /// Runs one turn for `session_id`, as Swift Deck did: a continue
    /// request continues; otherwise the planner reads the request, the
    /// answers, and the conversation, then asks, writes, edits, or
    /// replies. The session may be in any state but error.
    pub async fn run(
        mut self,
        session_id: &str,
        log: LogSink,
    ) -> Result<GenerationOutcome, String> {
        self.refresh_login_if_needed(&log).await?;
        let session = self
            .sessions
            .read(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no session `{session_id}`"))?;
        if session.state.is_halted() {
            return Err(format!(
                "the session is `{}`: resume it first",
                session.state
            ));
        }
        let answered = self
            .sessions
            .answered_questions(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let context = GenerationContext {
            request: SessionRequest {
                request: session.request.clone(),
                kind: session.artifact_kind,
                answers: answered_questions_from_answers(&answered),
                options: session.options.clone(),
            },
            options: session.options.clone(),
            session_id: session_id.to_owned(),
        };
        let run_id = self
            .sessions
            .start_run(
                session_id,
                RunRecord {
                    run_id: String::new(),
                    mode: RunMode::Generation,
                    runtime: "built-in".to_owned(),
                    provider: None,
                    model: Some(self.model.model().to_owned()),
                    started_at: crate::time::rfc3339_now(),
                    finished_at: None,
                    result: None,
                    error: None,
                    artifacts: Vec::new(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        log(&format!(
            "turn · {} · effort {}",
            self.label(),
            context.effort(),
        ));
        let client = ModelClient::build_http_client()?;
        // A session already generating asked to write now: no planner.
        let write_now = session.state == design_model::WorkflowState::Generating;
        let task = match self.pick_task(&context).await? {
            Some(task) => task,
            None if write_now => GenerationTask::Candidates,
            None => match self.plan_turn(&client, &session, &context, &log).await {
                Ok(PlanStep::Task(task)) => task,
                Ok(PlanStep::Asked(question_set)) => {
                    self.finish_run(session_id, &run_id, "asked_questions", None, Vec::new())
                        .await;
                    return Ok(GenerationOutcome::NeedsClarification { question_set });
                }
                Ok(PlanStep::Replied) => {
                    self.finish_run(session_id, &run_id, "replied", None, Vec::new())
                        .await;
                    return Ok(GenerationOutcome::Replied);
                }
                Err(message) => {
                    return self
                        .settle(
                            session_id,
                            &run_id,
                            Err(GenerationStop::Failed(message)),
                            &log,
                        )
                        .await;
                }
            },
        };
        if session.state != design_model::WorkflowState::Generating {
            self.sessions
                .apply(session_id, design_model::WorkflowEvent::GenerationStarted)
                .await
                .map_err(|error| error.to_string())?;
            self.notifier.notify();
        }
        self.report_progress(0);
        let outcome = self.execute(&client, &context, task, &log).await;
        let outcome = self
            .run_late_continues(&client, &context, outcome, &log)
            .await;
        self.report_progress(100);
        self.settle(session_id, &run_id, outcome, &log).await
    }

    /// Runs the continue requests that arrived while this run worked and
    /// folds what they wrote into `outcome`.
    ///
    /// Pressing Finish on a second candidate appends a request the
    /// running turn has already read past. Without this pass it would
    /// wait for a turn that nothing starts.
    ///
    /// Each id is tried once. A continue that fails leaves a preview
    /// behind, and trying it again would never end the run.
    async fn run_late_continues(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        outcome: Result<GenerationOutcome, GenerationStop>,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let Ok(GenerationOutcome::Wrote { mut design_ids }) = outcome else {
            return outcome;
        };
        let mut tried = design_ids.clone();
        loop {
            let pending = match self.pick_task(context).await {
                Ok(Some(GenerationTask::Continue(ids))) => ids,
                _ => return Ok(GenerationOutcome::Wrote { design_ids }),
            };
            let fresh: Vec<String> = pending
                .into_iter()
                .filter(|id| !tried.contains(id))
                .collect();
            if fresh.is_empty() {
                return Ok(GenerationOutcome::Wrote { design_ids });
            }
            log(&format!(
                "continuing {} more: {}",
                fresh.len(),
                fresh.join(", ")
            ));
            tried.extend(fresh.clone());
            let task = GenerationTask::Continue(fresh);
            match self.execute(client, context, task, log).await {
                Ok(GenerationOutcome::Wrote { design_ids: more }) => design_ids.extend(more),
                Ok(_) => {}
                Err(stop) => {
                    // The first task wrote, so the run is not a failure.
                    let reason = match &stop {
                        GenerationStop::Failed(message) => message.clone(),
                        GenerationStop::NeedsClarification(_) => "it asked a question".to_owned(),
                    };
                    log(&format!("a later continue stopped: {reason}"));
                    return Ok(GenerationOutcome::Wrote { design_ids });
                }
            }
        }
    }

    /// A continue or a regenerate request names its artifacts; no
    /// planning is needed.
    async fn pick_task(
        &self,
        context: &GenerationContext,
    ) -> Result<Option<GenerationTask>, String> {
        let messages = self
            .sessions
            .messages(&context.session_id)
            .await
            .map_err(|error| error.to_string())?;
        if let Some((design, instruction)) = trailing_regenerate(&messages) {
            return Ok(Some(GenerationTask::Regenerate {
                design,
                instruction,
            }));
        }
        let continues = match context.request.kind {
            design_model::ArtifactKind::Demo => self.continue_requests(&context.session_id).await?,
            design_model::ArtifactKind::Deck => {
                self.continue_deck_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Document => {
                self.continue_document_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Social => {
                self.continue_social_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Print => {
                self.continue_print_requests(&context.session_id).await?
            }
        };
        Ok((!continues.is_empty()).then_some(GenerationTask::Continue(continues)))
    }

    /// Fills the blank app questions from what the planner read in the
    /// request, before the card opens. The card then shows them as
    /// picked and marked as suggested. Only the setup turn does this:
    /// a later turn has no card, so a suggestion would land unseen.
    async fn suggest_options(
        &self,
        context: &GenerationContext,
        suggestions: &[(String, String)],
        log: &LogSink,
    ) -> Result<(), String> {
        if suggestions.is_empty() {
            return Ok(());
        }
        let kind = context.request.kind;
        let mut filled = Vec::new();
        self.sessions
            .update(&context.session_id, |session| {
                filled = session.options.suggest(kind, suggestions);
            })
            .await
            .map_err(|error| error.to_string())?;
        if filled.is_empty() {
            return Ok(());
        }
        tracing::info!(
            session_id = %context.session_id,
            axes = ?filled,
            "app questions suggested from the request"
        );
        log(&format!("suggested {}", filled.join(", ")));
        self.notifier.notify();
        Ok(())
    }

    /// Asks the planner what this turn does: questions, a write, an
    /// edit of the open artifact, or a reply.
    async fn plan_turn(
        &self,
        client: &reqwest::Client,
        session: &crate::sessions::Session,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<PlanStep, String> {
        let session_id = context.session_id.as_str();
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let candidate_count = self.candidate_count(context).await;
        let targets = edit_targets(&messages, session.chosen_design.as_deref());
        // The planner reads the user's source files, as the writing
        // turns do. Planning blind is what produced questions the files
        // already answered.
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = planner_input(&context.request, &messages, candidate_count, &targets);
        let request = vec![
            serde_json::json!({ "role": "system", "content": planner_prompt(context.request.kind) }),
            self.user_message(&input, &attachments),
        ];
        log("planning the turn");
        let reply = self.model.chat(client, &request, context.effort()).await?;
        let plan = parse_plan(&reply);
        // The first turn always asks. The app owns a fixed list of
        // questions and asks it before anything is written; the planner
        // only adds to that card. Without this, a planner that asks
        // nothing writes a session the user never set up.
        let is_setup_turn = session.state == design_model::WorkflowState::Intake;
        if is_setup_turn {
            self.suggest_options(context, &plan.suggestions, log)
                .await?;
        }
        let asked = plan
            .question_set
            .clone()
            .or_else(|| is_setup_turn.then(app_question_set));
        // The limit is per request: answers given before the last write
        // belong to an earlier request, and a new change may need its own
        // questions.
        let records = self
            .sessions
            .answers(session_id)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(set) = &asked
            && answers_since_last_write(&messages, &records) < ANSWERED_QUESTION_LIMIT
        {
            let number = self
                .sessions
                .write_question_set(session_id, set)
                .await
                .map_err(|error| error.to_string())?;
            // The setup turn never writes, so a planner reply that
            // promises a write would contradict the card it opens.
            let is_promise_to_write = is_setup_turn && plan.should_generate;
            let spoken = match plan.reply.trim() {
                reply if !reply.is_empty() && !is_promise_to_write => reply.to_owned(),
                _ => set.message.clone(),
            };
            self.sessions
                .append_message(
                    session_id,
                    ChatMessage::assistant_questions(&spoken, number),
                )
                .await
                .map_err(|error| error.to_string())?;
            self.sessions
                .apply(session_id, design_model::WorkflowEvent::QuestionsAsked)
                .await
                .map_err(|error| error.to_string())?;
            self.notifier.notify();
            log(&format!("asked question set {number}"));
            return Ok(PlanStep::Asked(number));
        }
        if !plan.reply.is_empty() {
            self.say(session_id, &plan.reply).await?;
        }
        // A merge needs two sources. With one, the change is an edit.
        if plan.should_merge && targets.len() >= 2 {
            let instruction = latest_user_text(&messages).unwrap_or_default();
            return Ok(PlanStep::Task(GenerationTask::Merge {
                sources: targets,
                instruction,
            }));
        }
        if (plan.should_edit || plan.should_merge) && !targets.is_empty() {
            let instruction = latest_user_text(&messages).unwrap_or_default();
            return Ok(PlanStep::Task(GenerationTask::Edit {
                designs: targets,
                instruction,
            }));
        }
        // Past the question limit the planner writes instead of asking.
        if plan.should_generate || plan.question_set.is_some() {
            return Ok(PlanStep::Task(GenerationTask::Candidates));
        }
        log("replied; waiting for the user");
        Ok(PlanStep::Replied)
    }

    /// How many candidates of this session are on the canvas.
    async fn candidate_count(&self, context: &GenerationContext) -> usize {
        let session_id = context.session_id.as_str();
        match context.request.kind {
            design_model::ArtifactKind::Demo => self
                .designs
                .list()
                .await
                .map(|rows| {
                    rows.iter()
                        .filter(|row| session_id_of_artifact(&row.id) == session_id)
                        .count()
                })
                .unwrap_or(0),
            design_model::ArtifactKind::Deck => match &self.decks {
                Some(decks) => decks
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Document => match &self.documents {
                Some(documents) => documents
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Social => match &self.socials {
                Some(socials) => socials
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Print => match &self.prints {
                Some(prints) => prints
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
        }
    }

    /// The preview designs the latest user turn asked to continue:
    /// every design named by a trailing continue request that still is
    /// a preview. Pressing Finish on several candidates continues them
    /// all.
    async fn continue_requests(&self, session_id: &str) -> Result<Vec<String>, String> {
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for design_id in trailing_continue_ids(&messages) {
            // A design that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(design)) = self.designs.load(&design_id).await
                && design.is_preview()
            {
                previews.push(design_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task and returns the outcome.
    async fn execute(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        if context.request.kind == design_model::ArtifactKind::Deck {
            return self.execute_deck(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Document {
            return self.execute_document(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Social {
            return self.execute_social(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Print {
            return self.execute_print(client, context, task, log).await;
        }
        match task {
            GenerationTask::Candidates => self.generate_candidates(client, context, log).await,
            GenerationTask::Edit {
                designs,
                instruction,
            } => {
                let order = EditOrder {
                    artifact_ids: &designs,
                    instruction: &instruction,
                    is_fresh: false,
                };
                let design_ids = self.edit_designs(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Regenerate {
                design,
                instruction,
            } => {
                let order = EditOrder {
                    artifact_ids: std::slice::from_ref(&design),
                    instruction: &instruction,
                    is_fresh: true,
                };
                let design_ids = self.edit_designs(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let design_id = self
                    .merge_designs(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![design_id],
                })
            }
            GenerationTask::Continue(design_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, design_ids, log)
                    .await;
                if outcomes.iter().all(|(_, outcome)| outcome.is_err()) {
                    let failures: Vec<String> = outcomes
                        .iter()
                        .filter_map(|(id, outcome)| {
                            outcome.as_ref().err().map(|error| format!("{id}: {error}"))
                        })
                        .collect();
                    return Err(GenerationStop::Failed(failure_message(
                        &failures,
                        "no design was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Records the run outcome and updates the session. A written design
    /// is reported and the run finishes; a clarification writes the set
    /// and returns to clarifying; a failure is recorded and propagated.
    async fn settle(
        &self,
        session_id: &str,
        run_id: &str,
        outcome: Result<GenerationOutcome, GenerationStop>,
        log: &LogSink,
    ) -> Result<GenerationOutcome, String> {
        match outcome {
            // The engine returns clarification as an Err below; an Ok
            // clarification cannot arise, but the type allows it.
            Ok(GenerationOutcome::NeedsClarification { question_set }) => {
                Ok(GenerationOutcome::NeedsClarification { question_set })
            }
            Ok(GenerationOutcome::Replied) => {
                self.finish_run(session_id, run_id, "replied", None, Vec::new())
                    .await;
                Ok(GenerationOutcome::Replied)
            }
            Ok(GenerationOutcome::Wrote { design_ids }) => {
                let summary = wrote_summary(&design_ids);
                // The reply names what it wrote, so the studio can
                // revert the turn.
                self.sessions
                    .append_message(
                        session_id,
                        ChatMessage::assistant(&summary).with_artifacts(design_ids.clone()),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                self.notifier.notify();
                self.finish_run(session_id, run_id, "succeeded", None, design_ids.clone())
                    .await;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            Err(GenerationStop::NeedsClarification(set)) => {
                let number = self
                    .sessions
                    .write_question_set(session_id, &set)
                    .await
                    .map_err(|error| error.to_string())?;
                self.sessions
                    .append_message(
                        session_id,
                        ChatMessage::assistant_questions(&set.message, number),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                self.sessions
                    .apply(session_id, design_model::WorkflowEvent::QuestionsAsked)
                    .await
                    .map_err(|error| error.to_string())?;
                self.finish_run(session_id, run_id, "needs_clarification", None, Vec::new())
                    .await;
                log("generation needs clarification; returned to the questions");
                Ok(GenerationOutcome::NeedsClarification {
                    question_set: number,
                })
            }
            Err(GenerationStop::Failed(message)) => {
                self.finish_run(
                    session_id,
                    run_id,
                    "failed",
                    Some(message.clone()),
                    Vec::new(),
                )
                .await;
                Err(message)
            }
        }
    }

    /// Appends an assistant turn and wakes the studio.
    async fn say(&self, session_id: &str, content: &str) -> Result<(), String> {
        self.sessions
            .append_message(session_id, ChatMessage::assistant(content))
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }

    /// Records the end of the run and wakes the studio.
    async fn finish_run(
        &self,
        session_id: &str,
        run_id: &str,
        result: &str,
        error: Option<String>,
        artifacts: Vec<String>,
    ) {
        if let Err(failure) = self
            .sessions
            .finish_run(session_id, run_id, result, error, artifacts)
            .await
        {
            tracing::warn!(%failure, "recording the generation run failed");
        }
        self.notifier.notify();
    }

    /// Writes one design per requested variation. Returns the ids.
    async fn generate_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let count = context.options.variation_count();
        let attachments = Arc::new(self.load_attachments(&context.session_id, log).await);
        let concepts = if count > 1 {
            self.plan_concepts(client, context, count, &attachments, log)
                .await?
        } else {
            Vec::new()
        };
        self.report_progress(10);
        let base = context.session_id.clone();
        // One design per platform per variation: the same concept drawn
        // on each canvas the user picked. The studio groups them by
        // canvas, so the numbering runs straight through.
        // The canvases are the app's answer, held on the session. The
        // brief mirrors them, but only from the revision after the pick,
        // and a user who picks a canvas at approval writes no revision.
        let platforms = context.request.platforms();
        // A later run numbers after the candidates the session has, so
        // it adds to them instead of overwriting them.
        let first_number = match self.designs.list().await {
            Ok(rows) => next_candidate_number(&base, rows.iter().map(|row| row.id.as_str())),
            Err(_) => 1,
        };
        let plans = candidate_plans(&base, &platforms, count, first_number);
        let ids: Vec<String> = plans.iter().map(|plan| plan.design_id.clone()).collect();
        if plans.len() > count {
            log(&format!(
                "writing {} candidates: {count} variations across {} canvases",
                plans.len(),
                platforms.len()
            ));
        }
        let shares = self.shared_progress(&ids, 10, 90);
        let templates = self.brief_templates(&context.options, log).await;
        // Every candidate runs at the same time; each saves itself as
        // soon as it is ready.
        let mut tasks = tokio::task::JoinSet::new();
        for (index, plan) in plans.iter().enumerate() {
            let engine = self.clone();
            let client = client.clone();
            let context = context.clone();
            let concepts = concepts.clone();
            let candidate_number = plan.candidate_number;
            let variation = plan.variation;
            let viewport = plan.viewport;
            // One look per variation, wrapping when the user picked
            // fewer templates than variations.
            let template = candidate_template(&templates, variation);
            let attachments = Arc::clone(&attachments);
            let share = Arc::clone(&shares[index]);
            let log = Arc::clone(log);
            let id = plan.design_id.clone();
            // The card appears at once, as a placeholder with its bar.
            share(0.0);
            tasks.spawn(async move {
                let request = CandidateRequest {
                    context: &context,
                    candidate_number: variation,
                    viewport,
                    concepts: &concepts,
                    preview_screens: context.preview_screens(),
                    design_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_candidate(&client, &request, &attachments, &share, &log)
                    .await?;
                log(&format!("candidate {candidate_number}: saved as {id}"));
                Ok::<(), GenerationStop>(())
            });
        }
        let mut saved = Vec::new();
        let mut failures = Vec::new();
        while let Some(outcome) = tasks.join_next().await {
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(GenerationStop::NeedsClarification(set))) => {
                    tasks.shutdown().await;
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Ok(Err(GenerationStop::Failed(error))) => failures.push(error),
                Err(error) => failures.push(format!("candidate task failed: {error}")),
            }
        }
        // Every candidate saves itself, so the written ids are those on
        // disk under this base.
        for id in &ids {
            if matches!(self.designs.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Combines parts of `sources` into one new candidate, as
    /// `instruction` asks, and returns its id. The sources must share a
    /// canvas: a phone screen and a desktop screen do not merge. The
    /// new candidate takes the next free number and goes through the
    /// same fix and polish rounds as a fresh candidate.
    async fn merge_designs(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let mut loaded = Vec::new();
        for id in sources {
            let design = self
                .designs
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("design `{id}` does not exist")))?;
            loaded.push((id.as_str(), design));
        }
        let Some((first_id, first)) = loaded.first() else {
            return Err(GenerationStop::Failed(
                "a merge needs two candidates: pin them with @".to_owned(),
            ));
        };
        let viewport = first.viewport;
        if let Some((other_id, _)) = loaded
            .iter()
            .find(|(_, design)| design.viewport != viewport)
        {
            return Err(GenerationStop::Failed(format!(
                "`{first_id}` and `{other_id}` are on different canvases: merge candidates of one \
                 canvas"
            )));
        }
        let merge = MergeInput {
            sources: merge_sources(&loaded).map_err(GenerationStop::Failed)?,
            instruction: instruction.to_owned(),
        };
        let base = context.session_id.as_str();
        let rows = self
            .designs
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let design_id = candidate_id(base, number);
        log(&format!("merging {} into {design_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&design_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = CandidateRequest {
            context,
            candidate_number: number,
            viewport,
            concepts: &[],
            preview_screens: None,
            design_id: design_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {design_id}"));
        Ok(design_id)
    }

    /// Asks the model for `count` distinct concepts in one call. A reply
    /// that does not parse yields no concepts, and the candidates are
    /// written without them.
    pub(crate) async fn plan_concepts(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        count: usize,
        attachments: &Attachments,
        log: &LogSink,
    ) -> Result<Vec<Concept>, GenerationStop> {
        log(&format!("planning {count} concepts"));
        let messages = vec![
            serde_json::json!({ "role": "system", "content": concept_prompt(count) }),
            self.user_message(&concept_input(&context.request), attachments),
        ];
        let started = std::time::Instant::now();
        let reply = self.model.chat(client, &messages, context.effort()).await?;
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

    /// Applies a critique to one chosen design: the model rewrites the
    /// design against the brief and the critique, the result is
    /// validated, polished, and saved under the same id.
    /// Applies `instruction` to each design in turn and returns the ones
    /// it saved. One failure is logged and the rest still run; the turn
    /// fails only when every edit failed.
    async fn edit_designs(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for design_id in order.artifact_ids {
            match self
                .edit_design(client, context, design_id, order, log)
                .await
            {
                Ok(()) => saved.push(design_id.clone()),
                // A question from the model halts the whole turn: the
                // user answers it before any other edit.
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {design_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_design(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        design_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<(), GenerationStop> {
        let design = self
            .designs
            .load(design_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("design `{design_id}` does not exist"))
            })?;
        let instruction = order.instruction;
        let label = format!("edit {design_id}");
        // A change that names screens is about those screens: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named screens without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "screen")
            .into_iter()
            .filter(|index| *index < design.screens.len())
            .collect();
        let measured = crate::polish::dom_findings(&design, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "screens", &indexes);
        let total = design.screens.len();
        let (design_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&design), String::new())
        } else if order.is_fresh {
            (
                focused_design_json(&design, &indexes, true),
                fresh_note("screen", "screens", &indexes, total),
            )
        } else {
            (
                focused_design_json(&design, &indexes, false),
                focus_note("screen", "screens", &indexes, total),
            )
        };
        let design_json = design_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &design_json,
            note: &note,
            findings: &findings,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(&edit_prompt(&context.request, &input), &attachments),
        ];
        let original = design.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| {
                crate::patch::apply_patch(&original, crate::patch::parse_patch(content)?)
            }),
            progress: self.shared_progress(&[design_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
        // A fix can make a new problem. The touched screens are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&design.screens, &edited.screens, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_design = self
            .fix_edited_design(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        self.designs
            .save(design_id, &final_design)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {design_id}: saved"));
        Ok(())
    }

    /// Continues every requested preview at the same time, and every
    /// preview whose Finish is pressed while they run. Returns one
    /// outcome per artifact, in start order: the screens added, or the
    /// error.
    ///
    /// A late Finish would otherwise wait for the first continue to end.
    /// The loop wakes on every store change and on a timer, reads the
    /// trailing continue requests again, and starts the new ones.
    pub(crate) async fn continue_artifacts(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        ids: Vec<String>,
        log: &LogSink,
    ) -> Vec<(String, Result<usize, String>)> {
        let shared = ContinueShared {
            client,
            context,
            attachments: Arc::new(self.load_attachments(&context.session_id, log).await),
            group: self.progress_group(5, 95),
            log,
        };
        let mut batch = ContinueBatch::default();
        let mut outcomes: Vec<(String, Result<usize, String>)> = Vec::new();
        let mut changes = self.notifier.subscribe();
        self.start_continues(&mut batch, ids, &shared);
        loop {
            if batch.tasks.is_empty() {
                let late = self.late_continue_ids(context, &batch.started).await;
                if late.is_empty() {
                    break;
                }
                self.start_continues(&mut batch, late, &shared);
                continue;
            }
            tokio::select! {
                joined = batch.tasks.join_next() => match joined {
                    Some(Ok((id, outcome))) => outcomes.push((id, outcome)),
                    Some(Err(error)) => log(&format!("continue task failed: {error}")),
                    None => {}
                },
                changed = changes.changed() => {
                    if changed.is_err() {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
            let late = self.late_continue_ids(context, &batch.started).await;
            self.start_continues(&mut batch, late, &shared);
        }
        outcomes.sort_by_key(|(id, _)| batch.started.iter().position(|known| known == id));
        outcomes
    }

    /// Starts a continue for each id and records it as started.
    fn start_continues(
        &self,
        batch: &mut ContinueBatch,
        ids: Vec<String>,
        shared: &ContinueShared<'_>,
    ) {
        if ids.is_empty() {
            return;
        }
        if !batch.started.is_empty() {
            (shared.log)(&format!(
                "continuing {} more: {}",
                ids.len(),
                ids.join(", ")
            ));
        }
        for id in ids {
            let share = shared.group.share(&id);
            self.spawn_continue(&mut batch.tasks, &id, share, shared);
            batch.started.push(id);
        }
    }

    /// The trailing continue requests that are not running yet: a
    /// Finish pressed after this turn read its task.
    async fn late_continue_ids(
        &self,
        context: &GenerationContext,
        started: &[String],
    ) -> Vec<String> {
        match self.pick_task(context).await {
            Ok(Some(GenerationTask::Continue(ids))) => {
                ids.into_iter().filter(|id| !started.contains(id)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Starts one continue on the session's kind of artifact.
    fn spawn_continue(
        &self,
        tasks: &mut tokio::task::JoinSet<(String, Result<usize, String>)>,
        id: &str,
        share: ShareSink,
        shared: &ContinueShared<'_>,
    ) {
        let engine = self.clone();
        let client = shared.client.clone();
        let context = shared.context.clone();
        let id = id.to_owned();
        let attachments = Arc::clone(&shared.attachments);
        let log = Arc::clone(shared.log);
        tasks.spawn(async move {
            let outcome = match context.request.kind {
                design_model::ArtifactKind::Demo => {
                    engine
                        .continue_design(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Deck => {
                    engine
                        .continue_deck(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Document => {
                    engine
                        .continue_document(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Social => {
                    engine
                        .continue_social(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Print => {
                    engine
                        .continue_print(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
            };
            (id, outcome)
        });
    }

    /// Writes the remaining screens of the preview design `design_id` in
    /// chunks of `CONTINUE_CHUNK_SCREENS`. The design is saved after every
    /// chunk, so the canvas shows it grow, then polished once it is
    /// complete. Returns how many screens were added; 0 when the design is
    /// complete already.
    async fn continue_design(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
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
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
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
            let context = context.clone();
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
                        &continue_prompt(&context.request, &preview, &design_json, chunk),
                        &attachments,
                    ),
                ];
                let original = preview.clone();
                let written = preview.screens.len();
                let live_board = Arc::clone(&board);
                let live_show = Arc::clone(&show);
                let request = ArtifactRequest {
                    effort: context.effort().to_owned(),
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
                    .request_valid(&client, messages, &request, &log)
                    .await
                    .map_err(stop_to_string)?;
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
            return Err(failure_message(&failures, "no screens were added"));
        }
        // A failed chunk leaves the design continuable: the outline stays
        // until every title has a screen.
        if continued.screens.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        // The polish rounds fill the last share.
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
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
        context: &ArtifactRequest<'_, Design>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(design);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // The version that measured best, and its finding count. A
        // round that makes the page worse is measured, reported, and
        // then thrown away in favour of this.
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = design.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings = crate::polish::dom_findings(&design, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = design.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a page that is already good.
            if findings.is_empty() {
                stop = crate::polish::PolishStop::Clean;
                break;
            }
            // The last round did not reduce the findings, so the next
            // will not either. This also breaks a fix-one-break-one
            // loop that would otherwise run to the limit.
            if previous_count.is_some_and(|before| findings.len() >= before) {
                stop = crate::polish::PolishStop::NoImprovement;
                break;
            }
            previous_count = Some(findings.len());
            rounds_taken = round;
            let images = self.screen_images(&design, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} screen images)",
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
                .model
                .chat_with(
                    client,
                    self.model
                        .request_body(&messages, writing_effort(&context.effort)),
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
            context.report(DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32);
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_count)
        ));
        context.report(1.0);
        Ok(best)
    }

    /// Measures the touched screens of an edited design and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// screens measure clean, a round does not help, or the effort's
    /// round limit runs out. Returns the best version measured.
    async fn fix_edited_design(
        &self,
        client: &reqwest::Client,
        mut design: Design,
        fix: &EditFix<'_, Design>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(design);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = design.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured = crate::polish::dom_findings(&design, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "screens", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = design.clone();
            }
            if findings.is_empty() {
                stop = crate::polish::PolishStop::Clean;
                break;
            }
            if previous_count.is_some_and(|before| findings.len() >= before) {
                stop = crate::polish::PolishStop::NoImprovement;
                break;
            }
            previous_count = Some(findings.len());
            rounds_taken = round;
            log(&format!(
                "{label}: fix round {round} of at most {limit} ({} findings on the touched screens)",
                findings.len()
            ));
            let design_json = focused_design_json(&design, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("screen", "screens", &fix.indexes, design.screens.len());
            let instruction = fix_instruction("screens");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &design_json,
                note: &note,
                findings: &findings,
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                serde_json::json!({ "role": "user", "content": edit_prompt(fix.request, &input) }),
            ];
            let reply = self
                .model
                .chat_with(
                    client,
                    self.model
                        .request_body(&messages, writing_effort(&fix.context.effort)),
                    None,
                )
                .await;
            // The edit itself is done. A fix round that cannot reach the
            // model leaves the best version as it is.
            let content = match reply {
                Ok(content) => content,
                Err(error) => {
                    log(&format!("{label}: fix round {round} failed: {error}"));
                    break;
                }
            };
            let improved = crate::patch::parse_patch(&content)
                .and_then(|patch| crate::patch::apply_patch(&design, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => design = improved,
                Ok(_) => log(&format!(
                    "{label}: the fix failed validation; keeping the previous version"
                )),
                Err(parse_error) => log(&format!(
                    "{label}: fix reply unusable ({parse_error}); keeping the previous version"
                )),
            }
            fix.context
                .report(DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32);
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_count)
        ));
        fix.context.report(1.0);
        Ok(best)
    }

    /// PNG screenshots of the design's screens for the polish pass, at most
    /// `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images or
    /// no Chrome is installed; a failed screenshot is logged and skipped.
    async fn screen_images(&self, design: &Design, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !crate::screenshots::supports_vision(self.model.model()) {
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
    pub(crate) fn base_url(&self) -> String {
        self.address.clone()
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
    ) -> Result<Design, GenerationStop> {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(&candidate_prompt(request), attachments),
        ];
        let saver = LiveSaver::new(self, &request.design_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
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
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.screens.len());
        let polished = self
            .polish_design(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        self.designs
            .clear_user_paths(&request.design_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Sends `messages`, parses the artifact reply, and repairs it through
    /// fix rounds until it validates. A reply that asks for a blocking
    /// detail stops the run with a clarification.
    pub(crate) async fn request_valid<T: Validated>(
        &self,
        client: &reqwest::Client,
        mut messages: Vec<serde_json::Value>,
        context: &ArtifactRequest<'_, T>,
        log: &LogSink,
    ) -> Result<T, GenerationStop> {
        let label = &context.label;
        let fix_round_limit = fix_round_limit(&context.effort);
        let effort = writing_effort(&context.effort);
        for round in 0..=fix_round_limit {
            log(&format!("{label}: requesting (round {})", round + 1));
            let started = std::time::Instant::now();
            let content = self
                .model
                .chat_with(
                    client,
                    self.model.request_body(&messages, effort),
                    context.live.as_deref(),
                )
                .await?;
            log(&format!(
                "{label}: reply in {:.0}s ({} chars)",
                started.elapsed().as_secs_f32(),
                content.len()
            ));
            // A valid clarification request stops the run at once.
            if let Some(result) = clarification_request(&content) {
                match result {
                    Ok(set) => {
                        log(&format!("{label}: the model asked for clarification"));
                        return Err(GenerationStop::NeedsClarification(set));
                    }
                    Err(problems) => {
                        log(&format!(
                            "{label}: the clarification request was invalid ({} problems); retrying",
                            problems.len()
                        ));
                    }
                }
            }
            match (context.parse)(&content) {
                Ok(artifact) => {
                    let errors = artifact.problems();
                    if errors.is_empty() {
                        context.report(DRAFT_SHARE);
                        return Ok(artifact);
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
        Err(GenerationStop::Failed(format!(
            "{label} still fails after {fix_round_limit} fix rounds"
        )))
    }

    /// Reports each request's token usage to `sink`.
    pub fn with_usage_sink(mut self, sink: UsageSink) -> Self {
        self.model = self.model.with_usage_sink(sink);
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
    pub(crate) fn report_progress(&self, percent: u8) {
        if let Some(sink) = &self.progress_sink {
            sink(percent.min(100));
        }
    }

    /// One share sink per design in `design_ids`, for designs written at the
    /// same time. Each design reports its own 0.0 to 1.0: the design sink
    /// gets it as a percent under its id, and the turn progress becomes
    /// `base` plus `span` times the mean of all shares.
    /// A progress group that takes artifacts as they start: each share
    /// reports its own percentage, and the run's bar shows the mean.
    pub(crate) fn progress_group(&self, base: u8, span: u8) -> ProgressGroup {
        ProgressGroup {
            shares: Arc::new(std::sync::Mutex::new(Vec::new())),
            sink: self.progress_sink.clone(),
            design_sink: self.design_progress_sink.clone(),
            base,
            span,
        }
    }

    pub(crate) fn shared_progress(
        &self,
        design_ids: &[String],
        base: u8,
        span: u8,
    ) -> Vec<ShareSink> {
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
}
/// A user message content: the text alone, or OpenAI-style parts with
/// one `image_url` data URL per PNG when images are present.
pub(crate) fn user_content_with_images(text: &str, images: &[Vec<u8>]) -> serde_json::Value {
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
pub(crate) const ATTACHMENT_TEXT_LIMIT_BYTES: usize = 100 * 1024;

/// Most text inlined into one request over every file. A codebase of
/// source files adds up to more than a context window holds, so once
/// the budget is spent the remaining text files are named only.
pub(crate) const ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES: usize = 256 * 1024;

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
    let mut budget = ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES;
    let mut left_out = Vec::new();
    for file in &attachments.files {
        let before = budget;
        parts.extend(attachment_parts(file, can_see_images, &mut budget));
        if before == 0 && is_text_attachment(file) {
            left_out.push(file.name.clone());
        }
    }
    if !left_out.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": format!(
                "Named only, the request has no room for more text: {}. \
                 Ask the user for the part you need.",
                left_out.join(", ")
            ),
        }));
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

/// The content parts for one attached file. `budget` is the text the
/// request may still inline; an inlined file takes its share.
fn attachment_parts(
    file: &UploadAttachment,
    can_see_images: bool,
    budget: &mut usize,
) -> Vec<serde_json::Value> {
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
        return vec![inlined_text_part(file, &text, &size, budget)];
    }
    if crate::office::is_office_type(&file.content_type) {
        return match crate::office::office_text(&file.content_type, &file.bytes) {
            Ok(text) => vec![inlined_text_part(file, &text, &size, budget)],
            Err(error) => vec![label(&format!(
                ": the file could not be read ({error:#}). Tell the user if you need its content."
            ))],
        };
    }
    // The type comes from the extension, and no table names every
    // source or configuration file. A file that reads as text is text.
    if let Some(text) = text_of(&file.bytes) {
        return vec![inlined_text_part(file, text, &size, budget)];
    }
    vec![label(
        ": a file this request cannot carry. Tell the user if you need its content.",
    )]
}

/// The bytes as text when they are valid UTF-8 with no NUL byte, which
/// is what a source file, a configuration file, or a log looks like.
/// A binary file fails one of the two checks in its first bytes.
fn text_of(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// True when the file reaches the prompt as text: a text or JSON
/// type, an Office file, or an unknown type that reads as UTF-8.
fn is_text_attachment(file: &UploadAttachment) -> bool {
    file.content_type.starts_with("text/")
        || file.content_type == "application/json"
        || crate::office::is_office_type(&file.content_type)
        || (!file.content_type.starts_with("image/")
            && file.content_type != "application/pdf"
            && text_of(&file.bytes).is_some())
}

/// One text part with the file's text inlined, cut at
/// `ATTACHMENT_TEXT_LIMIT_BYTES` or at what is left of `budget`, on a
/// character boundary. With no budget left the file is named only.
fn inlined_text_part(
    file: &UploadAttachment,
    text: &str,
    size: &str,
    budget: &mut usize,
) -> serde_json::Value {
    if *budget == 0 {
        return serde_json::json!({
            "type": "text",
            "text": format!(
                "File {} ({}, {size}): named only, the request has no room for its text.",
                file.name, file.content_type
            ),
        });
    }
    let limit = ATTACHMENT_TEXT_LIMIT_BYTES.min(*budget);
    let (shown, note) = if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (&text[..end], "\n[cut: the file continues]")
    } else {
        (text, "")
    };
    *budget -= shown.len();
    serde_json::json!({
        "type": "text",
        "text": format!("File {} ({}, {size}):\n{shown}{note}", file.name, file.content_type),
    })
}

/// The reasoning effort for requests that write screen HTML: one level
/// under the run's effort, never below `low`. Writing markup gains
/// little from long reasoning, and the effort level still sets the fix
/// and polish rounds. `minimal` is not a value every model accepts.
pub(crate) fn writing_effort(effort: &str) -> &'static str {
    match effort {
        "high" => "medium",
        _ => "low",
    }
}

/// The complete objects of the JSON array under `key` in a partial
/// JSON text: the index of the array's `[`, and each complete `{…}`
/// item. `None` until the array has started.
pub(crate) fn complete_array_items<'text>(
    text: &'text str,
    key: &str,
) -> Option<(usize, Vec<&'text str>)> {
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
        name: title.to_owned(),
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
    /// True once `finish` has written the final design. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl LiveSaver {
    fn new(engine: &GenerationEngine, design_id: &str) -> Self {
        Self {
            designs: engine.designs.clone(),
            notifier: engine.notifier.clone(),
            design_id: design_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
            if saver.is_finished.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            if saver.designs.save(&saver.design_id, &design).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final design after every partial save landed.
    async fn finish(&self, design: &Design) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
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
pub(crate) struct ContinueChunk {
    pub(crate) first: usize,
    pub(crate) count: usize,
}

/// The chunks that cover outline titles `start..planned`, each at most
/// `CONTINUE_CHUNK_SCREENS` long.
pub(crate) fn continue_chunks(start: usize, planned: usize) -> Vec<ContinueChunk> {
    (start..planned)
        .step_by(CONTINUE_CHUNK_SCREENS)
        .map(|first| ContinueChunk {
            first,
            count: (planned - first).min(CONTINUE_CHUNK_SCREENS),
        })
        .collect()
}

/// The system prompt: role, rules, schema, the clarification protocol,
/// and one example design.
fn system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Design)).unwrap_or_default();
    format!(
        "You build screen designs as JSON documents. Each screen is one HTML fragment plus its own CSS, \
         for the px canvas the design's `viewport` names.\n\
         Follow these rules:\n{rules}\n\
         The design must conform to this JSON Schema:\n{schema}\n\
         Example design:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one design JSON document. No prose, no code fences.",
        rules = DEMO_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-design.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// A brief question set the model returned instead of a design, when
/// the reply carries a `needs_clarification` object. `Some(Err)` when
/// the object is present but invalid.
pub(crate) fn clarification_request(
    content: &str,
) -> Option<Result<BriefQuestionSet, Vec<QuestionSetError>>> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Wrapper {
        needs_clarification: BriefQuestionSet,
    }
    let wrapper: Wrapper = serde_json::from_str(&content[start..=end]).ok()?;
    let set = wrapper.needs_clarification;
    let problems = validate_question_set(&set);
    Some(if problems.is_empty() {
        Ok(set)
    } else {
        Err(problems)
    })
}

/// Turns a generation stop into a plain message, for callers that do not
/// carry a clarification.
pub(crate) fn stop_to_string(stop: GenerationStop) -> String {
    match stop {
        GenerationStop::Failed(message) => message,
        GenerationStop::NeedsClarification(_) => {
            "the model asked for clarification mid-continuation".to_owned()
        }
    }
}

/// The assistant summary after a run wrote designs.
fn wrote_summary(design_ids: &[String]) -> String {
    match design_ids.len() {
        0 => "The run wrote no designs.".to_owned(),
        1 => format!(
            "I wrote `{}`. Open it, then ask for a change in the chat.",
            design_ids[0]
        ),
        count => format!(
            "I wrote {count} candidates to the canvas. Pick one, then ask for a change in the chat."
        ),
    }
}

/// Turns a model reply into an artifact: a whole design or deck, or a
/// patch applied to an existing one.
pub(crate) type ReplyParser<'request, T> =
    Box<dyn Fn(&str) -> Result<T, String> + Send + Sync + 'request>;

/// An artifact the fix-round loop can validate: a design or a deck.
pub(crate) trait Validated {
    /// Every validation problem, empty when the artifact is ready.
    fn problems(&self) -> Vec<design_model::ValidationError>;
}

impl Validated for Design {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Deck {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Document {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Social {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Print {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

/// Effort, log label, and reply parser for one artifact request: a
/// candidate (the reply is the artifact) or an edit (the reply is a
/// patch).
pub(crate) struct ArtifactRequest<'request, T> {
    pub(crate) effort: String,
    pub(crate) label: String,
    pub(crate) parse: ReplyParser<'request, T>,
    /// Where this request reports its share of the turn: `DRAFT_SHARE`
    /// once the draft validates, then up to 1.0 over the polish rounds.
    pub(crate) progress: Option<ShareSink>,
    /// Gets the accumulated reply text while it streams, so the caller
    /// can show partial results.
    pub(crate) live: Option<Arc<TextSink>>,
}

impl<T> ArtifactRequest<'_, T> {
    /// Reports this request's share, when it has a sink.
    pub(crate) fn report(&self, fraction: f32) {
        if let Some(progress) = &self.progress {
            progress(fraction);
        }
    }
}

/// The user prompt for an edit: the design as it is, the request, and
/// the change the user asked for.
fn edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the design to change:\n{design_json}\n{note}\
         The design is for this request:\n{request}\n\
         Apply this change: {critique}\n{findings}\
         A reference like [screen 3, node 0/1 <h2.title>: What Swift Design does] names a screen \
         (1-based) and one element in that screen's html by its index path from the screen root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [screen 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one screen the same way, without their text. A reference like [screen 3] \
         names the screen alone: the change is about that screen. Change only what the critique asks for. Keep every other screen and \
         value as it is. Return every changed screen complete: html, css, and notes.\n{format}",
        design_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        findings = findings_note(input.findings),
        format = crate::patch::PATCH_FORMAT
    )
}

/// The design as a focused edit sees it: the title, the theme, the
/// viewport, the screen count, and only the screens at `indexes`, each
/// with its index.
fn focused_design_json(
    design: &Design,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let screens: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            design.screens.get(*index).map(|screen| {
                let screen = if is_fresh {
                    fresh_screen(screen)
                } else {
                    screen.clone()
                };
                serde_json::json!({ "index": index, "screen": screen })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": design.title,
        "theme": design.theme,
        "viewport": design.viewport,
        "screen_count": design.screens.len(),
        "screens": screens,
    }))
}

/// The screen as a regenerate shows it: its name and notes, without
/// its markup, so the model writes it anew instead of tweaking it.
fn fresh_screen(screen: &design_model::Screen) -> design_model::Screen {
    design_model::Screen {
        html: String::new(),
        css: None,
        ..screen.clone()
    }
}

/// What one candidate call needs: the run context, the candidate number,
/// every concept, so the prompt can name its own and the others, and
/// the preview length when the candidate is a preview.
struct CandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    /// The canvas this candidate is written for. One platform the user
    /// picked, not always the first.
    viewport: design_model::Viewport,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` screens plus the outline.
    preview_screens: Option<usize>,
    /// The id the candidate is saved under.
    design_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

/// The prompt lines for a preview candidate: write `count` screens and
/// the full outline.
fn preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} screens of the design, in order, starting with the \
         title screen. Put the screen titles of the complete design in `outline`, in order, \
         every screen title of the complete design. The app asks you for the remaining screens \
         later. Make these {count} screens show the theme, the layout language, and the text \
         density of the whole design.\n"
    )
}

/// The user prompt for one continuation chunk: the preview design, the
/// conversation, and the chunk's screens to add, as a patch of inserts.
fn continue_prompt(
    request: &SessionRequest,
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
         The design is for this request:\n{}\n",
        request_input(request)
    );
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

/// The message for a run that produced nothing.
///
/// `failures` is empty when every task reported success and the work
/// still is not there, which is a bug in this engine rather than a
/// model failure. An empty message told the user nothing, so each site
/// names itself instead.
pub(crate) fn failure_message(failures: &[String], fallback: &str) -> String {
    if failures.is_empty() {
        return fallback.to_owned();
    }
    failures.join("; ")
}

/// One candidate to write: which variation, which canvas, which id.
///
/// A run writes one design per platform per variation. The variation
/// number picks the concept and the template; the viewport picks the
/// canvas. The id numbers every candidate straight through, so the
/// existing `{base}-candidate-{n}` naming still holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CandidatePlan {
    /// The id the candidate is saved under.
    pub design_id: String,
    /// Position in the whole run, from one.
    pub candidate_number: usize,
    /// Which variation this is, from one.
    pub variation: usize,
    /// The canvas to write for.
    pub viewport: design_model::Viewport,
}

/// Every candidate a run writes: `variations` concepts on each canvas,
/// canvas by canvas, numbered from `first_number`. A run numbers after
/// the candidates the session has, so it adds to them instead of
/// overwriting them.
pub(crate) fn candidate_plans(
    base: &str,
    platforms: &[String],
    variations: usize,
    first_number: usize,
) -> Vec<CandidatePlan> {
    let mut plans = Vec::new();
    for viewport in platforms
        .iter()
        .map(|platform| design_model::Viewport::for_platform(platform))
    {
        for variation in 1..=variations.max(1) {
            let candidate_number = first_number.max(1) + plans.len();
            plans.push(CandidatePlan {
                design_id: crate::candidates::candidate_id(base, candidate_number),
                candidate_number,
                variation,
                viewport,
            });
        }
    }
    plans
}

/// The user prompt for one candidate: the request and the answers are
/// authoritative, plus the template, preview, concept, and effort notes.
fn candidate_prompt(request: &CandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a design for this request. The request and the answers are authoritative; do not \
         override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
    }
    if let Some(count) = request.preview_screens {
        prompt.push_str(&preview_note(count));
    }
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("design", merge));
    }
    let count = options.variation_count();
    // A merge is one candidate on its own, not one of the run's set.
    if count > 1 && request.merge.is_none() {
        prompt.push_str(&format!(
            "This is candidate {candidate_number} of {count}. Make it distinct from the other \
             candidates in theme, structure, and angle.\n"
        ));
        prompt.push_str(&concept_note(request.concepts, candidate_number - 1));
    }
    match options.effort.as_str() {
        "low" => prompt.push_str("Keep the design concise: fewer screens, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    // A run writes one design per platform, and each is drawn for its
    // own canvas.
    let viewport = request.viewport;
    prompt.push_str(&format!(
        "Set `viewport` to {} by {} px. Design every screen for that canvas.\n",
        viewport.width, viewport.height
    ));
    prompt.push_str("Reply with only the design JSON.");
    prompt
}

/// The template candidate `candidate_number` takes its look from.
/// Candidates are numbered from one and wrap around the chosen
/// templates, so three looks across five candidates run 1, 2, 3, 1, 2.
pub(crate) fn candidate_template(
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
pub(crate) fn template_note(template: &crate::templates::Template) -> String {
    let mut note = format!(
        "Use the saved template `{name}` for the look of this design.\n\
         Copy this theme into the design exactly. Use it instead of any palette or fonts \
         named elsewhere in this prompt:\n{theme}\n",
        name = template.name,
        theme = serde_json::to_string(&template.theme).unwrap_or_default(),
    );
    if let Some(style_note) = template
        .note
        .as_deref()
        .filter(|note| !note.trim().is_empty())
    {
        note.push_str(&format!(
            "The template's style note, from its brand material. Follow it:\n{style_note}\n"
        ));
    }
    if !template.screens.is_empty() {
        note.push_str(
            "These screens show the template style. Match their CSS: the same font sizes, \
             spacing, alignment, colors, and layout language. Write new content for the \
             request. Do not copy their text.\n",
        );
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::WorkflowState;

    use super::edit_targets;

    use super::{
        ATTACHMENT_TEXT_LIMIT_BYTES, ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES, Attachments,
        GenerationContext, GenerationEngine, GenerationOutcome, ProgressGroup, ProgressSink,
        UploadAttachment, answers_since_last_write, candidate_plans, edit_prompt,
        focused_design_json, system_prompt, text_of, trailing_continue_ids,
        user_content_with_attachments,
    };
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{FakeModelServer, SAMPLE_DESIGN, low_effort_options};

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    fn engine(
        server: &FakeModelServer,
        designs: &DesignStore,
        sessions: &SessionStore,
    ) -> GenerationEngine {
        GenerationEngine::new(
            server.configuration(),
            designs.clone(),
            sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        )
    }

    /// A fresh one-candidate, low-effort session, still in intake.
    async fn fresh_session(sessions: &SessionStore, request: &str) {
        sessions
            .create(NewSession::demo("talk", "Talk", request).with_options(low_effort_options()))
            .await
            .unwrap();
    }

    /// A session past its setup card: the app's own questions were
    /// asked, so the next planner turn is free to write.
    async fn set_up_session(sessions: &SessionStore, request: &str) {
        fresh_session(sessions, request).await;
        sessions
            .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn a_run_writes_one_candidate_per_canvas_per_variation() {
        let platforms = vec!["desktop web".to_owned(), "phone".to_owned()];
        let plans = candidate_plans("talk", &platforms, 2, 1);
        assert_eq!(plans.len(), 4);
        let ids: Vec<&str> = plans.iter().map(|plan| plan.design_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "talk-candidate-1",
                "talk-candidate-2",
                "talk-candidate-3",
                "talk-candidate-4"
            ]
        );
        // The canvases run in the order the user picked them, and the
        // same two concepts are drawn on each.
        assert_eq!(plans[0].viewport, design_model::Viewport::default());
        assert_eq!(plans[1].variation, 2);
        assert_eq!(plans[2].viewport.width, 390);
        assert_eq!(plans[3].variation, 2);
    }

    #[test]
    fn one_canvas_leaves_the_candidate_numbering_alone() {
        let one = [String::new()];
        let plans = candidate_plans("talk", &one, 3, 1);
        assert_eq!(plans.len(), 3);
        assert_eq!(plans[0].design_id, "talk-candidate-1");
        assert_eq!(plans[2].variation, 3);
        // Zero variations still writes one candidate.
        assert_eq!(candidate_plans("talk", &one, 0, 1).len(), 1);
    }

    #[test]
    fn a_second_run_numbers_after_the_first() {
        let one = [String::new()];
        let plans = candidate_plans("talk", &one, 2, 4);
        let ids: Vec<&str> = plans.iter().map(|plan| plan.design_id.as_str()).collect();
        assert_eq!(ids, ["talk-candidate-4", "talk-candidate-5"]);
        assert_eq!(plans[1].candidate_number, 5);
        assert_eq!(plans[1].variation, 2);
        // A first number of zero still starts at one.
        assert_eq!(
            candidate_plans("talk", &one, 1, 0)[0].design_id,
            "talk-candidate-1"
        );
    }

    #[test]
    fn system_prompt_names_the_clarification_protocol() {
        let prompt = system_prompt();
        assert!(prompt.contains("The request and the answers are authoritative"));
        assert!(prompt.contains("needs_clarification"));
        assert!(prompt.contains("\"viewport\""));
    }

    #[tokio::test]
    async fn the_planner_reads_the_users_source_files() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Writing it now.","generate":true}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let uploads = crate::uploads::UploadStore::new(directory.path().join("uploads"));
        uploads
            .save(
                "talk",
                "spec.md",
                b"The deck is for iOS developers new to Swift.",
            )
            .await
            .unwrap();
        fresh_session(&sessions, "An intro deck.").await;
        let _ = engine(&server, &designs, &sessions)
            .with_uploads(uploads)
            .run("talk", silent_log())
            .await;
        // Planning blind is what produced questions the files already
        // answered, so the first call must carry the file.
        let planner_call = server.requests()[0].to_string();
        assert!(planner_call.contains("The user's source files follow"));
        assert!(planner_call.contains("iOS developers new to Swift"));
    }

    #[test]
    fn an_office_file_reaches_the_prompt_as_its_text() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        std::io::Write::write_all(
            &mut writer,
            b"<w:document><w:p><w:r><w:t>Pricing starts at 9 a month.</w:t></w:r></w:p></w:document>",
        )
        .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let attachments = Attachments {
            files: vec![UploadAttachment {
                name: "brief.docx".to_owned(),
                content_type: crate::office::DOCX.to_owned(),
                bytes,
            }],
            skipped: Vec::new(),
        };
        let content = user_content_with_attachments("Go.", &attachments, false).to_string();
        assert!(content.contains("File brief.docx"));
        assert!(content.contains("Pricing starts at 9 a month."));
        assert!(!content.contains("cannot carry"));
        // A broken archive is reported, not dropped in silence.
        let broken = Attachments {
            files: vec![UploadAttachment {
                name: "brief.docx".to_owned(),
                content_type: crate::office::DOCX.to_owned(),
                bytes: b"not a zip".to_vec(),
            }],
            skipped: Vec::new(),
        };
        let content = user_content_with_attachments("Go.", &broken, false).to_string();
        assert!(content.contains("could not be read"));
    }

    #[test]
    fn a_source_file_of_unknown_type_reaches_the_prompt_as_its_text() {
        let source = Attachments {
            files: vec![UploadAttachment {
                name: "main.zig".to_owned(),
                content_type: "application/octet-stream".to_owned(),
                bytes: b"pub fn main() void {}\n".to_vec(),
            }],
            skipped: Vec::new(),
        };
        let content = user_content_with_attachments("Go.", &source, false).to_string();
        assert!(content.contains("pub fn main() void {}"));
        assert!(!content.contains("cannot carry"));
        // A binary file is still named only.
        let binary = Attachments {
            files: vec![UploadAttachment {
                name: "font.bin".to_owned(),
                content_type: "application/octet-stream".to_owned(),
                bytes: vec![0, 159, 146, 150, 0, 1],
            }],
            skipped: Vec::new(),
        };
        let content = user_content_with_attachments("Go.", &binary, false).to_string();
        assert!(content.contains("cannot carry"));
        assert_eq!(text_of(b"plain"), Some("plain"));
        assert_eq!(text_of(b"a\0b"), None);
        assert_eq!(text_of(&[0xff, 0xfe]), None);
    }

    #[test]
    fn the_inlined_text_stops_at_the_request_budget() {
        let file = |name: &str, bytes: usize| UploadAttachment {
            name: name.to_owned(),
            content_type: "text/plain; charset=utf-8".to_owned(),
            bytes: vec![b'~'; bytes],
        };
        // Three files at the per-file cap pass the budget: the third
        // is cut at what is left, the fourth is named only, and the
        // model is told which one it is.
        let attachments = Attachments {
            files: vec![
                file("a.rs", ATTACHMENT_TEXT_LIMIT_BYTES),
                file("b.rs", ATTACHMENT_TEXT_LIMIT_BYTES),
                file("c.rs", ATTACHMENT_TEXT_LIMIT_BYTES),
                file("d.rs", 10),
            ],
            skipped: Vec::new(),
        };
        let content = user_content_with_attachments("Go.", &attachments, false);
        let text = content.to_string();
        assert!(text.contains("File a.rs"));
        assert!(text.contains("File c.rs"));
        assert!(text.contains("[cut: the file continues]"));
        assert!(text.contains("File d.rs (text/plain; charset=utf-8, 10 B): named only"));
        assert!(text.contains("no room for more text: d.rs"));
        let inlined: usize = content
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|part| part["text"].as_str())
            .map(|part| part.matches('~').count())
            .sum();
        assert_eq!(inlined, ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES);
        // A small set is inlined whole, with no note.
        let small = Attachments {
            files: vec![file("a.rs", 10), file("b.rs", 10)],
            skipped: Vec::new(),
        };
        let text = user_content_with_attachments("Go.", &small, false).to_string();
        assert!(!text.contains("named only"));
    }

    #[test]
    fn a_run_of_finish_presses_continues_every_candidate() {
        let messages = vec![
            ChatMessage::user("A landing page.", None),
            ChatMessage::assistant("Two candidates."),
            ChatMessage::continue_request("Finish it.", "talk-candidate-1"),
            ChatMessage::continue_request("Finish it.", "talk-candidate-2"),
            // The same card pressed twice is one request.
            ChatMessage::continue_request("Finish it.", "talk-candidate-2"),
        ];
        assert_eq!(
            trailing_continue_ids(&messages),
            ["talk-candidate-1", "talk-candidate-2"]
        );
    }

    #[test]
    fn a_continue_from_an_earlier_turn_is_that_turns_business() {
        let messages = vec![
            ChatMessage::continue_request("Finish it.", "talk-candidate-1"),
            ChatMessage::user("Make the title bigger.", None),
            ChatMessage::continue_request("Finish it.", "talk-candidate-2"),
        ];
        assert_eq!(trailing_continue_ids(&messages), ["talk-candidate-2"]);
        assert!(trailing_continue_ids(&[]).is_empty());
    }

    #[tokio::test]
    async fn the_first_turn_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        fresh_session(&sessions, "A landing page.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        // The app owns the fixed questions, so the first turn asks them
        // whatever the planner decided.
        assert!(matches!(
            outcome,
            GenerationOutcome::NeedsClarification { question_set: 1 }
        ));
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Clarifying);
        let set = sessions
            .read_question_set("talk", 1)
            .await
            .unwrap()
            .unwrap();
        // No question of the agent's own: the studio draws the app's
        // cards in this set's grid.
        assert!(set.questions.is_empty());
        assert!(set.can_proceed_with_assumptions);
        // The planner promised a write it does not get to make, so the
        // card speaks for itself instead of repeating that promise.
        let messages = sessions.messages("talk").await.unwrap();
        assert_eq!(messages[0].content, set.message);
        assert!(!messages[0].content.contains("Writing it now"));
        // Nothing was written: the model was called once, to plan.
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn the_setup_turn_fills_the_app_questions_the_request_answers() {
        let server = FakeModelServer::start().await;
        server.push_text(
            r#"{"reply":"A todo app in Zed's style.","questions":[],"suggestions":{"product_kind":"developer_tool","color_mode":"dark","audience":"newcomers","scope":"whole_app"},"generate":false,"edit":false}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        fresh_session(&sessions, "A TODO app with Zed IDE's aesthetics.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            GenerationOutcome::NeedsClarification { question_set: 1 }
        ));
        // The card opens with the answers the request gave, marked as
        // suggested. A deck axis and an unknown value are dropped.
        let options = sessions.read("talk").await.unwrap().unwrap().options;
        assert_eq!(options.product_kind.as_deref(), Some("developer_tool"));
        assert_eq!(options.color_mode.as_deref(), Some("dark"));
        assert_eq!(options.audience, None);
        assert_eq!(options.scope, None);
        assert_eq!(options.suggested, ["color_mode", "product_kind"]);
    }

    #[tokio::test]
    async fn a_second_turn_writes_without_asking_again() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DESIGN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "A landing page.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    }

    #[tokio::test]
    async fn a_planner_question_set_moves_the_session_to_clarifying() {
        let server = FakeModelServer::start().await;
        server.push_text(
            r#"{"reply":"Two things first.","questions":[{"question":"Who is it for?","options":["Developers","Buyers"]}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        fresh_session(&sessions, "A landing page.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            GenerationOutcome::NeedsClarification { question_set: 1 }
        ));
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Clarifying);
        let set = sessions
            .read_question_set("talk", 1)
            .await
            .unwrap()
            .unwrap();
        assert!(set.can_proceed_with_assumptions);
        assert_eq!(set.questions[0].options[1].label, "Buyers");
        let messages = sessions.messages("talk").await.unwrap();
        assert_eq!(messages[0].question_set, Some(1));
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("asked_questions"));
        // The planner saw the request, not a brief.
        let text = server.requests()[0].to_string();
        assert!(text.contains("Request:"));
        assert!(text.contains("Candidates on the canvas: 0"));
    }

    #[tokio::test]
    async fn a_plain_reply_keeps_the_state_and_lands_in_the_chat() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Hello. Tell me what to build."}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "Hi.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Replied));
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Clarifying);
        let messages = sessions.messages("talk").await.unwrap();
        assert_eq!(messages[0].content, "Hello. Tell me what to build.");
        assert_eq!(
            sessions.runs("talk").await.unwrap()[0].result.as_deref(),
            Some("replied")
        );
    }

    #[tokio::test]
    async fn generation_sends_the_request_and_the_answers_not_a_brief() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DESIGN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "A landing page for retail investors.").await;
        engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        let text = server.requests()[1].to_string();
        assert!(text.contains("retail investors"));
        assert!(text.contains("The request and the answers are authoritative"));
        assert!(!text.contains("approved brief"));
    }

    #[tokio::test]
    async fn a_valid_design_reply_is_saved_as_a_candidate_and_recorded_on_the_run() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DESIGN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "A landing page.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(designs.load("talk-candidate-1").await.unwrap().is_some());
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("succeeded"));
        assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
        // The planner's reply landed in the chat before the write.
        let messages = sessions.messages("talk").await.unwrap();
        assert_eq!(messages[0].content, "Writing it now.");
        assert_eq!(
            sessions.read("talk").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    #[tokio::test]
    async fn a_needs_clarification_reply_moves_the_session_back_to_clarifying() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(
            r#"{"needs_clarification":{"title":"One thing","message":"Which brand color?","questions":[{"id":"color","label":"Which brand color?","kind":"short_text","required":true}]}}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "A landing page.").await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            GenerationOutcome::NeedsClarification { .. }
        ));
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Clarifying);
    }

    #[tokio::test]
    async fn an_invalid_design_gets_fix_rounds_then_fails() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        // Low effort gives two fix rounds: three attempts, all empty.
        for _ in 0..3 {
            server.push_text("{}");
        }
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "A landing page.").await;
        let result = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await;
        assert!(result.unwrap_err().contains("fix rounds"));
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("failed"));
    }

    #[test]
    fn answers_count_per_request_from_the_last_write() {
        let record = |at: &str, count: usize| crate::sessions::AnswerRecord {
            question_set: 1,
            answers: (0..count)
                .map(|index| design_model::QuestionAnswer {
                    question_id: format!("q{index}"),
                    values: Vec::new(),
                    other_text: None,
                    skipped: true,
                })
                .collect(),
            at: at.to_owned(),
        };
        let records = vec![
            record("2026-08-29T07:00:00Z", 3),
            record("2026-08-29T07:30:00Z", 2),
        ];
        let mut wrote = ChatMessage::assistant("I wrote 2 candidates.")
            .with_artifacts(vec!["talk-candidate-1".to_owned()]);
        wrote.at = Some("2026-08-29T07:10:00Z".to_owned());
        let messages = vec![ChatMessage::user("A talk.", None), wrote];
        assert_eq!(answers_since_last_write(&messages, &records), 2);
        assert_eq!(answers_since_last_write(&messages[..1], &records), 5);
    }

    #[test]
    fn a_focused_design_edit_shows_only_the_named_screens_and_their_findings() {
        let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        design.screens.push(design.screens[0].clone());
        let focused = focused_design_json(&design, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["screen_count"], design.screens.len());
        assert_eq!(value["screens"].as_array().unwrap().len(), 1);
        assert_eq!(value["screens"][0]["index"], 1);
        let request = SessionRequest {
            request: "A landing page.".to_owned(),
            kind: design_model::ArtifactKind::Demo,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["screens[1] h1 (0/1): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[screen 2, node 0/1 <h1>: x] Fix it.",
            artifact_json: &focused,
            note: "Only screen 2 is shown.\n",
            findings: &findings,
        };
        let prompt = edit_prompt(&request, &input);
        assert!(prompt.contains("Only screen 2 is shown."));
        assert!(prompt.contains("- screens[1] h1 (0/1): overflow: shorten"));
    }

    #[tokio::test]
    async fn a_finish_pressed_mid_run_is_a_late_continue() {
        let server = FakeModelServer::start().await;
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        design.outline = (1..=6).map(|number| format!("Screen {number}")).collect();
        designs.save("talk-candidate-1", &design).await.unwrap();
        designs.save("talk-candidate-2", &design).await.unwrap();
        set_up_session(&sessions, "A landing page.").await;
        let session = sessions.read("talk").await.unwrap().unwrap();
        let context = GenerationContext {
            request: SessionRequest {
                request: session.request.clone(),
                kind: session.artifact_kind,
                answers: Vec::new(),
                options: session.options.clone(),
            },
            options: session.options.clone(),
            session_id: "talk".to_owned(),
        };
        let engine = engine(&server, &designs, &sessions);
        let press = |id: &str| ChatMessage::continue_request("Finish it.", id);
        sessions
            .append_message("talk", press("talk-candidate-1"))
            .await
            .unwrap();
        let started = vec!["talk-candidate-1".to_owned()];
        // Nothing new: the running continue is the only request.
        assert!(
            engine
                .late_continue_ids(&context, &started)
                .await
                .is_empty()
        );
        // The second Finish arrives while the first runs.
        sessions
            .append_message("talk", press("talk-candidate-2"))
            .await
            .unwrap();
        assert_eq!(
            engine.late_continue_ids(&context, &started).await,
            vec!["talk-candidate-2".to_owned()]
        );
    }

    #[test]
    fn a_progress_group_takes_shares_as_they_start() {
        let reported = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let sink: ProgressSink = {
            let reported = Arc::clone(&reported);
            Arc::new(move |percent| reported.lock().unwrap().push(percent))
        };
        let group = ProgressGroup {
            shares: Arc::new(std::sync::Mutex::new(Vec::new())),
            sink: Some(sink),
            design_sink: None,
            base: 0,
            span: 100,
        };
        let first = group.share("a");
        first(1.0);
        // A second share halves the mean: the bar steps back for the
        // late arrival instead of hiding it.
        let second = group.share("b");
        second(0.0);
        second(0.5);
        assert_eq!(*reported.lock().unwrap(), vec![100, 50, 75]);
    }

    #[tokio::test]
    async fn an_edit_on_a_preview_is_not_a_continue() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the hero.","edit":true}"#);
        server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        design.outline = (1..=6).map(|number| format!("Screen {number}")).collect();
        assert!(design.is_preview());
        designs.save("talk-candidate-1", &design).await.unwrap();
        set_up_session(&sessions, "A landing page.").await;
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
        sessions
            .append_message(
                "talk",
                ChatMessage::user("Tighten the hero.", Some("talk-candidate-1")),
            )
            .await
            .unwrap();
        engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        // The planner ran and the patch applied; nothing was continued.
        let planner = server.requests()[0].to_string();
        assert!(planner.contains("You plan software demos"));
        let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
        assert!(edited.screens[0].html.contains("Tighter"));
        assert!(edited.is_preview());
    }

    #[tokio::test]
    async fn a_chat_request_with_a_design_open_patches_that_design() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the hero.","edit":true}"#);
        server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        designs.save("talk-candidate-1", &design).await.unwrap();
        set_up_session(&sessions, "A landing page.").await;
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
        sessions
            .append_message(
                "talk",
                ChatMessage::user("Tighten the hero.", Some("talk-candidate-1")),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
        assert!(edited.screens[0].html.contains("Tighter"));
        let text = server.requests()[1].to_string();
        assert!(text.contains("Apply this change: Tighten the hero."));
        let planner = server.requests()[0].to_string();
        assert!(planner.contains("Artifacts named this turn: talk-candidate-1"));
    }

    #[tokio::test]
    async fn a_message_with_pinned_candidates_patches_each_of_them() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening both.","edit":true}"#);
        // Each edit takes a patch reply and one fix-round reply.
        for _ in 0..4 {
            server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
        }
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        designs.save("talk-candidate-1", &design).await.unwrap();
        designs.save("talk-candidate-2", &design).await.unwrap();
        designs.save("talk-candidate-3", &design).await.unwrap();
        set_up_session(&sessions, "A landing page.").await;
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
        let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-3".to_owned()];
        sessions
            .append_message(
                "talk",
                ChatMessage::user("[candidate 1] [candidate 3] Tighten the hero.", None)
                    .with_pinned(pinned),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        let GenerationOutcome::Wrote { design_ids } = outcome else {
            panic!("expected a write");
        };
        assert_eq!(
            design_ids,
            vec!["talk-candidate-1".to_owned(), "talk-candidate-3".to_owned()]
        );
        for id in ["talk-candidate-1", "talk-candidate-3"] {
            let edited = designs.load(id).await.unwrap().unwrap();
            assert!(edited.screens[0].html.contains("Tighter"), "{id}");
        }
        let untouched = designs.load("talk-candidate-2").await.unwrap().unwrap();
        assert!(!untouched.screens[0].html.contains("Tighter"));
        let planner = server.requests()[0].to_string();
        assert!(planner.contains("Artifacts named this turn: talk-candidate-1, talk-candidate-3"));
    }

    /// A reviewing session with `count` saved candidates of `design`.
    async fn reviewing_session_with(
        sessions: &SessionStore,
        designs: &DesignStore,
        design: &design_model::Design,
        count: usize,
    ) {
        for number in 1..=count {
            designs
                .save(&format!("talk-candidate-{number}"), design)
                .await
                .unwrap();
        }
        set_up_session(sessions, "A landing page.").await;
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_candidates_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_DESIGN);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"screens":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        reviewing_session_with(&sessions, &designs, &design, 2).await;
        let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-2".to_owned()];
        sessions
            .append_message(
                "talk",
                ChatMessage::user(
                    "[candidate 1] [candidate 2] Hero from 1, pricing from 2.",
                    None,
                )
                .with_pinned(pinned),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        let GenerationOutcome::Wrote { design_ids } = outcome else {
            panic!("expected a write");
        };
        assert_eq!(design_ids, vec!["talk-candidate-3".to_owned()]);
        assert!(designs.load("talk-candidate-3").await.unwrap().is_some());
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one design"));
        assert!(text.contains("Hero from 1, pricing from 2."));
        assert!(text.contains("Candidate 1:"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_merge_across_canvases_is_refused() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        reviewing_session_with(&sessions, &designs, &design, 1).await;
        let mut phone = design.clone();
        phone.viewport = design_model::Viewport::for_platform("phone");
        designs.save("talk-candidate-2", &phone).await.unwrap();
        let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-2".to_owned()];
        sessions
            .append_message(
                "talk",
                ChatMessage::user("[candidate 1] [candidate 2] Combine them.", None)
                    .with_pinned(pinned),
            )
            .await
            .unwrap();
        let error = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap_err();
        assert!(error.contains("different canvases"), "{error}");
        assert!(designs.load("talk-candidate-3").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_merge_with_one_pin_is_an_edit() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Changing it.","merge":true}"#);
        server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        reviewing_session_with(&sessions, &designs, &design, 1).await;
        sessions
            .append_message(
                "talk",
                ChatMessage::user("[candidate 1] Tighten the hero.", None)
                    .with_pinned(vec!["talk-candidate-1".to_owned()]),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        let GenerationOutcome::Wrote { design_ids } = outcome else {
            panic!("expected a write");
        };
        assert_eq!(design_ids, vec!["talk-candidate-1".to_owned()]);
        let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
        assert!(edited.screens[0].html.contains("Tighter"));
    }

    #[tokio::test]
    async fn a_regenerated_screen_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its screen itself.
        server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:64px;}"}}]}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        design.screens[0].html = "<h1>Old hero markup</h1>".to_owned();
        reviewing_session_with(&sessions, &designs, &design, 1).await;
        sessions
            .append_message(
                "talk",
                ChatMessage::regenerate_request(
                    "[screen 1] Write this screen anew.",
                    "talk-candidate-1",
                ),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let text = server.requests()[0].to_string();
        assert!(text.contains("Write screen 1 of"));
        assert!(text.contains("anew"));
        assert!(!text.contains("Old hero markup"));
        let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
        assert!(edited.screens[0].html.contains("Fresh"));
        assert_eq!(edited.screens.len(), design.screens.len());
    }

    #[tokio::test]
    async fn a_later_run_numbers_after_the_candidates_the_session_has() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DESIGN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        reviewing_session_with(&sessions, &designs, &design, 2).await;
        sessions
            .append_message("talk", ChatMessage::user("Another take.", None))
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        let GenerationOutcome::Wrote { design_ids } = outcome else {
            panic!("expected a write");
        };
        assert_eq!(design_ids, vec!["talk-candidate-3".to_owned()]);
    }

    #[test]
    fn the_edit_targets_are_the_pins_then_the_open_artifact_then_the_chosen_one() {
        let pinned = vec![
            ChatMessage::user("Bigger.", Some("talk-candidate-1")).with_pinned(vec![
                "talk-candidate-2".to_owned(),
                "talk-candidate-3".to_owned(),
            ]),
        ];
        assert_eq!(
            edit_targets(&pinned, Some("talk-candidate-1")),
            vec!["talk-candidate-2".to_owned(), "talk-candidate-3".to_owned()]
        );
        let open = vec![ChatMessage::user("Bigger.", Some("talk-candidate-1"))];
        assert_eq!(
            edit_targets(&open, Some("talk-candidate-2")),
            vec!["talk-candidate-1".to_owned()]
        );
        let plain = vec![
            ChatMessage::user("Bigger.", None),
            ChatMessage::assistant("Done."),
        ];
        assert_eq!(
            edit_targets(&plain, Some("talk-candidate-2")),
            vec!["talk-candidate-2".to_owned()]
        );
        assert!(edit_targets(&plain, None).is_empty());
    }
}
