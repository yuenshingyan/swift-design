//! The built-in generation engine.
//!
//! Talks to the model through `model_client`, with the user's own keys
//! and only when a run starts. The loop: read the brief, ask the model
//! for each candidate design, validate, feed every validation error
//! back for a fix round, and save the result. The studio watches it all
//! through `/events`.

use std::sync::Arc;

use design_model::{
    BriefQuestionSet, Critique, Design, DesignBrief, QuestionSetError, validate_question_set,
};

use crate::concepts::{Concept, concept_input, concept_note, concept_prompt, parse_concepts};
use crate::designs::DesignStore;
use crate::events::ChangeNotifier;
use crate::instructions::CONTENT_RULES;
use crate::model_client::{LogSink, ModelClient, ModelConfiguration, TextSink, UsageSink};
use crate::sessions::{ChatMessage, RunMode, RunOptions, RunRecord, SessionStore};

/// Fix rounds per candidate before giving up, by effort level.
fn fix_round_limit(effort: &str) -> usize {
    match effort {
        "low" => 2,
        "high" => 4,
        _ => 3,
    }
}

/// Screens a preview candidate writes before the rest are continued.
const PREVIEW_SCREEN_COUNT: usize = 3;

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
}

/// What the model must do this run.
enum GenerationTask {
    /// Write the requested candidates.
    Candidates,
    /// Apply a critique to one chosen design.
    Edit {
        /// The design id to edit.
        design: String,
        /// The critique to apply.
        critique: Critique,
    },
    /// Continue the preview designs named here.
    Continue(Vec<String>),
}

/// The approved brief and options one generation run works from.
#[derive(Clone)]
struct GenerationContext {
    brief: DesignBrief,
    options: RunOptions,
    session_id: String,
}

impl GenerationContext {
    /// The effort level for this run.
    fn effort(&self) -> &str {
        &self.options.effort
    }

    /// The preview screen count, or `None` for complete candidates.
    fn preview_screens(&self) -> Option<usize> {
        self.options.preview.then_some(PREVIEW_SCREEN_COUNT)
    }
}

/// Why a generation run stopped without writing a design.
enum GenerationStop {
    /// The run failed with this message.
    Failed(String),
    /// The model asked for a blocking detail. The engine writes the set
    /// and returns the session to clarifying.
    NeedsClarification(BriefQuestionSet),
}

impl From<String> for GenerationStop {
    fn from(message: String) -> Self {
        GenerationStop::Failed(message)
    }
}

