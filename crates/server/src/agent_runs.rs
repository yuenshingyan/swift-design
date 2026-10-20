//! Generation runs: the app runs the model the user picked for one
//! session.
//!
//! `POST /agent-runs` with `{session_id}` starts a run. The mode comes
//! from the session state: a briefing state runs the briefing engine, a
//! generating state runs the generation engine. No agent CLI is needed;
//! `SWIFT_DESIGN_AGENT_COMMAND` remains as an override for users who
//! want an external command. Output streams into a log served by
//! `GET /agent-runs`, and every change bumps `/events`, so the studio

mod routes;
#[cfg(test)]
mod tests;
pub use routes::routes;
use routes::{RunEnd, mark_running, settle_built_in, settle_shell};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use design_model::{ArtifactKind, WorkflowState};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::AppState;
use crate::api_error;
use crate::artworks::ArtworkStore;
use crate::campaigns::CampaignStore;
use crate::capture::capture_urls;
use crate::decks::DeckStore;
use crate::designs::DesignStore;
use crate::documents::DocumentStore;
use crate::events::ChangeNotifier;
use crate::generation::{DesignProgressSink, GenerationEngine, GenerationOutcome, ProgressSink};
use crate::mailings::MailingStore;
use crate::model_client::{self, LogSink, TokenUsage, UsageSink};
use crate::prints::PrintStore;
use crate::screenshots::{check_chrome, find_chrome};
use crate::sessions::{RunMode, RunRecord, Session, SessionStore, run_mode_for};
use crate::settings::SettingsStore;
use crate::socials::SocialStore;
use crate::templates::TemplateStore;
use crate::time::rfc3339_now;
use crate::uploads::UploadStore;

/// Most log bytes kept in memory. Older output is dropped from the
/// front.
const LOG_LIMIT_BYTES: usize = 64 * 1024;

/// Log bytes returned by `GET /agent-runs`.
const LOG_TAIL_BYTES: usize = 4 * 1024;

/// How a resolved run starts.
enum ResolvedLaunch {
    /// The user's custom shell command.
    Shell(String),
    /// The built-in generation engine.
    Generation(Box<GenerationEngine>),
}

/// Why a run could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// No session with that id.
    #[error("no session `{id}`: create it with POST /sessions")]
    NoSession {
        /// The missing id.
        id: String,
    },
    /// The session state has no run to start.
    #[error("no run for a session in state `{state}`")]
    WrongState {
        /// The session state.
        state: WorkflowState,
    },
    /// A run is already active.
    #[error("a run is already active")]
    AlreadyRunning,
    /// Another start already consumed the waiting prompt.
    #[error("no queued prompt remains")]
    NoQueuedPrompt,
    /// No model is configured.
    #[error("{0}")]
    NotConfigured(String),
    /// A storage failure. No path is named.
    #[error("session storage failed: {0}")]
    Storage(String),
}

impl StartError {
    /// The HTTP status for this error.
    fn status(&self) -> StatusCode {
        match self {
            StartError::NoSession { .. } => StatusCode::NOT_FOUND,
            StartError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::CONFLICT,
        }
    }
}

/// Mutable state of the current (or last) run.
struct RunState {
    log: String,
    is_running: bool,
    is_queue_paused: bool,
    exit_code: Option<i32>,
    active_agent: Option<String>,
    session_id: Option<String>,
    mode: Option<RunMode>,
    stop_sender: Option<oneshot::Sender<()>>,
    /// Input tokens of the latest request: the live context size.
    context_tokens: u64,
    /// Input plus output tokens over the whole run.
    total_tokens: u64,
    /// Context window of the running model, in tokens. 0 when unknown.
    context_window: u64,
    /// How far the current turn is, 0 to 100. `None` until the engine
    /// reports it; the custom command never does.
    progress: Option<u8>,
    /// How far each design the turn writes is, 0 to 100, by design id.
    /// Cleared when a turn starts.
    designs: HashMap<String, u8>,
}

/// Starts and tracks one run at a time.
#[derive(Clone)]
pub struct AgentRunner {
    custom_command: Option<String>,
    settings: SettingsStore,
    designs: DesignStore,
    decks: Option<DeckStore>,
    documents: Option<DocumentStore>,
    socials: Option<SocialStore>,
    prints: Option<PrintStore>,
    mailings: Option<MailingStore>,
    campaigns: Option<CampaignStore>,
    artworks: Option<ArtworkStore>,
    sessions: SessionStore,
    address: String,
    templates: Option<TemplateStore>,
    uploads: Option<UploadStore>,
    state: Arc<Mutex<RunState>>,
    notifier: ChangeNotifier,
}

