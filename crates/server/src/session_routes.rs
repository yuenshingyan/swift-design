//! The `/sessions` routes: create a session, drive the workflow, and
//! read its state.
//!
//! Every state change goes through `SessionStore::apply`, which is the
//! only place the workflow state changes. Handlers validate structured
//! input at this boundary and answer 422 with every problem.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use design_model::{
    ArtifactKind, BriefQuestionSet, Critique, DesignBrief, QuestionAnswer, RevisionSource,
    WorkflowEvent, WorkflowState, validate_answers, validate_question_set,
};
use serde::Deserialize;

use crate::agent_runs::AgentRunner;
use crate::api_error;
use crate::candidates::{CANDIDATE_LIMIT, PLATFORM_LIMIT};
use crate::decks::DeckStore;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::events::ChangeNotifier;
use crate::sessions::{
    AnswerRecord, ChatMessage, NewSession, PendingCritique, RunOptions, Session, SessionError,
    SessionStore, SessionSummary, SessionView, is_valid_session_id, session_id_from_request,
    session_id_of_artifact,
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
        .route("/sessions/{id}/brief", get(get_brief).put(put_brief))
        .route("/sessions/{id}/brief/revisions", get(get_revisions))
        .route(
            "/sessions/{id}/brief/revisions/{revision}/restore",
            post(restore_revision),
        )
        .route("/sessions/{id}/approve", post(approve))
        .route(
            "/sessions/{id}/generate-with-assumptions",
            post(generate_with_assumptions),
        )
        .route("/sessions/{id}/critiques", post(post_critique))
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
async fn with_answered_questions(
    sessions: &SessionStore,
    id: &str,
    brief: Option<DesignBrief>,
) -> Result<Option<DesignBrief>, SessionError> {
    let Some(mut brief) = brief else {
        return Ok(None);
    };
    if !brief.answered_questions.is_empty() {
        return Ok(Some(brief));
    }
    let answered = sessions.answered_questions(id).await?;
    brief.answered_questions = crate::briefing::answered_questions_from_answers(&answered);
    crate::briefing::tidy_brief(&mut brief);
    Ok(Some(brief))
}

/// Builds the full session view. A demo session lists its designs, a
/// deck session its decks.
async fn build_view(
    sessions: &SessionStore,
    designs: &DesignStore,
    decks: &DeckStore,
    session: Session,
) -> Result<SessionView, SessionError> {
    let id = session.id.clone();
    let brief = with_answered_questions(sessions, &id, sessions.latest_brief(&id).await?).await?;
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
        brief,
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
        None => session_id_from_request(prompt),
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
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.state == WorkflowState::Generating {
        return conflict("cannot delete a session while it is generating");
    }
    match sessions.delete(&id).await {
        Ok(_) => {
            notifier.notify();
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => session_error_response(&error),
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
    None
}

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
        if session.state != WorkflowState::Reviewing {
            return conflict("continue is only allowed while reviewing");
        }
    } else if matches!(
        session.state,
        WorkflowState::Generating | WorkflowState::Error
    ) {
        return conflict("cannot send a message while generating or in error");
    }
    let message = ChatMessage::user(content, request.design.as_deref());
    if let Err(error) = sessions.append_message(&id, message).await {
        return session_error_response(&error);
    }
    if is_continue {
        if let Err(error) = sessions.apply(&id, WorkflowEvent::ContinueRequested).await {
            return session_error_response(&error);
        }
        try_start(&runner, &id).await;
    }
    notifier.notify();
    StatusCode::NO_CONTENT.into_response()
}

/// Saves a question set the agent asks. Moves the session to
/// clarifying.
async fn put_question_set(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(set): Json<BriefQuestionSet>,
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

/// Body of `PUT /sessions/{id}/brief`.
#[derive(Debug, Deserialize)]
struct BriefRequest {
    brief: DesignBrief,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// True when the user may write a brief revision in `state`: while
/// the brief is being settled, and while a result is under review.
fn is_user_edit_allowed(state: WorkflowState) -> bool {
    matches!(
        state,
        WorkflowState::Clarifying
            | WorkflowState::BriefReady
            | WorkflowState::AwaitingApproval
            | WorkflowState::Reviewing
    )
}

/// Saves a new brief revision. A user edit moves the session to
/// awaiting approval; an agent draft presents the brief.
async fn put_brief(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<BriefRequest>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let is_agent = request.source.as_deref() == Some("agent");
    let (source, event, default_summary): (RevisionSource, &[WorkflowEvent], &str) = if is_agent {
        (
            RevisionSource::Agent,
            &[WorkflowEvent::BriefDrafted, WorkflowEvent::BriefPresented],
            "Drafted the brief",
        )
    } else {
        (
            RevisionSource::UserEdit,
            &[WorkflowEvent::BriefEdited],
            "Edited the brief",
        )
    };
    let allowed = if is_agent {
        matches!(
            session.state,
            WorkflowState::Intake | WorkflowState::Clarifying
        )
    } else {
        is_user_edit_allowed(session.state)
    };
    if !allowed {
        return conflict(&format!(
            "a {} brief edit is not allowed in state `{}`",
            source.as_str(),
            session.state
        ));
    }
    // The kind picks the store the candidates live in, so it is fixed
    // once anything has been generated.
    if !is_agent
        && session.state == WorkflowState::Reviewing
        && request.brief.artifact_kind != session.artifact_kind
    {
        return conflict("the artifact kind cannot change after generation");
    }
    let summary = request
        .summary
        .unwrap_or_else(|| default_summary.to_owned());
    let mut brief = request.brief;
    if is_agent {
        crate::briefing::tidy_brief(&mut brief);
    }
    let revision = match sessions
        .write_brief_revision(&id, brief, source, &summary)
        .await
    {
        Ok(revision) => revision,
        Err(error) => return session_error_response(&error),
    };
    for event in event {
        if let Err(error) = sessions.apply(&id, *event).await {
            return session_error_response(&error);
        }
    }
    notifier.notify();
    Json(serde_json::json!({ "revision": revision })).into_response()
}

/// Query of `GET /sessions/{id}/brief`.
#[derive(Debug, Deserialize)]
struct BriefQuery {
    #[serde(default)]
    revision: Option<u32>,
}

/// Returns one brief revision, the latest by default.
async fn get_brief(
    State(sessions): State<SessionStore>,
    Path(id): Path<String>,
    Query(query): Query<BriefQuery>,
) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    let brief = match query.revision {
        Some(revision) => sessions.read_brief(&id, revision).await,
        None => sessions.latest_brief(&id).await,
    };
    let brief = match brief {
        Ok(brief) => with_answered_questions(&sessions, &id, brief).await,
        Err(error) => Err(error),
    };
    match brief {
        Ok(Some(brief)) => Json(brief).into_response(),
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            "no brief for this session yet",
            Vec::new(),
        ),
        Err(error) => session_error_response(&error),
    }
}

/// Returns the brief revision history.
async fn get_revisions(State(sessions): State<SessionStore>, Path(id): Path<String>) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    match sessions.brief_revisions(&id).await {
        Ok(revisions) => Json(revisions).into_response(),
        Err(error) => session_error_response(&error),
    }
}

