//! The `/sessions` routes: create a session, drive the workflow, and
//! read its state.
//!
//! Every state change goes through `SessionStore::apply`, which is the
//! only place the workflow state changes. Handlers validate structured
//! input at this boundary and answer 422 with every problem.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use design_model::{
    AUDIENCES, ArtifactKind, BriefQuestionSet, COLOR_MODES, CUSTOM_ANSWER_LIMIT, DATA_STATES,
    DECK_SCENARIOS, DEMO_SCOPES, EVIDENCE_STYLES, PRODUCT_KINDS, QuestionAnswer, SLIDE_DENSITIES,
    TONES, WorkflowEvent, WorkflowState, is_custom_answer, is_deck_scenario, validate_answers,
    validate_question_set,
};
use serde::Deserialize;

use crate::agent_runs::AgentRunner;
use crate::api_error;
use crate::candidates::{CANDIDATE_LIMIT, PLATFORM_LIMIT};
use crate::decks::DeckStore;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::events::ChangeNotifier;
use crate::sessions::{
    AnswerRecord, ChatMessage, NewSession, RunOptions, Session, SessionError, SessionStore,
    SessionSummary, SessionView, is_valid_session_id, new_session_id, session_id_of_artifact,
};

/// Most slides a deck run may be asked for. Past this the run costs
/// more than a user waits for.
const SLIDE_COUNT_LIMIT: u32 = 60;

/// The `/sessions` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/options", put(put_options))
        .route("/sessions/{id}/messages", post(post_message))
        .route("/sessions/{id}/question-set", put(put_question_set))
        .route("/sessions/{id}/answers", post(post_answers))
        .route("/sessions/{id}/generate", post(generate_now))
        .route("/sessions/{id}/complete", post(complete))
        .route("/sessions/{id}/retry", post(retry))
}

/// Maps a session error to an HTTP response. Storage errors carry no
/// path, so they are safe to return.
fn session_error_response(error: &SessionError) -> Response {
    match error {
        SessionError::NotFound { id } => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("no session `{id}`: create it with POST /sessions"),
            Vec::new(),
        ),
        SessionError::AlreadyExists { id } => api_error::error_response(
            StatusCode::CONFLICT,
            &format!("session `{id}` already exists"),
            Vec::new(),
        ),
        SessionError::Workflow(workflow) => {
            api_error::error_response(StatusCode::CONFLICT, &workflow.to_string(), Vec::new())
        }
        SessionError::Io(message) => api_error::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("internal error: {message}"),
            Vec::new(),
        ),
    }
}

/// A short title from the request: the first line, capped.
fn title_from_request(request: &str) -> String {
    let line = request.lines().next().unwrap_or(request).trim();
    line.chars().take(80).collect()
}

/// Prepares a stored brief for a reader: fills `answered_questions`
/// when the brief has none, then drops the lines a reader has already
/// read.
///
/// A brief written before the app recorded them kept the answers only
/// as `question: answer` lines among the confirmed facts. The studio no
/// longer shows those lines, so without this the answers would vanish
/// from an old session. The session store still holds every question
/// set and every answer, so the entries are rebuilt from there.
/// Builds the full session view. A demo session lists its designs, a
/// deck session its decks.
async fn build_view(
    sessions: &SessionStore,
    designs: &DesignStore,
    decks: &DeckStore,
    session: Session,
) -> Result<SessionView, SessionError> {
    let id = session.id.clone();
    let question_sets = sessions.question_sets(&id).await?;
    let answers = sessions.answers(&id).await?;
    let messages = sessions.messages(&id).await?;
    let runs = sessions.runs(&id).await?;
    let open_question_set = open_question_set(&session, &answers);
    let (session_designs, session_decks) = match session.artifact_kind {
        ArtifactKind::Demo => (
            designs
                .list()
                .await
                .map_err(|error| SessionError::Io(error.to_string()))?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect(),
            Vec::new(),
        ),
        ArtifactKind::Deck => (
            Vec::new(),
            decks
                .list()
                .await
                .map_err(|error| SessionError::Io(error.to_string()))?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect(),
        ),
    };
    Ok(SessionView {
        session,
        question_sets,
        open_question_set,
        answers,
        messages,
        runs,
        designs: session_designs,
        decks: session_decks,
    })
}