impl AgentRunner {
    /// Creates a runner. `custom_command` overrides the built-in
    /// engines when set.
    pub fn new(
        custom_command: Option<String>,
        settings: SettingsStore,
        designs: DesignStore,
        sessions: SessionStore,
        address: String,
        notifier: ChangeNotifier,
    ) -> Self {
        Self {
            custom_command,
            settings,
            designs,
            decks: None,
            documents: None,
            socials: None,
            prints: None,
            mailings: None,
            campaigns: None,
            artworks: None,
            sessions,
            address,
            templates: None,
            uploads: None,
            state: Arc::new(Mutex::new(RunState {
                log: String::new(),
                is_running: false,
                is_queue_paused: false,
                exit_code: None,
                active_agent: None,
                session_id: None,
                mode: None,
                stop_sender: None,
                context_tokens: 0,
                total_tokens: 0,
                context_window: 0,
                progress: None,
                designs: HashMap::new(),
            })),
            notifier,
        }
    }

    /// Lets deck sessions write their candidates to the deck store.
    pub fn with_decks(mut self, decks: DeckStore) -> Self {
        self.decks = Some(decks);
        self
    }

    /// Lets document sessions write their candidates to the document
    /// store.
    pub fn with_documents(mut self, documents: DocumentStore) -> Self {
        self.documents = Some(documents);
        self
    }

    /// Lets social sessions write their candidates to the social store.
    pub fn with_socials(mut self, socials: SocialStore) -> Self {
        self.socials = Some(socials);
        self
    }

    /// Lets print sessions write their candidates to the print store.
    pub fn with_prints(mut self, prints: PrintStore) -> Self {
        self.prints = Some(prints);
        self
    }

    /// Lets mailing sessions write their candidates to the mailing
    /// store.
    pub fn with_mailings(mut self, mailings: MailingStore) -> Self {
        self.mailings = Some(mailings);
        self
    }

    /// Lets campaign sessions write their candidates to the campaign
    /// store.
    pub fn with_campaigns(mut self, campaigns: CampaignStore) -> Self {
        self.campaigns = Some(campaigns);
        self
    }

    /// Lets artwork sessions write their candidates to the artwork
    /// store.
    pub fn with_artworks(mut self, artworks: ArtworkStore) -> Self {
        self.artworks = Some(artworks);
        self
    }

    /// Lets runs style their candidates from a saved template.
    pub fn with_templates(mut self, templates: TemplateStore) -> Self {
        self.templates = Some(templates);
        self
    }

    /// Lets runs attach the user's uploads to model requests.
    pub fn with_uploads(mut self, uploads: UploadStore) -> Self {
        self.uploads = Some(uploads);
        self
    }

    fn append_log(state: &Arc<Mutex<RunState>>, notifier: &ChangeNotifier, line: &str) {
        if let Ok(mut state) = state.lock() {
            state.log.push_str(line);
            state.log.push('\n');
            if state.log.len() > LOG_LIMIT_BYTES {
                let cut = state.log.len() - LOG_LIMIT_BYTES;
                state.log.drain(..cut);
            }
        }
        notifier.notify();
    }

    /// The model configuration the user chose, or `None` when nothing
    /// is set.
    async fn model_configuration(&self) -> Option<model_client::ModelConfiguration> {
        match self.settings.read().await {
            Ok(Some(stored)) => model_client::configuration_from_settings(&stored),
            _ => None,
        }
        .or_else(model_client::configured_model)
    }

