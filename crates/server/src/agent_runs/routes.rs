use super::*;

/// How a run ended.
///
/// A stop is not a failure, so the two are kept apart all the way to
/// the state machine: a stop leaves the session `stopped`, a failure
/// leaves it `error`.
pub(super) enum RunEnd {
    /// The run ran to its own end. `Err` carries the failure message.
    Finished(Result<(), String>),
    /// The user pressed stop, or the run was cut short before it
    /// finished. A dropped stop channel reads as this too, so a server
    /// that goes away mid-run leaves the session stopped, not failed.
    Stopped,
}

/// Halts a session that is still running, recording why. A session that
/// already halted keeps the first reason.
pub(super) async fn halt(sessions: &SessionStore, session_id: &str, message: Option<String>) {
    let Ok(Some(session)) = sessions.read(session_id).await else {
        return;
    };
    if session.state.is_halted() {
        return;
    }
    if let Ok(records) = sessions.runs(session_id).await
        && let Some(record) = records.last().filter(|record| record.finished_at.is_none())
    {
        let result = if message.is_some() {
            "failed"
        } else {
            "stopped"
        };
        let _ = sessions
            .finish_run(
                session_id,
                &record.run_id,
                result,
                message.clone(),
                Vec::new(),
            )
            .await;
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
pub(super) async fn settle_built_in(
    sessions: &SessionStore,
    session_id: &str,
    end: RunEnd,
    log: &LogSink,
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
pub(super) async fn settle_shell(
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
            if let Ok(records) = sessions.runs(session_id).await
                && let Some(record) = records.last()
            {
                let _ = sessions
                    .finish_run(session_id, &record.run_id, "succeeded", None, Vec::new())
                    .await;
            }
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
pub(super) fn mark_running(
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
pub(super) struct StartRequest {
    session_id: String,
}

/// The `/agent-runs` route table.
pub fn routes() -> Router<AppState> {
    Router::new().route("/agent-runs", get(get_run).post(start_run).delete(stop_run))
}

/// Reports the current run: state, exit code, and the log tail.
pub(super) async fn get_run(State(runner): State<AgentRunner>) -> Response {
    Json(runner.status()).into_response()
}

/// Starts a run for the named session.
pub(super) async fn start_run(
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
pub(super) async fn stop_run(State(runner): State<AgentRunner>) -> Response {
    runner.stop();
    tracing::info!("run stop requested");
    StatusCode::NO_CONTENT.into_response()
}