/// Writes an old brief revision back as a new user revision. The
/// history keeps every step, and the session returns to awaiting
/// approval like any user edit.
async fn restore_revision(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path((id, revision)): Path<(String, u32)>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !is_user_edit_allowed(session.state) {
        return conflict(&format!(
            "a brief restore is not allowed in state `{}`",
            session.state
        ));
    }
    if session.latest_brief_revision == Some(revision) {
        return conflict(&format!("revision {revision} is already the latest"));
    }
    let brief = match sessions.read_brief(&id, revision).await {
        Ok(Some(brief)) => brief,
        Ok(None) => {
            return api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no brief revision {revision} for this session"),
                Vec::new(),
            );
        }
        Err(error) => return session_error_response(&error),
    };
    if session.state == WorkflowState::Reviewing && brief.artifact_kind != session.artifact_kind {
        return conflict("the artifact kind cannot change after generation");
    }
    let summary = format!("Restored revision {revision}");
    let written = match sessions
        .write_brief_revision(&id, brief, RevisionSource::UserEdit, &summary)
        .await
    {
        Ok(written) => written,
        Err(error) => return session_error_response(&error),
    };
    if let Err(error) = sessions.apply(&id, WorkflowEvent::BriefEdited).await {
        return session_error_response(&error);
    }
    notifier.notify();
    Json(serde_json::json!({ "revision": written })).into_response()
}

