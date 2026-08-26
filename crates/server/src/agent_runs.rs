//! Generation runs: the app runs the model the user picked.
//!
//! `POST /agent-runs` starts a run with the settings from the studio
//! (provider, model, API key or Claude login). No agent CLI is needed
//! or launched; `SWIFT_DESIGN_AGENT_COMMAND` remains as an override for
//! users who want an external command instead. Output streams into a
//! log served by `GET /agent-runs`, and every change bumps `/events`,
//! so the studio shows the run live. All credentials are the user's
//! own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::api_error;
use crate::briefs::BriefStore;
use crate::designs::DesignStore;
use crate::events::ChangeNotifier;
use crate::generation::{self, GenerationEngine, TokenUsage};
use crate::questions::QuestionStore;
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
    /// The built-in engine with the chosen model.
    BuiltIn(Box<GenerationEngine>),
}

/// Mutable state of the current (or last) run.
struct RunState {
    log: String,
    is_running: bool,
    exit_code: Option<i32>,
    active_agent: Option<String>,
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

/// Starts and tracks one generation run at a time.
#[derive(Clone)]
pub struct AgentRunner {
    custom_command: Option<String>,
    settings: SettingsStore,
    designs: DesignStore,
    briefs: BriefStore,
    questions: QuestionStore,
    templates: Option<crate::templates::TemplateStore>,
    uploads: Option<crate::uploads::UploadStore>,
    state: Arc<Mutex<RunState>>,
    notifier: ChangeNotifier,
}

impl AgentRunner {
    /// Creates a runner. `custom_command` overrides the built-in
    /// engine when set.
    pub fn new(
        custom_command: Option<String>,
        settings: SettingsStore,
        designs: DesignStore,
        briefs: BriefStore,
        questions: QuestionStore,
        notifier: ChangeNotifier,
    ) -> Self {
        Self {
            custom_command,
            settings,
            designs,
            briefs,
            questions,
            templates: None,
            uploads: None,
            state: Arc::new(Mutex::new(RunState {
                log: String::new(),
                is_running: false,
                exit_code: None,
                active_agent: None,
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

    /// Picks how to run: the custom command, the studio settings, or
    /// the environment configuration, in that order.
    async fn resolve(&self) -> Result<(String, ResolvedLaunch), String> {
        if let Some(command) = &self.custom_command {
            return Ok(("custom".to_owned(), ResolvedLaunch::Shell(command.clone())));
        }
        let configuration = match self.settings.read().await {
            Ok(Some(stored)) => generation::configuration_from_settings(&stored),
            _ => None,
        }
        .or_else(generation::configured_model);
        let Some(configuration) = configuration else {
            return Err("no model is chosen: pick a model in the studio settings first".to_owned());
        };
        let mut engine = GenerationEngine::new(
            configuration,
            self.designs.clone(),
            self.briefs.clone(),
            self.questions.clone(),
            Some(self.settings.clone()),
            self.notifier.clone(),
        );
        if let Some(templates) = &self.templates {
            engine = engine.with_templates(templates.clone());
        }
        if let Some(uploads) = &self.uploads {
            engine = engine.with_uploads(uploads.clone());
        }
        Ok((engine.label(), ResolvedLaunch::BuiltIn(Box::new(engine))))
    }

    /// Starts a run. Errors when one is already active or nothing is
    /// configured.
    pub async fn start(&self) -> Result<(), String> {
        let (name, launch) = self.resolve().await?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run state lock poisoned".to_owned())?;
        if state.is_running {
            return Err("a run is already active".to_owned());
        }
        let (stop_sender, stop_receiver) = oneshot::channel();
        match launch {
            ResolvedLaunch::Shell(command) => {
                // A login shell, so the user's PATH additions apply no
                // matter how the server started.
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned());
                let process = Command::new(shell)
                    .arg("-lc")
                    .arg(&command)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|error| format!("failed to start the custom command: {error}"))?;
                mark_running(&mut state, &name, stop_sender);
                drop(state);
                self.spawn_shell_tasks(process, stop_receiver);
            }
            ResolvedLaunch::BuiltIn(engine) => {
                mark_running(&mut state, &name, stop_sender);
                state.context_window = engine.context_window();
                drop(state);
                self.spawn_built_in_task(*engine, stop_receiver);
            }
        }
        self.notifier.notify();
        Ok(())
    }

    /// Streams the subprocess output into the log and records its exit.
    fn spawn_shell_tasks(
        &self,
        mut process: tokio::process::Child,
        stop_receiver: oneshot::Receiver<()>,
    ) {
        let stdout = process.stdout.take();
        let stderr = process.stderr.take();
        if let Some(stdout) = stdout {
            let state = Arc::clone(&self.state);
            let notifier = self.notifier.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    Self::append_log(&state, &notifier, &line);
                }
            });
        }
        if let Some(stderr) = stderr {
            let state = Arc::clone(&self.state);
            let notifier = self.notifier.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    Self::append_log(&state, &notifier, &line);
                }
            });
        }
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        tokio::spawn(async move {
            let status = tokio::select! {
                status = process.wait() => status.ok(),
                _ = stop_receiver => {
                    let _ = process.start_kill();
                    process.wait().await.ok()
                }
            };
            if let Ok(mut state) = state.lock() {
                state.is_running = false;
                state.exit_code = status.and_then(|status| status.code());
                state.stop_sender = None;
            }
            notifier.notify();
        });
    }

    /// Runs the built-in engine and records its result like a process
    /// exit: 0 on success, 1 on failure.
    fn spawn_built_in_task(&self, engine: GenerationEngine, stop_receiver: oneshot::Receiver<()>) {
        let log_state = Arc::clone(&self.state);
        let log_notifier = self.notifier.clone();
        let log: crate::generation::LogSink =
            Arc::new(move |line: &str| Self::append_log(&log_state, &log_notifier, line));
        let usage_state = Arc::clone(&self.state);
        let usage_notifier = self.notifier.clone();
        let usage: crate::generation::UsageSink = Arc::new(move |usage: TokenUsage| {
            if let Ok(mut state) = usage_state.lock() {
                state.context_tokens = usage.input_tokens;
                state.total_tokens += usage.input_tokens + usage.output_tokens;
            }
            usage_notifier.notify();
        });
        let progress_state = Arc::clone(&self.state);
        let progress_notifier = self.notifier.clone();
        let progress: crate::generation::ProgressSink = Arc::new(move |percent: u8| {
            if let Ok(mut state) = progress_state.lock() {
                state.progress = Some(percent.min(100));
                // A turn starts at 0: the per-design bars of the previous
                // turn are done.
                if percent == 0 {
                    state.designs.clear();
                }
            }
            progress_notifier.notify();
        });
        let design_state = Arc::clone(&self.state);
        let design_notifier = self.notifier.clone();
        let design_progress: crate::generation::DesignProgressSink =
            Arc::new(move |design_id: &str, percent: u8| {
                if let Ok(mut state) = design_state.lock() {
                    state.designs.insert(design_id.to_owned(), percent.min(100));
                }
                design_notifier.notify();
            });
        let engine = engine
            .with_usage_sink(usage)
            .with_progress_sink(progress)
            .with_design_progress_sink(design_progress);
        let state = Arc::clone(&self.state);
        let notifier = self.notifier.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                result = engine.run(Arc::clone(&log)) => result,
                _ = stop_receiver => Err("stopped by the user".to_owned()),
            };
            let exit_code = match result {
                Ok(()) => 0,
                Err(message) => {
                    log(&format!("error: {message}"));
                    1
                }
            };
            if let Ok(mut state) = state.lock() {
                state.is_running = false;
                state.exit_code = Some(exit_code);
                state.stop_sender = None;
            }
            notifier.notify();
        });
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
                "context_tokens": 0,
                "total_tokens": 0,
                "context_window": 0,
                "progress": null,
                "designs": {},
            }),
        }
    }
}

