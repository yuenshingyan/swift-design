//! Generation runs: the app runs the model the user picked for one
//! session.
//!
//! `POST /agent-runs` with `{session_id}` starts a run. The mode comes
//! from the session state: a briefing state runs the briefing engine, a
//! generating state runs the generation engine. No agent CLI is needed;
//! `SWIFT_DESIGN_AGENT_COMMAND` remains as an override for users who
//! want an external command. Output streams into a log served by
//! `GET /agent-runs`, and every change bumps `/events`, so the studio
//! shows the run live. All credentials are the user's own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use design_model::WorkflowState;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::api_error;
use crate::designs::DesignStore;
use crate::events::ChangeNotifier;
use crate::generation::{GenerationEngine, GenerationOutcome};
use crate::model_client::{self, TokenUsage};
use crate::sessions::{RunMode, RunRecord, SessionStore, run_mode_for};
use crate::settings::SettingsStore;

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
    decks: Option<crate::decks::DeckStore>,
    documents: Option<crate::documents::DocumentStore>,
    socials: Option<crate::socials::SocialStore>,
    sessions: SessionStore,
    address: String,
    templates: Option<crate::templates::TemplateStore>,
    uploads: Option<crate::uploads::UploadStore>,
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
            sessions,
            address,
            templates: None,
            uploads: None,
            state: Arc::new(Mutex::new(RunState {
                log: String::new(),
                is_running: false,
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
    pub fn with_decks(mut self, decks: crate::decks::DeckStore) -> Self {
        self.decks = Some(decks);
        self
    }

    /// Lets document sessions write their candidates to the document
    /// store.
    pub fn with_documents(mut self, documents: crate::documents::DocumentStore) -> Self {
        self.documents = Some(documents);
        self
    }

    /// Lets social sessions write their candidates to the social store.
    pub fn with_socials(mut self, socials: crate::socials::SocialStore) -> Self {
        self.socials = Some(socials);
        self
    }

    /// Lets runs style their candidates from a saved template.
    pub fn with_templates(mut self, templates: crate::templates::TemplateStore) -> Self {
        self.templates = Some(templates);
        self
    }

    /// Lets runs attach the user's uploads to model requests.
    pub fn with_uploads(mut self, uploads: crate::uploads::UploadStore) -> Self {
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
        let (name, launch) = self.resolve(session_id, mode).await?;
        // A shell command is spawned before the lock so the guard never
        // crosses an await.
        let mut shell_process = None;
        if let ResolvedLaunch::Shell(command) = &launch {
            {
                let state = self
                    .state
                    .lock()
                    .map_err(|_| StartError::Storage("run state lock poisoned".to_owned()))?;
                if state.is_running {
                    return Err(StartError::AlreadyRunning);
                }
            }
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned());
            let process = Command::new(shell)
                .arg("-lc")
                .arg(command)
                .env("SWIFT_DESIGN_SESSION_ID", session_id)
                .env("SWIFT_DESIGN_RUN_MODE", mode.as_str())
                .env("SWIFT_DESIGN_ARTIFACT_KIND", session.artifact_kind.as_str())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|error| {
                    StartError::NotConfigured(format!(
                        "failed to start the custom command: {error}"
                    ))
                })?;
            self.record_run_start(session_id, mode, "custom").await;
            shell_process = Some(process);
        }
        let (stop_sender, stop_receiver) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| StartError::Storage("run state lock poisoned".to_owned()))?;
            if state.is_running {
                return Err(StartError::AlreadyRunning);
            }
            mark_running(&mut state, &name, session_id, mode, stop_sender);
            if let ResolvedLaunch::Generation(engine) = &launch {
                state.context_window = engine.context_window();
            }
        }
        match launch {
            ResolvedLaunch::Shell(_) => {
                if let Some(process) = shell_process.take() {
                    self.spawn_shell_tasks(session_id.to_owned(), mode, process, stop_receiver);
                }
            }
            ResolvedLaunch::Generation(engine) => {
                self.spawn_generation_task(*engine, session_id.to_owned(), stop_receiver);
            }
        }
        self.notifier.notify();
        Ok(())
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
            started_at: crate::time::rfc3339_now(),
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
        });
    }

    /// Runs the generation engine and settles the session.
    fn spawn_generation_task(
        &self,
        engine: GenerationEngine,
        session_id: String,
        stop_receiver: oneshot::Receiver<()>,
    ) {
        let log = self.log_sink();
        let engine = engine
            .with_usage_sink(self.usage_sink())
            .with_progress_sink(self.progress_sink())
            .with_design_progress_sink(self.design_progress_sink());
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            let end = tokio::select! {
                result = engine.run(&session_id, Arc::clone(&log)) => {
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
        });
    }

    /// A log sink that appends to the run log.
    fn log_sink(&self) -> crate::model_client::LogSink {
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        Arc::new(move |line: &str| Self::append_log(&state, &notifier, line))
    }

    /// A usage sink that records the token counts.
    fn usage_sink(&self) -> crate::model_client::UsageSink {
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
    fn progress_sink(&self) -> crate::generation::ProgressSink {
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
    fn design_progress_sink(&self) -> crate::generation::DesignProgressSink {
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

    /// Stops the active run. Does nothing when no run is active.
    pub fn stop(&self) {
        if let Ok(mut state) = self.state.lock()
            && let Some(sender) = state.stop_sender.take()
        {
            let _ = sender.send(());
        }
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

/// How a run ended.
///
/// A stop is not a failure, so the two are kept apart all the way to
/// the state machine: a stop leaves the session `stopped`, a failure
/// leaves it `error`.
enum RunEnd {
    /// The run ran to its own end. `Err` carries the failure message.
    Finished(Result<(), String>),
    /// The user pressed stop, or the run was cut short before it
    /// finished. A dropped stop channel reads as this too, so a server
    /// that goes away mid-run leaves the session stopped, not failed.
    Stopped,
}

/// Halts a session that is still running, recording why. A session that
/// already halted keeps the first reason.
async fn halt(sessions: &SessionStore, session_id: &str, message: Option<String>) {
    let Ok(Some(session)) = sessions.read(session_id).await else {
        return;
    };
    if session.state.is_halted() {
        return;
    }
    let event = match message {
        Some(_) => design_model::WorkflowEvent::RunFailed,
        None => design_model::WorkflowEvent::RunStopped,
    };
    let _ = sessions.apply(session_id, event).await;
    // A stop has nothing to report, so the failure message stays unset
    // and the studio shows the plain stopped card.
    if let Some(message) = message {
        let _ = sessions
            .update(session_id, |session| session.error = Some(message))
            .await;
    }
}

/// Settles the session after a built-in run. Returns the exit code:
/// 0 on success, 1 on failure. A generation success is already recorded
/// by the engine only for questions; the run moves to reviewing here.
async fn settle_built_in(
    sessions: &SessionStore,
    session_id: &str,
    end: RunEnd,
    log: &crate::model_client::LogSink,
) -> i32 {
    let outcome = match end {
        RunEnd::Stopped => {
            log("stopped by the user");
            halt(sessions, session_id, None).await;
            return 1;
        }
        RunEnd::Finished(outcome) => outcome,
    };
    match outcome {
        Ok(()) => {
            // The engine leaves the session generating on a written
            // artifact; move it to reviewing. Questions and replies
            // already set the state.
            if let Ok(Some(session)) = sessions.read(session_id).await
                && session.state == WorkflowState::Generating
            {
                let _ = sessions
                    .apply(session_id, design_model::WorkflowEvent::GenerationSucceeded)
                    .await;
            }
            0
        }
        Err(message) => {
            log(&format!("error: {message}"));
            halt(sessions, session_id, Some(format!("built-in: {message}"))).await;
            1
        }
    }
}

/// Settles the session after an external command exits.
async fn settle_shell(
    sessions: &SessionStore,
    session_id: &str,
    mode: RunMode,
    code: Option<i32>,
    stopped: bool,
) {
    let Ok(Some(session)) = sessions.read(session_id).await else {
        return;
    };
    // A killed command reports whatever code the signal left behind, so
    // the stop flag decides, not the code.
    if stopped {
        halt(sessions, session_id, None).await;
        return;
    }
    match code {
        Some(0) => {
            if mode == RunMode::Generation && session.state == WorkflowState::Generating {
                let _ = sessions
                    .apply(session_id, design_model::WorkflowEvent::GenerationSucceeded)
                    .await;
            }
        }
        other => {
            let code = other.unwrap_or(-1);
            let message =
                format!("the custom command exited with {code}: check SWIFT_DESIGN_AGENT_COMMAND");
            halt(sessions, session_id, Some(message)).await;
        }
    }
}

/// Marks the run state as active for `session_id` with a fresh log.
fn mark_running(
    state: &mut RunState,
    name: &str,
    session_id: &str,
    mode: RunMode,
    stop_sender: oneshot::Sender<()>,
) {
    state.is_running = true;
    state.exit_code = None;
    state.log.clear();
    state.active_agent = Some(name.to_owned());
    state.session_id = Some(session_id.to_owned());
    state.mode = Some(mode);
    state.stop_sender = Some(stop_sender);
    state.context_tokens = 0;
    state.total_tokens = 0;
    state.context_window = 0;
    state.progress = None;
    state.designs.clear();
}

/// Body of `POST /agent-runs`.
#[derive(Debug, Deserialize)]
struct StartRequest {
    session_id: String,
}

/// The `/agent-runs` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new().route("/agent-runs", get(get_run).post(start_run).delete(stop_run))
}

/// Reports the current run: state, exit code, and the log tail.
async fn get_run(State(runner): State<AgentRunner>) -> Response {
    Json(runner.status()).into_response()
}

/// Starts a run for the named session.
async fn start_run(
    State(runner): State<AgentRunner>,
    Json(request): Json<StartRequest>,
) -> Response {
    match runner.start(&request.session_id).await {
        Ok(()) => {
            tracing::info!(session_id = %request.session_id, "run started");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => api_error::error_response(error.status(), &error.to_string(), Vec::new()),
    }
}

/// Stops the run.
async fn stop_run(State(runner): State<AgentRunner>) -> Response {
    runner.stop();
    tracing::info!("run stop requested");
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use crate::agent_runs::AgentRunner;
    use crate::designs::DesignStore;
    use crate::events::ChangeNotifier;
    use crate::sessions::{NewSession, SessionStore};
    use crate::settings::SettingsStore;

    fn command_runner(directory: &TempDir, command: &str) -> (AgentRunner, SessionStore) {
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let runner = AgentRunner::new(
            Some(command.to_owned()),
            SettingsStore::new(
                directory.path().join("settings.json"),
                "127.0.0.1:3000".to_owned(),
            ),
            DesignStore::new(directory.path().join("designs")),
            sessions.clone(),
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        (runner, sessions)
    }

    async fn wait_until_finished(runner: &AgentRunner) -> serde_json::Value {
        for _ in 0..100 {
            let status = runner.status();
            if status["is_running"] == false {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        runner.status()
    }

    #[tokio::test]
    async fn a_custom_command_run_captures_output_and_exit_code() {
        let directory = TempDir::new().unwrap();
        let (runner, sessions) = command_runner(&directory, "echo custom-agent");
        sessions
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        runner.start("talk").await.unwrap();
        let status = wait_until_finished(&runner).await;
        assert_eq!(status["exit_code"], 0);
        assert_eq!(status["active_agent"], "custom");
        assert_eq!(status["mode"], "generation");
        assert!(
            status["log_tail"]
                .as_str()
                .unwrap()
                .contains("custom-agent")
        );
    }

    #[tokio::test]
    async fn a_second_start_while_running_is_rejected() {
        let directory = TempDir::new().unwrap();
        let (runner, sessions) = command_runner(&directory, "sleep 5");
        sessions
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        runner.start("talk").await.unwrap();
        assert!(runner.start("talk").await.is_err());
        runner.stop();
        let status = wait_until_finished(&runner).await;
        assert_eq!(status["is_running"], false);
    }

    #[tokio::test]
    async fn a_run_is_refused_when_the_session_is_in_error() {
        let directory = TempDir::new().unwrap();
        let (runner, sessions) = command_runner(&directory, "echo hi");
        sessions
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::RunFailed)
            .await
            .unwrap();
        assert!(runner.start("talk").await.is_err());
    }

    #[tokio::test]
    async fn a_missing_session_is_reported_as_not_found() {
        let directory = TempDir::new().unwrap();
        let (runner, _sessions) = command_runner(&directory, "echo hi");
        let error = runner.start("missing").await.unwrap_err();
        assert_eq!(error.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stopping_a_command_leaves_the_session_stopped_not_failed() {
        let directory = TempDir::new().unwrap();
        // A command long enough to still be running when stop arrives.
        let (runner, sessions) = command_runner(&directory, "sleep 30");
        sessions
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        runner.start("talk").await.unwrap();
        runner.stop();
        wait_until_finished(&runner).await;
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, design_model::WorkflowState::Stopped);
        // A stop is not a failure, so nothing is reported and the run
        // resumes where it left off.
        assert_eq!(session.error, None);
        assert_eq!(
            session.resume_state,
            Some(design_model::WorkflowState::Generating)
        );
    }

    #[tokio::test]
    async fn a_failing_command_moves_the_session_to_error_and_names_the_runtime() {
        let directory = TempDir::new().unwrap();
        let (runner, sessions) = command_runner(&directory, "exit 3");
        sessions
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        runner.start("talk").await.unwrap();
        wait_until_finished(&runner).await;
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, design_model::WorkflowState::Error);
        assert!(
            session
                .error
                .unwrap()
                .contains("SWIFT_DESIGN_AGENT_COMMAND")
        );
    }

    #[tokio::test]
    async fn a_zero_exit_in_generating_moves_the_session_to_reviewing() {
        let directory = TempDir::new().unwrap();
        let (runner, sessions) = command_runner(&directory, "echo done");
        sessions
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        runner.start("talk").await.unwrap();
        wait_until_finished(&runner).await;
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, design_model::WorkflowState::Reviewing);
    }
}