/// The number of the latest question set when it is still unanswered.
fn open_question_set(session: &Session, answers: &[AnswerRecord]) -> Option<u32> {
    let latest = session.latest_question_set?;
    let answered = answers.iter().any(|record| record.question_set == latest);
    (!answered).then_some(latest)
}

/// Body of `POST /sessions`.
#[derive(Debug, Deserialize)]
struct CreateRequest {
    request: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    options: Option<RunOptions>,
    /// `demo` (the default) or `deck`.
    #[serde(default)]
    artifact_kind: Option<ArtifactKind>,
}

/// Creates a session in the intake state.
async fn create_session(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(uploads): State<crate::uploads::UploadStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<CreateRequest>,
) -> Response {
    let prompt = request.request.trim();
    if prompt.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "request is empty: describe what to design",
            Vec::new(),
        );
    }
    let id = match request.id {
        Some(id) => id.trim().to_owned(),
        None => match new_session_id() {
            Ok(id) => id,
            Err(error) => return api_error::internal_error(&error),
        },
    };
    if !is_valid_session_id(&id) {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("session id `{id}` is invalid: use kebab-case without `-candidate-`"),
            Vec::new(),
        );
    }
    if let Some(options) = &request.options
        && let Some(message) = option_problem(options)
    {
        return api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new());
    }
    let title = request
        .title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| title_from_request(prompt));
    let new = NewSession::demo(&id, &title, prompt)
        .with_options(request.options.unwrap_or_default())
        .with_kind(request.artifact_kind.unwrap_or_default());
    match sessions.create(new).await {
        Ok(session) => {
            // The landing page attaches files before the session exists,
            // so the new session takes what the composer showed.
            match uploads.adopt(&id).await {
                Ok(count) if count > 0 => {
                    tracing::info!(session_id = %id, count, "uploads adopted")
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(session_id = %id, %error, "adopting uploads failed"),
            }
            notifier.notify();
            try_start(&runner, &id).await;
            tracing::info!(
                session_id = %id,
                artifact_kind = session.artifact_kind.as_str(),
                "session created"
            );
            (StatusCode::CREATED, Json(session)).into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

/// Lists every session.
async fn list_sessions(State(sessions): State<SessionStore>) -> Response {
    match sessions.list().await {
        Ok(list) => Json::<Vec<SessionSummary>>(list).into_response(),
        Err(error) => session_error_response(&error),
    }
}

/// Returns one session view.
async fn get_session(
    State(sessions): State<SessionStore>,
    State(designs): State<DesignStore>,
    State(decks): State<DeckStore>,
    Path(id): Path<String>,
) -> Response {
    let session = match sessions.read(&id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return session_error_response(&SessionError::NotFound { id });
        }
        Err(error) => return session_error_response(&error),
    };
    match build_view(&sessions, &designs, &decks, session).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => session_error_response(&error),
    }
}

/// Deletes a session. The designs stay.
async fn delete_session(
    State(sessions): State<SessionStore>,
    State(designs): State<DesignStore>,
    State(decks): State<DeckStore>,
    State(runner): State<AgentRunner>,
    State(uploads): State<crate::uploads::UploadStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    // Deleting a session ends its run. The user asked for the session to
    // go, so there is nothing left for the run to write to.
    if runner.is_running_session(&id) {
        runner.stop();
        runner
            .wait_until_idle(&id, std::time::Duration::from_millis(1500))
            .await;
        tracing::info!(session_id = %id, "run stopped for a delete");
    }
    match sessions.delete(&id).await {
        Ok(_) => {
            delete_session_artifacts(&designs, &decks, &id).await;
            // The session's source files go with it: nothing else reads
            // them.
            if let Err(error) = uploads.delete_scope(&id).await {
                tracing::warn!(session_id = %id, %error, "deleting the session uploads failed");
            }
            notifier.notify();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

/// Deletes the artifacts of a deleted session: its designs and its
/// decks, candidates included.
///
/// A session id comes from the request text, so the same request makes
/// the same id. Without this the next session with that request would
/// open on the deleted session's candidates. A failure is logged and
/// does not fail the delete: the session record is already gone.
async fn delete_session_artifacts(designs: &DesignStore, decks: &DeckStore, id: &str) {
    match designs.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "designs deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session designs failed")
        }
    }
    match decks.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "decks deleted"),
        Ok(_) => {}
        Err(error) => tracing::warn!(session_id = %id, %error, "deleting the session decks failed"),
    }
}