    /// Picks how to run for `session_id`: the custom command, or the
    /// built-in engine for the session's mode.
    async fn resolve(
        &self,
        _session_id: &str,
        mode: RunMode,
    ) -> Result<(String, ResolvedLaunch), StartError> {
        if let Some(command) = &self.custom_command {
            return Ok(("custom".to_owned(), ResolvedLaunch::Shell(command.clone())));
        }
        let configuration = self.model_configuration().await.ok_or_else(|| {
            StartError::NotConfigured(
                "no model is chosen: pick a model in the studio settings first".to_owned(),
            )
        })?;
        match mode {
            RunMode::Generation => {
                let mut engine = GenerationEngine::new(
                    configuration,
                    self.designs.clone(),
                    self.sessions.clone(),
                    Some(self.settings.clone()),
                    self.address.clone(),
                    self.notifier.clone(),
                );
                if let Some(decks) = &self.decks {
                    engine = engine.with_decks(decks.clone());
                }
                if let Some(documents) = &self.documents {
                    engine = engine.with_documents(documents.clone());
                }
                if let Some(socials) = &self.socials {
                    engine = engine.with_socials(socials.clone());
                }
                if let Some(prints) = &self.prints {
                    engine = engine.with_prints(prints.clone());
                }
                if let Some(mailings) = &self.mailings {
                    engine = engine.with_mailings(mailings.clone());
                }
                if let Some(campaigns) = &self.campaigns {
                    engine = engine.with_campaigns(campaigns.clone());
                }
                if let Some(artworks) = &self.artworks {
                    engine = engine.with_artworks(artworks.clone());
                }
                if let Some(templates) = &self.templates {
                    engine = engine.with_templates(templates.clone());
                }
                if let Some(uploads) = &self.uploads {
                    engine = engine.with_uploads(uploads.clone());
                }
                Ok((engine.label(), ResolvedLaunch::Generation(Box::new(engine))))
            }
        }
    }

    /// Starts a run for `session_id`. The mode comes from the session
    /// state.
    pub async fn start(&self, session_id: &str) -> Result<(), StartError> {
        self.start_turn(session_id, false, true).await
    }

    /// Starts an explicit Finish request without consuming an ordinary queued prompt.
    pub async fn start_continue(&self, session_id: &str) -> Result<(), StartError> {
        self.start_turn(session_id, false, false).await
    }