/// The built-in engine: the model client plus the stores it writes.
#[derive(Clone)]
pub struct GenerationEngine {
    model: ModelClient,
    designs: DesignStore,
    sessions: SessionStore,
    address: String,
    notifier: ChangeNotifier,
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
            sessions,
            address,
            notifier,
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
        let can_see_images = crate::screenshots::supports_vision(self.model.model());
        serde_json::json!({
            "role": "user",
            "content": user_content_with_attachments(text, attachments, can_see_images),
        })
    }

    /// The templates the options name, in order. A template that was
    /// deleted is skipped, so the run still writes the rest.
    async fn brief_templates(
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

    /// Runs one generation turn for `session_id`: read the approved
    /// brief, write designs from it, and report the outcome. The session
    /// must be in the generating state.
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
        if session.state != design_model::WorkflowState::Generating {
            return Err(format!(
                "the session is in state `{}`, not generating",
                session.state
            ));
        }
        let revision = session
            .approved_revision
            .ok_or_else(|| "the session has no approved brief".to_owned())?;
        let brief = self
            .sessions
            .read_brief(session_id, revision)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("brief revision {revision} is missing"))?;
        let context = GenerationContext {
            brief,
            options: session.options.clone(),
            session_id: session_id.to_owned(),
        };
        let task = self.pick_task(&session, &context).await?;
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
                    brief_revision: Some(revision),
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
            "generation · {} · brief revision {revision} · effort {}",
            self.label(),
            context.effort(),
        ));
        self.report_progress(0);
        let client = ModelClient::build_http_client()?;
        let outcome = self.execute(&client, &context, task, &log).await;
        self.report_progress(100);
        self.settle(session_id, &run_id, outcome, &log).await
    }

    /// Picks what the run does from the session and the brief.
    async fn pick_task(
        &self,
        session: &crate::sessions::Session,
        context: &GenerationContext,
    ) -> Result<GenerationTask, String> {
        if let Some(pending) = &session.pending_critique {
            return Ok(GenerationTask::Edit {
                design: pending.design.clone(),
                critique: pending.critique.clone(),
            });
        }
        let continues = self.continue_requests(&context.session_id).await?;
        if !continues.is_empty() {
            return Ok(GenerationTask::Continue(continues));
        }
        Ok(GenerationTask::Candidates)
    }

    /// The preview designs the latest user turn asked to continue: the
    /// design id of the newest user message, when that design exists and
    /// is still a preview.
    async fn continue_requests(&self, session_id: &str) -> Result<Vec<String>, String> {
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let Some(design_id) = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .and_then(|message| message.design.clone())
        else {
            return Ok(Vec::new());
        };
        match self.designs.load(&design_id).await {
            Ok(Some(design)) if design.is_preview() => Ok(vec![design_id]),
            _ => Ok(Vec::new()),
        }
    }

    /// Runs the chosen task and returns the outcome.
    async fn execute(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => self.generate_candidates(client, context, log).await,
            GenerationTask::Edit { design, critique } => {
                self.edit_design(client, context, &design, &critique, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![design],
                })
            }
            GenerationTask::Continue(design_ids) => {
                let refs: Vec<&str> = design_ids.iter().map(String::as_str).collect();
                let outcomes = self.continue_designs(client, context, &refs, log).await;
                if outcomes.iter().all(|(_, outcome)| outcome.is_err()) {
                    let failures: Vec<String> = outcomes
                        .iter()
                        .filter_map(|(id, outcome)| {
                            outcome.as_ref().err().map(|error| format!("{id}: {error}"))
                        })
                        .collect();
                    return Err(GenerationStop::Failed(failures.join("; ")));
                }
                Ok(GenerationOutcome::Wrote { design_ids })
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
            Ok(GenerationOutcome::Wrote { design_ids }) => {
                let summary = wrote_summary(&design_ids);
                self.say(session_id, &summary).await?;
                self.sessions
                    .update(session_id, |session| session.pending_critique = None)
                    .await
                    .map_err(|error| error.to_string())?;
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
        let attachments = Arc::new(self.load_attachments(log).await);
        let concepts = if count > 1 {
            self.plan_concepts(client, context, count, &attachments, log)
                .await?
        } else {
            Vec::new()
        };
        self.report_progress(10);
        let base = context.session_id.clone();
        let ids: Vec<String> = (1..=count)
            .map(|candidate_number| {
                if count > 1 {
                    format!("{base}-candidate-{candidate_number}")
                } else {
                    format!("{base}-candidate-1")
                }
            })
            .collect();
        let shares = self.shared_progress(&ids, 10, 90);
        let templates = self.brief_templates(&context.options, log).await;
        // Every candidate runs at the same time; each saves itself as
        // soon as it is ready.
        let mut tasks = tokio::task::JoinSet::new();
        for candidate_number in 1..=count {
            let engine = self.clone();
            let client = client.clone();
            let context = context.clone();
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
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_screens: context.preview_screens(),
                    design_id: id.clone(),
                    template: template.as_ref(),
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
            return Err(GenerationStop::Failed(failures.join("; ")));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for `count` distinct concepts in one call. A reply
    /// that does not parse yields no concepts, and the candidates are
    /// written without them.
    async fn plan_concepts(
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
            self.user_message(&concept_input(&context.brief), attachments),
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
    async fn edit_design(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        design_id: &str,
        critique: &Critique,
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
        let design_json = serde_json::to_string(&design)
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(log).await;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(
                &edit_prompt(&context.brief, critique, &design_json),
                &attachments,
            ),
        ];
        let original = design.clone();
        let effort = context.effort().to_owned();
        let request = DesignRequest {
            effort: effort.clone(),
            label: format!("edit {design_id}"),
            parse: Box::new(move |content| {
                crate::patch::apply_patch(&original, crate::patch::parse_patch(content)?)
            }),
            progress: self.shared_progress(&[design_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_valid_design(client, messages, &request, log)
            .await?;
        // A polish round costs a full-design rewrite; edits get one only
        // at high effort.
        let final_design = if effort == "high" {
            self.polish_design(client, edited, &request, log)
                .await
                .map_err(GenerationStop::Failed)?
        } else {
            edited
        };
        self.designs
            .save(design_id, &final_design)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
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
        context: &GenerationContext,
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
            let context = context.clone();
            let design_id = (*design_id).to_owned();
            let attachments = Arc::clone(&attachments);
            let share = Arc::clone(&shares[index]);
            let log = Arc::clone(log);
            tasks.spawn(async move {
                let outcome = engine
                    .continue_design(&client, &context, &design_id, &attachments, &share, &log)
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
                        &continue_prompt(&context.brief, &preview, &design_json, chunk),
                        &attachments,
                    ),
                ];
                let original = preview.clone();
                let written = preview.screens.len();
                let live_board = Arc::clone(&board);
                let live_show = Arc::clone(&show);
                let request = DesignRequest {
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
                    .request_valid_design(&client, messages, &request, &log)
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
        context: &DesignRequest<'_>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &context.label;
        let rounds = crate::polish::polish_rounds(&context.effort);
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
            context.report(DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / rounds as f32);
        }
        Ok(design)
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
    fn base_url(&self) -> String {
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
        let context = DesignRequest {
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
        let draft = self
            .request_valid_design(client, messages, &context, log)
            .await?;
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

    /// Sends `messages`, parses the design reply, and repairs it through
    /// fix rounds until it validates. A reply that asks for a blocking
    /// detail stops the run with a clarification.
    async fn request_valid_design(
        &self,
        client: &reqwest::Client,
        mut messages: Vec<serde_json::Value>,
        context: &DesignRequest<'_>,
        log: &LogSink,
    ) -> Result<Design, GenerationStop> {
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
         The brief is authoritative. Do not override a confirmed fact. Use an assumption only where the brief has no confirmed fact for the need.\n\
         If the brief lacks a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one design JSON document. No prose, no code fences.",
        rules = CONTENT_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-design.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// A brief question set the model returned instead of a design, when
/// the reply carries a `needs_clarification` object. `Some(Err)` when
/// the object is present but invalid.
fn clarification_request(content: &str) -> Option<Result<BriefQuestionSet, Vec<QuestionSetError>>> {
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
fn stop_to_string(stop: GenerationStop) -> String {
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
            "I wrote `{}`. Review it, then critique it or ask for a change.",
            design_ids[0]
        ),
        count => format!(
            "I wrote {count} candidates to the canvas. Pick one, then critique it or ask for a change."
        ),
    }
}

/// Appends `title: value` when the value is not blank.
fn push_field(input: &mut String, title: &str, value: &str) {
    if !value.trim().is_empty() {
        input.push_str(&format!("{title}: {value}\n"));
    }
}

/// Appends a bullet list under `title` when it has items.
fn push_list(input: &mut String, title: &str, items: &[String]) {
    if !items.is_empty() {
        input.push_str(&format!("{title}:\n"));
        for item in items {
            input.push_str(&format!("- {item}\n"));
        }
    }
}

/// The brief, rendered as labelled sections for a generation prompt.
fn brief_input(brief: &DesignBrief) -> String {
    let mut input = String::new();
    push_field(&mut input, "Request", &brief.request);
    push_field(&mut input, "Target artifact", &brief.target_artifact);
    push_field(&mut input, "Target platform", &brief.target_platform);
    push_field(&mut input, "Audience", &brief.audience);
    push_field(&mut input, "User problem", &brief.user_problem);
    push_field(&mut input, "Primary job", &brief.primary_job);
    push_field(&mut input, "Success criterion", &brief.success_criterion);
    push_field(&mut input, "Visual direction", &brief.visual_direction);
    push_list(&mut input, "Confirmed facts", &brief.confirmed_facts);
    push_list(
        &mut input,
        "Assumptions (use only where a confirmed fact does not cover the need)",
        &brief.assumptions,
    );
    push_list(&mut input, "Open questions", &brief.open_questions);
    push_list(
        &mut input,
        "Information architecture",
        &brief.information_architecture,
    );
    if !brief.required_sections.is_empty() {
        input.push_str("Required sections:\n");
        for section in &brief.required_sections {
            input.push_str(&format!("- {}: {}\n", section.name, section.content));
        }
    }
    push_list(&mut input, "Brand assets", &brief.brand_assets);
    push_list(
        &mut input,
        "Accessibility constraints",
        &brief.accessibility_constraints,
    );
    push_list(
        &mut input,
        "Technical constraints",
        &brief.technical_constraints,
    );
    push_list(
        &mut input,
        "Generation instructions (newest last)",
        &brief.generation_instructions,
    );
    input
}

/// Turns a model reply into a design: a whole design, or a patch applied
/// to an existing one.
type ReplyParser<'request> = Box<dyn Fn(&str) -> Result<Design, String> + Send + Sync + 'request>;

/// Effort, log label, and reply parser for one design request: a
/// candidate (the reply is a design) or an edit (the reply is a patch).
struct DesignRequest<'request> {
    effort: String,
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

/// The user prompt for an edit: the design as it is, the approved
/// brief, and the critique to apply.
fn edit_prompt(brief: &DesignBrief, critique: &Critique, design_json: &str) -> String {
    format!(
        "Here is the design to change:\n{design_json}\n\
         The design is for this approved brief:\n{brief}\n\
         Apply this critique: {critique}\n\
         A reference like [screen 3, node 0/1 <h2.title>: What Swift Design does] names a screen \
         (1-based) and one element in that screen's html by its index path from the screen root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. Change only what the critique asks for. Keep every other screen and \
         value as it is. Return every changed screen complete: html, css, and notes.\n{format}",
        brief = brief_input(brief),
        critique = critique.as_instruction(),
        format = crate::patch::PATCH_FORMAT
    )
}

/// What one candidate call needs: the run context, the candidate number,
/// every concept, so the prompt can name its own and the others, and
/// the preview length when the candidate is a preview.
struct CandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` screens plus the outline.
    preview_screens: Option<usize>,
    /// The id the candidate is saved under.
    design_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
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
    brief: &DesignBrief,
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
         The design is for this approved brief:\n{}\n",
        brief_input(brief)
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

/// The user prompt for one candidate: the approved brief is
/// authoritative, plus the template, preview, concept, and effort notes.
fn candidate_prompt(request: &CandidateRequest<'_>) -> String {
    let brief = &request.context.brief;
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a design for this approved brief. The brief is authoritative; do not override a \
         confirmed fact.\n{}\n",
        brief_input(brief)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
    }
    if let Some(count) = request.preview_screens {
        prompt.push_str(&preview_note(count));
    }
    let count = options.variation_count();
    if count > 1 {
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
    let viewport = brief.viewport();
    prompt.push_str(&format!(
        "Set `viewport` to {} by {} px.\n",
        viewport.width, viewport.height
    ));
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{Critique, CritiqueCategory, DesignBrief, RevisionSource, WorkflowState};

    use super::{GenerationEngine, GenerationOutcome, brief_input, system_prompt};
    use crate::designs::DesignStore;
    use crate::events::ChangeNotifier;
    use crate::model_client::LogSink;
    use crate::sessions::SessionStore;
    use crate::test_support::{FakeModelServer, SAMPLE_DESIGN, low_effort_options};

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

    /// A session in the generating state with a one-candidate,
    /// low-effort brief approved.
    async fn generating_session(sessions: &SessionStore, brief: DesignBrief) {
        sessions
            .create("talk", "Talk", "A landing page.", low_effort_options())
            .await
            .unwrap();
        let revision = sessions
            .write_brief_revision("talk", brief, RevisionSource::Agent, "Drafted")
            .await
            .unwrap();
        sessions
            .update("talk", |session| session.approved_revision = Some(revision))
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerateWithAssumptions)
            .await
            .unwrap();
    }

    #[test]
    fn brief_input_keeps_assumptions_apart_from_facts() {
        let brief = DesignBrief {
            request: "A finance page.".to_owned(),
            confirmed_facts: vec!["Platform: web".to_owned()],
            assumptions: vec!["Audience: investors".to_owned()],
            audience: "investors".to_owned(),
            ..DesignBrief::default()
        };
        let text = brief_input(&brief);
        assert!(text.contains("Confirmed facts:\n- Platform: web"));
        assert!(text.contains("Assumptions (use only where a confirmed fact"));
        assert!(text.contains("- Audience: investors"));
    }

    #[test]
    fn system_prompt_names_the_clarification_protocol() {
        let prompt = system_prompt();
        assert!(prompt.contains("The brief is authoritative"));
        assert!(prompt.contains("needs_clarification"));
        assert!(prompt.contains("\"viewport\""));
    }

    #[tokio::test]
    async fn generation_sends_the_approved_brief_not_the_chat() {
        let server = FakeModelServer::start().await;
        server.push_text(SAMPLE_DESIGN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let brief = DesignBrief {
            audience: "retail investors".to_owned(),
            ..DesignBrief::default()
        };
        generating_session(&sessions, brief).await;
        engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        let request = &server.requests()[0];
        let text = request.to_string();
        assert!(text.contains("retail investors"));
        assert!(text.contains("The brief is authoritative"));
    }

    #[tokio::test]
    async fn a_valid_design_reply_is_saved_as_a_candidate_and_recorded_on_the_run() {
        let server = FakeModelServer::start().await;
        server.push_text(SAMPLE_DESIGN);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        generating_session(&sessions, DesignBrief::default()).await;
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(designs.load("talk-candidate-1").await.unwrap().is_some());
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("succeeded"));
        assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
    }

    #[tokio::test]
    async fn a_needs_clarification_reply_moves_the_session_back_to_clarifying() {
        let server = FakeModelServer::start().await;
        server.push_text(
            r#"{"needs_clarification":{"title":"One thing","message":"Which brand color?","questions":[{"id":"color","label":"Which brand color?","kind":"short_text","required":true}]}}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        generating_session(&sessions, DesignBrief::default()).await;
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
        // Low effort gives two fix rounds: three attempts, all empty.
        for _ in 0..3 {
            server.push_text("{}");
        }
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        generating_session(&sessions, DesignBrief::default()).await;
        let result = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await;
        assert!(result.unwrap_err().contains("fix rounds"));
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("failed"));
    }

    #[tokio::test]
    async fn a_critique_run_patches_the_chosen_design() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
        let directory = tempfile::tempdir().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        designs.save("talk-candidate-1", &design).await.unwrap();
        generating_session(&sessions, DesignBrief::default()).await;
        sessions
            .update("talk", |session| {
                session.chosen_design = Some("talk-candidate-1".to_owned());
                session.pending_critique = Some(crate::sessions::PendingCritique {
                    design: "talk-candidate-1".to_owned(),
                    critique: Critique {
                        category: CritiqueCategory::Structure,
                        text: "Tighten the hero.".to_owned(),
                    },
                });
            })
            .await
            .unwrap();
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
        assert!(edited.screens[0].html.contains("Tighter"));
    }
}