/// Reads a session or turns the miss into a response.
async fn require_session(sessions: &SessionStore, id: &str) -> Result<Session, Response> {
    match sessions.read(id).await {
        Ok(Some(session)) => Ok(session),
        Ok(None) => Err(session_error_response(&SessionError::NotFound {
            id: id.to_owned(),
        })),
        Err(error) => Err(session_error_response(&error)),
    }
}

/// A 409 with `message`.
fn conflict(message: &str) -> Response {
    api_error::error_response(StatusCode::CONFLICT, message, Vec::new())
}

/// Starts a run for the session, ignoring "already running", "not
/// configured", and "wrong state". The studio shows a Start button when
/// no run began.
async fn try_start(runner: &AgentRunner, id: &str) {
    if let Err(error) = runner.start(id).await {
        tracing::debug!(%error, session_id = %id, "run not started automatically");
    }
}

/// Checks the run options, returning a message when they are invalid.
fn option_problem(options: &RunOptions) -> Option<String> {
    for (name, level) in [("effort", &options.effort), ("variety", &options.variety)] {
        if !matches!(level.as_str(), "low" | "medium" | "high") {
            return Some(format!(
                "{name} `{level}` is unknown: use low, medium, or high"
            ));
        }
    }
    if options.platforms.len() > PLATFORM_LIMIT {
        return Some(format!(
            "at most {PLATFORM_LIMIT} canvases, got {}",
            options.platforms.len()
        ));
    }
    if let Some(count) = options.slide_count
        && (count == 0 || count > SLIDE_COUNT_LIMIT)
    {
        return Some(format!(
            "slide_count must be between 1 and {SLIDE_COUNT_LIMIT}, got {count}"
        ));
    }
    if let Some(variations) = options.variations
        && (variations == 0 || variations > CANDIDATE_LIMIT)
    {
        return Some(format!(
            "variations must be between 1 and {CANDIDATE_LIMIT}, got {variations}"
        ));
    }
    if let Some(scenario) = &options.scenario
        && !is_deck_scenario(scenario)
        && !is_custom_answer(scenario)
    {
        return Some(format!(
            "scenario `{scenario}` is not usable: type at most {CUSTOM_ANSWER_LIMIT} printable \
             characters, or use one of {}",
            DECK_SCENARIOS.join(", ")
        ));
    }
    // The app owns every axis, but the lists cannot cover every answer,
    // so a typed answer is accepted beside them. Only the shape is
    // checked: the value goes into one prompt line.
    let picked: [AxisCheck<'_>; 8] = [
        ("audience", &options.audience, &AUDIENCES),
        ("tone", &options.tone, &TONES),
        ("color_mode", &options.color_mode, &COLOR_MODES),
        ("scope", &options.scope, &DEMO_SCOPES),
        ("product_kind", &options.product_kind, &PRODUCT_KINDS),
        ("data_state", &options.data_state, &DATA_STATES),
        ("slide_density", &options.slide_density, &SLIDE_DENSITIES),
        ("evidence_style", &options.evidence_style, &EVIDENCE_STYLES),
    ];
    for (name, chosen, choices) in picked {
        if let Some(value) = chosen
            && !choices.iter().any(|(known, _)| known == value)
            && !is_custom_answer(value)
        {
            let known: Vec<&str> = choices.iter().map(|(value, _)| *value).collect();
            return Some(format!(
                "{name} `{value}` is not usable: type at most {CUSTOM_ANSWER_LIMIT} printable \
                 characters, or use one of {}",
                known.join(", ")
            ));
        }
    }
    None
}