/// Marks the run state as active under `name` with a fresh log.
fn mark_running(state: &mut RunState, name: &str, stop_sender: oneshot::Sender<()>) {
    state.is_running = true;
    state.exit_code = None;
    state.log.clear();
    state.active_agent = Some(name.to_owned());
    state.stop_sender = Some(stop_sender);
    state.context_tokens = 0;
    state.total_tokens = 0;
    state.context_window = 0;
    state.progress = None;
    state.designs.clear();
}

/// The `/agent-runs` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new().route("/agent-runs", get(get_run).post(start_run).delete(stop_run))
}

/// Reports the current run: state, exit code, and the log tail.
async fn get_run(State(runner): State<AgentRunner>) -> Response {
    Json(runner.status()).into_response()
}

/// Starts a run with the current settings.
async fn start_run(State(runner): State<AgentRunner>) -> Response {
    match runner.start().await {
        Ok(()) => {
            tracing::info!("run started");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(message) => api_error::error_response(StatusCode::CONFLICT, &message, Vec::new()),
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
    use crate::briefs::BriefStore;
    use crate::designs::DesignStore;
    use crate::events::ChangeNotifier;
    use crate::settings::SettingsStore;

    fn command_runner(directory: &TempDir, command: &str) -> AgentRunner {
        AgentRunner::new(
            Some(command.to_owned()),
            SettingsStore::new(
                directory.path().join("settings.json"),
                "127.0.0.1:3000".to_owned(),
            ),
            DesignStore::new(directory.path().join("designs")),
            BriefStore::new(directory.path().join("brief.json")),
            crate::questions::QuestionStore::new(directory.path().join("questions.json")),
            ChangeNotifier::new(),
        )
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
        let runner = command_runner(&directory, "echo custom-agent");
        runner.start().await.unwrap();
        let status = wait_until_finished(&runner).await;
        assert_eq!(status["exit_code"], 0);
        assert_eq!(status["active_agent"], "custom");
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
        let runner = command_runner(&directory, "sleep 5");
        runner.start().await.unwrap();
        assert!(runner.start().await.is_err());
        runner.stop();
        let status = wait_until_finished(&runner).await;
        assert_eq!(status["is_running"], false);
    }
}