    async fn start_turn(
        &self,
        session_id: &str,
        is_automatic: bool,
        should_consume: bool,
    ) -> Result<(), StartError> {
        let session = self
            .sessions
            .read(session_id)
            .await
            .map_err(|error| StartError::Storage(error.to_string()))?
            .ok_or_else(|| StartError::NoSession {
                id: session_id.to_owned(),
            })?;
        let mode = run_mode_for(session.state).ok_or(StartError::WrongState {
            state: session.state,
        })?;
        // Freeze whether this start consumes a prompt before reserving the runner.
        // Prompts arriving after the reservation belong to a later turn.
        let has_queued = should_consume
            && self
                .sessions
                .has_queued_messages(session_id)
                .await
                .map_err(|error| StartError::Storage(error.to_string()))?;
        let (name, launch) = self.resolve(session_id, mode).await?;
        let (stop_sender, stop_receiver) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| StartError::Storage("run state lock poisoned".to_owned()))?;
            if state.is_running || (is_automatic && state.is_queue_paused) {
                return Err(StartError::AlreadyRunning);
            }
            state.is_queue_paused = false;
            mark_running(&mut state, &name, session_id, mode, stop_sender);
            if let ResolvedLaunch::Generation(engine) = &launch {
                state.context_window = engine.context_window();
            }
        }
        if let Err(error) = self.prepare_queued_turn(session_id, has_queued).await {
            self.clear_failed_start();
            return Err(error);
        }
        match launch {
            ResolvedLaunch::Shell(command) => {
                self.record_run_start(session_id, mode, "custom").await;
                let process = match self.launch_shell(&command, &session, mode) {
                    Ok(process) => process,
                    Err(error) => {
                        settle_shell(&self.sessions, session_id, mode, Some(1), false).await;
                        self.clear_failed_start();
                        return Err(error);
                    }
                };
                self.spawn_shell_tasks(session_id.to_owned(), mode, process, stop_receiver);
            }
            ResolvedLaunch::Generation(engine) => {
                self.spawn_generation_task(
                    *engine,
                    session_id.to_owned(),
                    session.artifact_kind,
                    stop_receiver,
                );
            }
        }
        self.notifier.notify();
        Ok(())
    }

    async fn prepare_queued_turn(&self, id: &str, has_queued: bool) -> Result<(), StartError> {
        if !has_queued {
            return Ok(());
        }
        let message = self
            .sessions
            .activate_next_message(id)
            .await
            .map_err(|error| StartError::Storage(error.to_string()))?
            .ok_or(StartError::NoQueuedPrompt)?;
        if let Some(uploads) = &self.uploads
            && !message.is_regenerate
        {
            capture_urls(uploads, id, &message.content).await;
        }
        Ok(())
    }

    fn clear_failed_start(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.is_running = false;
            state.stop_sender = None;
        }
        self.notifier.notify();
    }

    fn launch_shell(
        &self,
        command: &str,
        session: &Session,
        mode: RunMode,
    ) -> Result<tokio::process::Child, StartError> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned());
        Command::new(shell)
            .arg("-lc")
            .arg(command)
            .env("SWIFT_DESIGN_SESSION_ID", &session.id)
            .env("SWIFT_DESIGN_RUN_MODE", mode.as_str())
            .env("SWIFT_DESIGN_ARTIFACT_KIND", session.artifact_kind.as_str())
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| {
                StartError::NotConfigured(format!("failed to start the custom command: {error}"))
            })
    }

    /// Schedules pending turns only after the previous run released the runner.
    fn schedule_pending(&self) {
        let runner = self.clone();
        tokio::spawn(async move {
            runner.start_pending().await;
        });
    }

    async fn start_pending(&self) {
        let Ok(sessions) = self.sessions.list().await else {
            return;
        };
        for session in sessions {
            let can_resume = session.state == WorkflowState::Clarifying
                && self
                    .sessions
                    .has_requested_resume(&session.id)
                    .await
                    .unwrap_or(false);
            if !matches!(
                session.state,
                WorkflowState::Reviewing | WorkflowState::Intake
            ) && !can_resume
            {
                continue;
            }
            if !self
                .sessions
                .has_queued_messages(&session.id)
                .await
                .unwrap_or(false)
            {
                continue;
            }
            match self.start_turn(&session.id, true, true).await {
                Ok(()) | Err(StartError::AlreadyRunning) => return,
                Err(error) => {
                    tracing::debug!(session_id = %session.id, %error, "queued turn waits")
                }
            }
        }
    }

    /// Records a run start for an external command, so its history
    /// mirrors the built-in engines.
    async fn record_run_start(&self, session_id: &str, mode: RunMode, runtime: &str) {
        let record = RunRecord {
            run_id: String::new(),
            mode,
            runtime: runtime.to_owned(),
            provider: None,
            model: None,
            started_at: rfc3339_now(),
            finished_at: None,
            result: None,
            error: None,
            artifacts: Vec::new(),
        };
        if let Err(error) = self.sessions.start_run(session_id, record).await {
            tracing::warn!(%error, "recording the custom run failed");
        }
    }

    /// Streams the subprocess output into the log and settles the
    /// session when it exits.
    fn spawn_shell_tasks(
        &self,
        session_id: String,
        mode: RunMode,
        mut process: tokio::process::Child,
        stop_receiver: oneshot::Receiver<()>,
    ) {
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        for stream in [stdout.map(Ok), stderr.map(Err)].into_iter().flatten() {
            let state = Arc::clone(&self.state);
            let notifier = self.notifier.clone();
            match stream {
                Ok(stdout) => {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            Self::append_log(&state, &notifier, &line);
                        }
                    });
                }
                Err(stderr) => {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            Self::append_log(&state, &notifier, &line);
                        }
                    });
                }
            }
        }
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        let sessions = self.sessions.clone();
        let runner = self.clone();
        tokio::spawn(async move {
            let (status, stopped) = tokio::select! {
                status = process.wait() => (status.ok(), false),
                _ = stop_receiver => {
                    let _ = process.start_kill();
                    (process.wait().await.ok(), true)
                }
            };
            let code = status.and_then(|status| status.code());
            // Settle the session before marking the run finished, so a
            // watcher that sees `is_running: false` also sees the final
            // session state.
            settle_shell(&sessions, &session_id, mode, code, stopped).await;
            if let Ok(mut state) = state.lock() {
                state.is_running = false;
                state.exit_code = code;
                state.stop_sender = None;
            }
            notifier.notify();
            runner.schedule_pending();
        });
    }

    /// Runs the generation engine and settles the session.
    fn spawn_generation_task(
        &self,
        engine: GenerationEngine,
        session_id: String,
        kind: ArtifactKind,
        stop_receiver: oneshot::Receiver<()>,
    ) {
        let log = self.log_sink();
        let engine = engine
            .with_usage_sink(self.usage_sink())
            .with_retry_log(Arc::clone(&log))
            .with_progress_sink(self.progress_sink())
            .with_design_progress_sink(self.design_progress_sink());
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        let sessions = self.sessions.clone();
        let runner = self.clone();
        tokio::spawn(async move {
            // Keep preflight inside the cancellable run so failures reach the
            // session UI, including runs started automatically after Create.
            let run = async {
                if kind == ArtifactKind::Demo {
                    log("checking Chrome before model requests");
                    let chrome = find_chrome().ok_or_else(|| {
                        "Cannot start the demo: Chrome is missing; restore the app-managed browser or install Chrome/Chromium and retry.".to_owned()
                    })?;
                    check_chrome(&chrome).await?;
                }
                engine.run(&session_id, Arc::clone(&log)).await
            };
            let end = tokio::select! {
                result = run => {
                    RunEnd::Finished(result.map(|_: GenerationOutcome| ()))
                }
                _ = stop_receiver => RunEnd::Stopped,
            };
            let exit_code = settle_built_in(&sessions, &session_id, end, &log).await;
            if let Ok(mut state) = state.lock() {
                state.is_running = false;
                state.exit_code = Some(exit_code);
                state.stop_sender = None;
            }
            notifier.notify();
            runner.schedule_pending();
        });
    }

    /// A log sink that appends to the run log.
    fn log_sink(&self) -> LogSink {
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        Arc::new(move |line: &str| Self::append_log(&state, &notifier, line))
    }

    /// A usage sink that records the token counts.
    fn usage_sink(&self) -> UsageSink {
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        Arc::new(move |usage: TokenUsage| {
            if let Ok(mut state) = state.lock() {
                state.context_tokens = usage.input_tokens;
                state.total_tokens += usage.input_tokens + usage.output_tokens;
            }
            notifier.notify();
        })
    }

    /// A progress sink that records how far the turn is.
    fn progress_sink(&self) -> ProgressSink {
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        Arc::new(move |percent: u8| {
            if let Ok(mut state) = state.lock() {
                state.progress = Some(percent.min(100));
                if percent == 0 {
                    state.designs.clear();
                }
            }
            notifier.notify();
        })
    }

    /// A per-design progress sink.
    fn design_progress_sink(&self) -> DesignProgressSink {
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        Arc::new(move |design_id: &str, percent: u8| {
            if let Ok(mut state) = state.lock() {
                state.designs.insert(design_id.to_owned(), percent.min(100));
            }
            notifier.notify();
        })
    }

    /// True when a run of `session_id` is in flight right now.
    ///
    /// The session state alone does not answer this: a server that goes
    /// away mid-run leaves `generating` on disk with no run behind it.
    pub fn is_running_session(&self, session_id: &str) -> bool {
        match self.state.lock() {
            Ok(state) => state.is_running && state.session_id.as_deref() == Some(session_id),
            Err(_) => false,
        }
    }

    /// Waits until no run of `session_id` is in flight, or `limit`
    /// passes. Used before a delete, so the run settles before its
    /// session is removed.
    pub async fn wait_until_idle(&self, session_id: &str, limit: std::time::Duration) {
        let deadline = std::time::Instant::now() + limit;
        while self.is_running_session(session_id) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Stops the active run and pauses queued turns, including between runs.
    pub fn stop(&self) {
        if let Ok(mut state) = self.state.lock() {
            // The pause also covers the idle gap between two queued turns.
            state.is_queue_paused = true;
            if let Some(sender) = state.stop_sender.take() {
                let _ = sender.send(());
            }
        }
        self.notifier.notify();
    }

    /// A JSON snapshot of the run for the studio.
    pub fn status(&self) -> serde_json::Value {
        match self.state.lock() {
            Ok(state) => {
                let tail_start = state.log.len().saturating_sub(LOG_TAIL_BYTES);
                let boundary = (tail_start..state.log.len())
                    .find(|index| state.log.is_char_boundary(*index))
                    .unwrap_or(state.log.len());
                serde_json::json!({
                    "is_running": state.is_running,
                    "is_queue_paused": state.is_queue_paused,
                    "exit_code": state.exit_code,
                    "log_tail": &state.log[boundary..],
                    "active_agent": state.active_agent,
                    "session_id": state.session_id,
                    "mode": state.mode.map(|mode| mode.as_str()),
                    "context_tokens": state.context_tokens,
                    "total_tokens": state.total_tokens,
                    "context_window": state.context_window,
                    "progress": state.progress,
                    "designs": state.designs,
                })
            }
            Err(_) => serde_json::json!({
                "is_running": false,
                "is_queue_paused": true,
                "exit_code": null,
                "log_tail": "",
                "active_agent": null,
                "session_id": null,
                "mode": null,
                "context_tokens": 0,
                "total_tokens": 0,
                "context_window": 0,
                "progress": null,
                "designs": {},
            }),
        }
    }
}