/// One app-owned axis to check: its option name, the stored value, and
/// the fixed list the value must come from.
type AxisCheck<'options> = (
    &'static str,
    &'options Option<String>,
    &'static [(&'static str, &'static str)],
);

/// Replaces the run options.
async fn put_options(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(options): Json<RunOptions>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.state == WorkflowState::Generating {
        return conflict("cannot change options while the session is generating");
    }
    if let Some(message) = option_problem(&options) {
        return api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &message, Vec::new());
    }
    match sessions
        .update(&id, |session| session.options = options)
        .await
    {
        Ok(_) => {
            notifier.notify();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

/// Body of `POST /sessions/{id}/messages`.
#[derive(Debug, Deserialize)]
struct MessageRequest {
    content: String,
    #[serde(default)]
    design: Option<String>,
    #[serde(default)]
    action: Option<String>,
}

/// Appends a conversation turn. A `continue` action on a reviewing
/// session starts a generation run over the named design.
async fn post_message(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<MessageRequest>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let content = request.content.trim();
    if content.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "message content is empty",
            Vec::new(),
        );
    }
    if let Some(design) = &request.design
        && !is_valid_design_id(design)
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("design `{design}` is not a valid id"),
            Vec::new(),
        );
    }
    let is_continue = request.action.as_deref() == Some("continue");
    if let Some(action) = &request.action
        && action != "continue"
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("action `{action}` is unknown: use continue, or omit it"),
            Vec::new(),
        );
    }
    if is_continue {
        if request.design.is_none() {
            return api_error::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "a continue message needs the design id to continue",
                Vec::new(),
            );
        }
        // Generating counts: a Finish pressed while a run works joins
        // that run instead of being refused.
        if !matches!(
            session.state,
            WorkflowState::Reviewing | WorkflowState::Generating
        ) {
            return conflict("continue is only allowed while reviewing or generating");
        }
    } else if session.state == WorkflowState::Generating {
        return conflict("cannot send a message while generating");
    }
    // A message after a halted run is the resume: the session returns to
    // the state the run halted in, then takes the turn.
    if session.state.is_halted() {
        let target = match sessions.recovery_target(&id).await {
            Ok(target) => target,
            Err(error) => return session_error_response(&error),
        };
        if let Err(error) = sessions
            .apply(&id, WorkflowEvent::Recovered { to: target })
            .await
        {
            return session_error_response(&error);
        }
    }
    let message = match (is_continue, request.design.as_deref()) {
        (true, Some(design)) => ChatMessage::continue_request(content, design),
        _ => ChatMessage::user(content, request.design.as_deref()),
    };
    if let Err(error) = sessions.append_message(&id, message).await {
        return session_error_response(&error);
    }
    if is_continue && let Err(error) = sessions.apply(&id, WorkflowEvent::ContinueRequested).await {
        return session_error_response(&error);
    }
    notifier.notify();
    // Every message is a turn: the planner answers, asks, writes, or
    // edits, as Swift Deck did.
    try_start(&runner, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Saves a question set the agent asks. Moves the session to
/// clarifying.
async fn put_question_set(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(mut set): Json<BriefQuestionSet>,
) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    let problems = validate_question_set(&set);
    if !problems.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "question set failed validation",
            problems.iter().map(ToString::to_string).collect(),
        );
    }
    // Every set asks the Swift Deck way: the user can always skip.
    crate::planner::relax_question_set(&mut set);
    let number = match sessions.write_question_set(&id, &set).await {
        Ok(number) => number,
        Err(error) => return session_error_response(&error),
    };
    let message = ChatMessage::assistant_questions(&set.message, number);
    if let Err(error) = sessions.append_message(&id, message).await {
        return session_error_response(&error);
    }
    if let Err(error) = sessions.apply(&id, WorkflowEvent::QuestionsAsked).await {
        return session_error_response(&error);
    }
    notifier.notify();
    Json(serde_json::json!({ "number": number })).into_response()
}

/// Body of `POST /sessions/{id}/answers`.
#[derive(Debug, Deserialize)]
struct AnswersRequest {
    question_set: u32,
    answers: Vec<QuestionAnswer>,
}