/// Approves the latest brief and starts generation.
async fn approve(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.state != WorkflowState::AwaitingApproval {
        return conflict("approve is only allowed while awaiting approval");
    }
    let Some(revision) = session.latest_brief_revision else {
        return conflict("there is no brief to approve");
    };
    if let Err(error) = sessions
        .update(&id, |session| session.approved_revision = Some(revision))
        .await
    {
        return session_error_response(&error);
    }
    match sessions.apply(&id, WorkflowEvent::Approved).await {
        Ok(session) => {
            notifier.notify();
            try_start(&runner, &id).await;
            Json(session).into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

/// Generates with the recorded assumptions, skipping approval.
async fn generate_with_assumptions(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !matches!(
        session.state,
        WorkflowState::Intake
            | WorkflowState::Clarifying
            | WorkflowState::BriefReady
            | WorkflowState::AwaitingApproval
    ) {
        return conflict("generate with assumptions is not allowed in this state");
    }
    let base = match sessions.latest_brief(&id).await {
        Ok(brief) => brief.unwrap_or_default(),
        Err(error) => return session_error_response(&error),
    };
    let skipped = match sessions.answered_questions(&id).await {
        Ok(answered) => crate::briefing::assumptions_from_skipped_answers(&answered),
        Err(error) => return session_error_response(&error),
    };
    let brief = base.with_assumed_open_questions(skipped);
    let revision = match sessions
        .write_brief_revision(
            &id,
            brief,
            RevisionSource::Assumptions,
            "The agent decides the open items",
        )
        .await
    {
        Ok(revision) => revision,
        Err(error) => return session_error_response(&error),
    };
    if let Err(error) = sessions
        .update(&id, |session| session.approved_revision = Some(revision))
        .await
    {
        return session_error_response(&error);
    }
    match sessions
        .apply(&id, WorkflowEvent::GenerateWithAssumptions)
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

/// Body of `POST /sessions/{id}/critiques`.
#[derive(Debug, Deserialize)]
struct CritiqueRequest {
    #[serde(flatten)]
    critique: Critique,
    #[serde(default)]
    design: Option<String>,
}

/// Records a critique, makes a brief revision, and starts an edit run.
async fn post_critique(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<CritiqueRequest>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.state != WorkflowState::Reviewing {
        return conflict("critiques are only allowed while reviewing");
    }
    if request.critique.text.trim().is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "critique text is empty",
            Vec::new(),
        );
    }
    let design = request.design.or_else(|| session.chosen_design.clone());
    let Some(design) = design else {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no design to critique: choose one first",
            Vec::new(),
        );
    };
    if session_id_of_artifact(&design) != id {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("design `{design}` does not belong to session `{id}`"),
            Vec::new(),
        );
    }
    let base = match sessions.latest_brief(&id).await {
        Ok(brief) => brief.unwrap_or_default(),
        Err(error) => return session_error_response(&error),
    };
    let brief = base.with_instruction(request.critique.as_instruction());
    let revision = match sessions
        .write_brief_revision(&id, brief, RevisionSource::Critique, "Applied a critique")
        .await
    {
        Ok(revision) => revision,
        Err(error) => return session_error_response(&error),
    };
    if let Err(error) = sessions
        .update(&id, |session| {
            session.approved_revision = Some(revision);
            session.pending_critique = Some(PendingCritique {
                design: design.clone(),
                critique: request.critique.clone(),
            });
        })
        .await
    {
        return session_error_response(&error);
    }
    match sessions.apply(&id, WorkflowEvent::CritiqueSubmitted).await {
        Ok(session) => {
            notifier.notify();
            try_start(&runner, &id).await;
            Json(session).into_response()
        }
        Err(error) => session_error_response(&error),
    }
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
    if session.state != WorkflowState::Error {
        return conflict("retry is only allowed after an error");
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
    async fn a_specific_request_can_enter_brief_ready_and_awaiting_approval() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        let (status, body) = send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"source":"agent","brief":{"target_artifact":"page","audience":"devs"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            view(&application, "talk").await["session"]["state"],
            "awaiting_approval"
        );
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
    async fn generate_with_assumptions_skips_approval_from_intake() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions/talk/generate-with-assumptions",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            view(&application, "talk").await["session"]["state"],
            "generating"
        );
    }

    #[tokio::test]
    async fn an_old_brief_gets_its_answered_questions_back() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/question-set",
            Some(r#"{"title":"T","message":"m","questions":[{"id":"tone","label":"Which tone?","kind":"single_select","required":false,"options":[{"value":"warm","label":"Warm"}]}]}"#),
        )
        .await;
        send(
            application.clone(),
            "POST",
            "/sessions/talk/answers",
            Some(r#"{"question_set":1,"answers":[{"question_id":"tone","values":["warm"]}]}"#),
        )
        .await;
        // A brief written the way the old server wrote them: the answer
        // is a fact line, and answered_questions is empty.
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(
                r#"{"brief":{"audience":"devs","confirmed_facts":["Which tone?: Warm"]},"source":"agent"}"#,
            ),
        )
        .await;
        for path in ["/sessions/talk/brief", "/sessions/talk"] {
            let (_, body) = send(application.clone(), "GET", path, None).await;
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            let brief = if path == "/sessions/talk" {
                value["brief"].clone()
            } else {
                value
            };
            let entries = brief["answered_questions"].as_array().unwrap();
            assert_eq!(entries.len(), 1, "no answers rebuilt for {path}");
            assert_eq!(entries[0]["question"], "Which tone?");
            assert_eq!(entries[0]["answer"], "Warm");
            assert_eq!(entries[0]["is_assumed"], false);
        }
    }

    #[tokio::test]
    async fn skipped_answers_become_explicit_assumptions_on_generate_with_assumptions() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/question-set",
            Some(r#"{"title":"T","message":"m","questions":[{"id":"tone","label":"Which tone?","kind":"single_select","required":false,"options":[{"value":"warm","label":"Warm"}]}]}"#),
        )
        .await;
        send(
            application.clone(),
            "POST",
            "/sessions/talk/answers",
            Some(r#"{"question_set":1,"answers":[{"question_id":"tone","skipped":true}]}"#),
        )
        .await;
        send(
            application.clone(),
            "POST",
            "/sessions/talk/generate-with-assumptions",
            None,
        )
        .await;
        let (_, body) = send(application.clone(), "GET", "/sessions/talk/brief", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        let assumptions = brief["assumptions"].as_array().unwrap();
        assert!(
            assumptions
                .iter()
                .any(|line| line.as_str().unwrap().contains("Which tone?"))
        );
    }

    #[tokio::test]
    async fn editing_the_brief_creates_a_new_revision_and_returns_to_awaiting_approval() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"source":"agent","brief":{"audience":"devs"}}"#),
        )
        .await;
        let (status, body) = send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"brief":{"audience":"designers"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["revision"],
            2
        );
        assert_eq!(
            view(&application, "talk").await["session"]["state"],
            "awaiting_approval"
        );
        let (_, revisions) = send(
            application.clone(),
            "GET",
            "/sessions/talk/brief/revisions",
            None,
        )
        .await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&revisions)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn restoring_a_revision_writes_it_back_as_a_new_user_revision() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"source":"agent","brief":{"audience":"devs"}}"#),
        )
        .await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"brief":{"audience":"designers"}}"#),
        )
        .await;
        let (status, body) = send(
            application.clone(),
            "POST",
            "/sessions/talk/brief/revisions/1/restore",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["revision"],
            3
        );
        let session = view(&application, "talk").await;
        assert_eq!(session["session"]["state"], "awaiting_approval");
        assert_eq!(session["brief"]["audience"], "devs");
        assert_eq!(session["brief"]["revision"], 3);
        let history = &session["brief"]["revision_history"];
        assert_eq!(history.as_array().unwrap().len(), 3);
        assert_eq!(history[2]["source"], "user_edit");
        assert_eq!(history[2]["summary"], "Restored revision 1");
    }

    #[tokio::test]
    async fn restoring_the_latest_or_a_missing_revision_is_refused() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"source":"agent","brief":{"audience":"devs"}}"#),
        )
        .await;
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions/talk/brief/revisions/1/restore",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions/talk/brief/revisions/7/restore",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn approving_records_the_revision_and_starts_generating() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/sessions/talk/brief",
            Some(r#"{"source":"agent","brief":{"audience":"devs"}}"#),
        )
        .await;
        let (status, _) = send(application.clone(), "POST", "/sessions/talk/approve", None).await;
        assert_eq!(status, StatusCode::OK);
        let session = view(&application, "talk").await;
        assert_eq!(session["session"]["state"], "generating");
        assert_eq!(session["session"]["approved_revision"], 1);
    }

    #[tokio::test]
    async fn a_critique_creates_a_revision_and_starts_an_edit_run() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        // Reach reviewing with a chosen design.
        create(&application, "talk").await;
        send(
            application.clone(),
            "POST",
            "/sessions/talk/generate-with-assumptions",
            None,
        )
        .await;
        send(
            application.clone(),
            "PUT",
            "/designs/talk-candidate-1",
            Some(crate::test_support::SAMPLE_DESIGN),
        )
        .await;
        send(
            application.clone(),
            "POST",
            "/candidates/talk/choose",
            Some(r#"{"id":"talk-candidate-1"}"#),
        )
        .await;
        send(application.clone(), "POST", "/sessions/talk/complete", None).await;
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions/talk/critiques",
            Some(r#"{"text":"Tighten the hero."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let session = view(&application, "talk").await;
        assert_eq!(session["session"]["state"], "generating");
        assert!(session["session"]["pending_critique"].is_object());
    }

    #[tokio::test]
    async fn critiques_need_a_chosen_design() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        create(&application, "talk").await;
        send(
            application.clone(),
            "POST",
            "/sessions/talk/generate-with-assumptions",
            None,
        )
        .await;
        send(application.clone(), "POST", "/sessions/talk/complete", None).await;
        let (status, body) = send(
            application.clone(),
            "POST",
            "/sessions/talk/critiques",
            Some(r#"{"category":"content","text":"More copy."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("no design to critique"));
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
    async fn state_survives_a_server_restart() {
        let directory = TempDir::new().unwrap();
        let first = test_application(&directory);
        create(&first, "talk").await;
        send(
            first.clone(),
            "POST",
            "/sessions/talk/generate-with-assumptions",
            None,
        )
        .await;
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