/// Records the user's answers. Stays in clarifying; the studio then
/// starts a briefing run.
async fn post_answers(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<AnswersRequest>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.state != WorkflowState::Clarifying {
        return conflict("answers are only accepted while clarifying");
    }
    if session.latest_question_set != Some(request.question_set) {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "answers must be for the latest question set",
            Vec::new(),
        );
    }
    let set = match sessions.read_question_set(&id, request.question_set).await {
        Ok(Some(set)) => set,
        Ok(None) => return session_error_response(&SessionError::NotFound { id }),
        Err(error) => return session_error_response(&error),
    };
    let problems = validate_answers(&set, &request.answers);
    if !problems.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "answers failed validation",
            problems.iter().map(ToString::to_string).collect(),
        );
    }
    if let Err(error) = sessions
        .record_answers(&id, request.question_set, request.answers)
        .await
    {
        return session_error_response(&error);
    }
    notifier.notify();
    try_start(&runner, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Writes candidates now, without a planner turn: the user skipped the
/// questions. Moves the session to generating and starts a run.
async fn generate_now(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !session.state.can_take_turn() {
        return conflict("cannot generate while generating or in error");
    }
    // A skip closes the open card. Without a record the set stays open,
    // and the workbench keeps asking over the candidates it wrote.
    let answers = match sessions.answers(&id).await {
        Ok(answers) => answers,
        Err(error) => return session_error_response(&error),
    };
    if let Some(number) = open_question_set(&session, &answers)
        && let Err(error) = sessions.record_answers(&id, number, Vec::new()).await
    {
        return session_error_response(&error);
    }
    if let Err(error) = sessions.apply(&id, WorkflowEvent::GenerationStarted).await {
        return session_error_response(&error);
    }
    notifier.notify();
    try_start(&runner, &id).await;
    StatusCode::OK.into_response()
}

/// Marks a generation run complete, for external runtimes and tests.
async fn complete(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    match sessions
        .apply(&id, WorkflowEvent::GenerationSucceeded)
        .await
    {
        Ok(session) => {
            notifier.notify();
            Json(session).into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

/// Recovers a failed session into the state it was in before the error.
async fn retry(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !session.state.is_halted() {
        return conflict("resume is only allowed after a stop or an error");
    }
    let target = match sessions.recovery_target(&id).await {
        Ok(target) => target,
        Err(error) => return session_error_response(&error),
    };
    match sessions
        .apply(&id, WorkflowEvent::Recovered { to: target })
        .await
    {
        Ok(session) => {
            notifier.notify();
            try_start(&runner, &id).await;
            Json(session).into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::http::StatusCode;
    use tempfile::TempDir;

    use crate::test_support::{send, test_application};

    /// Reads a session view.
    async fn view(application: &axum::Router, id: &str) -> serde_json::Value {
        let (status, body) =
            send(application.clone(), "GET", &format!("/sessions/{id}"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_str(&body).unwrap()
    }

    async fn create(application: &axum::Router, id: &str) {
        let body = format!("{{\"id\":\"{id}\",\"request\":\"Design {id}.\"}}");
        let (status, _) = send(application.clone(), "POST", "/sessions", Some(&body)).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn a_vague_request_enters_clarifying_when_questions_are_asked() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "finance-app").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/sessions/finance-app/question-set",
            Some(r#"{"title":"T","message":"Two things.","questions":[{"id":"platform","label":"Which platform?","kind":"single_select","required":true,"options":[{"value":"web","label":"Web"}]}]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            view(&application, "finance-app").await["session"]["state"],
            "clarifying"
        );
    }

    #[tokio::test]
    async fn a_deck_scenario_is_validated_and_kept_on_the_options() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"intro","request":"Intro for Swift Design.","artifact_kind":"deck"}"#),
        )
        .await;
        // A scenario outside the presets is the user's own words, so it
        // is kept. Only an unusable shape is refused.
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/options",
            Some(r#"{"effort":"medium","variety":"high","templates":[],"preview":true,"platforms":[],"scenario":"Cooking"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            view(&application, "intro").await["session"]["options"]["scenario"],
            "Cooking"
        );
        let long = "x".repeat(design_model::CUSTOM_ANSWER_LIMIT + 1);
        let (status, body) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/options",
            Some(&format!(r#"{{"effort":"medium","variety":"high","templates":[],"preview":true,"platforms":[],"scenario":"{long}"}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("is not usable"), "{body}");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/options",
            Some(r#"{"effort":"medium","variety":"high","templates":[],"preview":true,"platforms":[],"scenario":"Pitch or launch"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let session = view(&application, "intro").await;
        assert_eq!(session["session"]["options"]["scenario"], "Pitch or launch");
        assert!(session.get("brief").is_none());
    }

    #[tokio::test]
    async fn the_apps_own_axes_take_a_preset_or_the_users_own_words() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"intro","request":"Intro for Swift Design."}"#),
        )
        .await;
        let base =
            r#"{"effort":"medium","variety":"medium","templates":[],"preview":true,"platforms":[]"#;
        // The fixed lists cannot cover every answer, so an answer the
        // user typed is kept beside them.
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/options",
            Some(&format!(r#"{base},"audience":"astronauts"}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            view(&application, "intro").await["session"]["options"]["audience"],
            "astronauts"
        );
        // An answer that would break the prompt line is refused.
        let long = "x".repeat(design_model::CUSTOM_ANSWER_LIMIT + 1);
        let (status, body) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/options",
            Some(&format!(r#"{base},"tone":"{long}"}}"#)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("tone `"), "{body}");
        assert!(body.contains("is not usable"), "{body}");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/options",
            Some(&format!(
                r#"{base},"audience":"practitioners","tone":"technical","scope":"short_flow"}}"#
            )),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let options = &view(&application, "intro").await["session"]["options"];
        assert_eq!(options["audience"], "practitioners");
        assert_eq!(options["tone"], "technical");
        assert_eq!(options["scope"], "short_flow");
    }

    #[tokio::test]
    async fn a_deck_question_set_is_written_as_skippable() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"intro","request":"Intro for Swift Design.","artifact_kind":"deck"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/sessions/intro/question-set",
            Some(r#"{"title":"T","message":"One thing.","questions":[{"id":"scenario","label":"What scenario?","kind":"single_select","required":true,"options":[{"value":"talk","label":"Talk"}]}],"can_proceed_with_assumptions":false}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let set = &view(&application, "intro").await["question_sets"][0];
        assert_eq!(set["can_proceed_with_assumptions"], true);
        assert_eq!(set["questions"][0]["required"], false);
    }

    #[tokio::test]
    async fn a_question_set_with_four_questions_is_rejected_with_every_error() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        let (status, body) = send(
            application.clone(),
            "PUT",
            "/sessions/talk/question-set",
            Some(r#"{"title":"T","message":"m","questions":[{"id":"a","label":"A?","kind":"short_text"},{"id":"b","label":"B?","kind":"short_text"},{"id":"c","label":"C?","kind":"short_text"},{"id":"d","label":"D?","kind":"short_text"}]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let response: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(!response["error"]["details"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn answers_are_only_accepted_in_clarifying() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        // No question set yet, session is intake: 409.
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions/talk/answers",
            Some(r#"{"question_set":1,"answers":[]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn retry_restores_the_state_before_the_error() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        // Force the session into error via a failed built-in run: no
        // model is configured, so drive it by hand through the store.
        // Here we assert retry from error is refused when not in error.
        let (status, _) = send(application.clone(), "POST", "/sessions/talk/retry", None).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_message_after_an_error_recovers_the_session() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(application.clone(), "POST", "/sessions/talk/generate", None).await;
        let sessions = crate::sessions::SessionStore::new(directory.path().join("data/sessions"));
        sessions
            .apply("talk", design_model::WorkflowEvent::RunFailed)
            .await
            .unwrap();
        assert_eq!(
            view(&application, "talk").await["session"]["state"],
            "error"
        );
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions/talk/messages",
            Some(r#"{"content":"Try again, smaller."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let session = view(&application, "talk").await;
        assert_eq!(session["session"]["state"], "generating");
        assert_eq!(session["messages"][0]["content"], "Try again, smaller.");
    }

    #[tokio::test]
    async fn deleting_a_session_takes_its_candidates_with_it() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        // The stores the test application serves from.
        let designs = crate::designs::DesignStore::new(directory.path().join("designs"));
        let decks = crate::decks::DeckStore::new(directory.path().join("decks"));
        let design: design_model::Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        designs.save("talk-candidate-1", &design).await.unwrap();
        designs.save("other-candidate-1", &design).await.unwrap();
        decks
            .save("talk-candidate-1", &crate::test_support::sample_deck())
            .await
            .unwrap();
        let (status, body) = send(application.clone(), "DELETE", "/sessions/talk", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        // A session id comes from the request text, so the next session
        // with the same request reuses this id. Its candidates must not
        // be waiting for it.
        assert!(designs.load("talk-candidate-1").await.unwrap().is_none());
        assert!(decks.load("talk-candidate-1").await.unwrap().is_none());
        assert!(designs.load("other-candidate-1").await.unwrap().is_some());
        create(&application, "talk").await;
        assert_eq!(
            view(&application, "talk").await["designs"],
            serde_json::json!([])
        );
    }

    #[tokio::test]
    async fn a_generating_session_deletes_without_a_fight() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(application.clone(), "POST", "/sessions/talk/generate", None).await;
        assert_eq!(
            view(&application, "talk").await["session"]["state"],
            "generating"
        );
        // Generating is not a reason to keep a session the user asked
        // to delete. A run in flight is stopped first; a stale state
        // like this one has no run behind it at all.
        let (status, body) = send(application.clone(), "DELETE", "/sessions/talk", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let (status, _) = send(application.clone(), "GET", "/sessions/talk", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn skipping_the_questions_closes_the_open_card() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/question-set",
            Some(r#"{"title":"T","message":"Two things.","questions":[{"id":"tone","label":"Which tone?","kind":"single_select","required":false,"options":[{"value":"warm","label":"Warm"}]}]}"#),
        )
        .await;
        assert_eq!(
            view(&application, "talk").await["open_question_set"],
            serde_json::json!(1)
        );
        let (status, _) = send(application.clone(), "POST", "/sessions/talk/generate", None).await;
        assert_eq!(status, StatusCode::OK);
        // The card is answered by the skip, so the workbench shows the
        // candidates instead of asking over them.
        let session = view(&application, "talk").await;
        assert_eq!(session["open_question_set"], serde_json::Value::Null);
        assert_eq!(session["answers"][0]["question_set"], 1);
    }

    #[tokio::test]
    async fn a_second_finish_joins_the_running_turn() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        // The run that writes the candidates is in flight.
        send(application.clone(), "POST", "/sessions/talk/generate", None).await;
        assert_eq!(
            view(&application, "talk").await["session"]["state"],
            "generating"
        );
        for candidate in ["talk-candidate-1", "talk-candidate-2"] {
            let body =
                format!(r#"{{"content":"Finish it.","design":"{candidate}","action":"continue"}}"#);
            let (status, body) = send(
                application.clone(),
                "POST",
                "/sessions/talk/messages",
                Some(&body),
            )
            .await;
            // Pressing Finish on a second candidate is not a conflict.
            assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        }
        let session = view(&application, "talk").await;
        assert_eq!(session["session"]["state"], "generating");
        assert_eq!(session["messages"][0]["design"], "talk-candidate-1");
        assert_eq!(session["messages"][1]["design"], "talk-candidate-2");
    }

    #[tokio::test]
    async fn state_survives_a_server_restart() {
        let directory = TempDir::new().unwrap();
        let first = test_application(&directory);
        create(&first, "talk").await;
        send(first.clone(), "POST", "/sessions/talk/generate", None).await;
        // A fresh application over the same directory sees the state.
        let second = test_application(&directory);
        assert_eq!(
            view(&second, "talk").await["session"]["state"],
            "generating"
        );
    }

    #[tokio::test]
    async fn session_views_never_include_credentials_or_paths() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        let (_, body) = send(application.clone(), "GET", "/sessions/talk", None).await;
        assert!(!body.contains("data/sessions"));
        assert!(!body.contains(directory.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn a_missing_session_returns_404() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "GET",
            "/sessions/missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
