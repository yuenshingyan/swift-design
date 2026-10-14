//! The `/sessions` routes: create a session, drive the workflow, and
//! read its state.
//!
//! Every state change goes through `SessionStore::apply`, which is the
//! only place the workflow state changes. Handlers validate structured
//! input at this boundary and answer 422 with every problem.

mod handlers;
#[cfg(test)]
mod tests;
use handlers::{complete, generate_now, post_answers, post_message, put_question_set, retry};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use design_model::{
    AD_COUNT_LIMIT, ArtifactKind, BriefQuestionSet, COVER_COUNT_LIMIT, CUSTOM_ANSWER_LIMIT,
    DECK_SCENARIOS, EMAIL_COUNT_LIMIT, FRAME_COUNT_LIMIT, PAGE_COUNT_LIMIT, QuestionAnswer,
    SHEET_COUNT_LIMIT, WorkflowEvent, WorkflowState, app_axes, axis_by_key, is_custom_answer,
    is_deck_scenario, validate_answers, validate_question_set,
};
use serde::Deserialize;

use crate::AppState;
use crate::agent_runs::AgentRunner;
use crate::api_error;
use crate::artworks::ArtworkStore;
use crate::campaigns::CampaignStore;
use crate::candidates::{CANDIDATE_LIMIT, PLATFORM_LIMIT};
use crate::capture::capture_urls;
use crate::decks::DeckStore;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::documents::DocumentStore;
use crate::edit_focus::referenced_indexes;
use crate::events::ChangeNotifier;
use crate::mailings::MailingStore;
use crate::planner::relax_question_set;
use crate::prints::PrintStore;
use crate::sessions::{
    AnswerRecord, ChatMessage, NewSession, RunOptions, Session, SessionError, SessionStore,
    SessionSummary, SessionView, is_valid_session_id, new_session_id, session_id_of_artifact,
};
use crate::socials::SocialStore;
use crate::uploads::UploadStore;

/// Most slides a deck run may be asked for. Past this the run costs
/// more than a user waits for.
const SLIDE_COUNT_LIMIT: u32 = 60;

/// The `/sessions` route table.
pub fn routes() -> Router<AppState> {
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

/// The eight artifact stores as one extractor, so a handler that reads
/// or deletes a session's artifacts takes one argument for them.
#[derive(Clone)]
pub(crate) struct ArtifactStores {
    pub(crate) designs: DesignStore,
    pub(crate) decks: DeckStore,
    pub(crate) documents: DocumentStore,
    pub(crate) socials: SocialStore,
    pub(crate) prints: PrintStore,
    pub(crate) mailings: MailingStore,
    pub(crate) campaigns: CampaignStore,
    pub(crate) artworks: ArtworkStore,
}

impl axum::extract::FromRef<AppState> for ArtifactStores {
    fn from_ref(state: &AppState) -> ArtifactStores {
        ArtifactStores {
            designs: state.designs.clone(),
            decks: state.decks.clone(),
            documents: state.documents.clone(),
            socials: state.socials.clone(),
            prints: state.prints.clone(),
            mailings: state.mailings.clone(),
            campaigns: state.campaigns.clone(),
            artworks: state.artworks.clone(),
        }
    }
}

/// Builds the full session view. A demo session lists its designs, a
/// deck session its decks, a document session its documents.
async fn build_view(
    sessions: &SessionStore,
    stores: &ArtifactStores,
    session: Session,
) -> Result<SessionView, SessionError> {
    let id = session.id.clone();
    let question_sets = sessions.question_sets(&id).await?;
    let answers = sessions.answers(&id).await?;
    let messages = sessions.messages(&id).await?;
    let runs = sessions.runs(&id).await?;
    let open_question_set = open_question_set(&session, &answers);
    let io = |error: anyhow::Error| SessionError::Io(error.to_string());
    let mut view = SessionView {
        session,
        question_sets,
        open_question_set,
        answers,
        messages,
        runs,
        designs: Vec::new(),
        decks: Vec::new(),
        documents: Vec::new(),
        socials: Vec::new(),
        prints: Vec::new(),
        mailings: Vec::new(),
        campaigns: Vec::new(),
        artworks: Vec::new(),
    };
    match view.session.artifact_kind {
        ArtifactKind::Demo => {
            view.designs = stores
                .designs
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Deck => {
            view.decks = stores
                .decks
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Document => {
            view.documents = stores
                .documents
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Social => {
            view.socials = stores
                .socials
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Print => {
            view.prints = stores
                .prints
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Mailing => {
            view.mailings = stores
                .mailings
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Campaign => {
            view.campaigns = stores
                .campaigns
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
        ArtifactKind::Artwork => {
            view.artworks = stores
                .artworks
                .list()
                .await
                .map_err(io)?
                .into_iter()
                .filter(|summary| session_id_of_artifact(&summary.id) == id)
                .collect();
        }
    }
    Ok(view)
}

/// The latest unanswered question set while the workflow awaits answers.
fn open_question_set(session: &Session, answers: &[AnswerRecord]) -> Option<u32> {
    if session.state != WorkflowState::Clarifying {
        return None;
    }
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
    /// `demo` (the default), `deck`, or `document`.
    #[serde(default)]
    artifact_kind: Option<ArtifactKind>,
}

/// Creates a session in the intake state.
async fn create_session(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(uploads): State<UploadStore>,
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
            // A link in the request is captured before the first run
            // reads the uploads.
            capture_urls(&uploads, &id, prompt).await;
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
    State(stores): State<ArtifactStores>,
    Path(id): Path<String>,
) -> Response {
    let session = match sessions.read(&id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return session_error_response(&SessionError::NotFound { id });
        }
        Err(error) => return session_error_response(&error),
    };
    match build_view(&sessions, &stores, session).await {
        Ok(view) => Json(view).into_response(),
        Err(error) => session_error_response(&error),
    }
}

/// Deletes a session. The designs stay.
async fn delete_session(
    State(sessions): State<SessionStore>,
    State(stores): State<ArtifactStores>,
    State(runner): State<AgentRunner>,
    State(uploads): State<UploadStore>,
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
            delete_session_artifacts(&stores, &id).await;
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

/// Deletes the artifacts of a deleted session: its designs, its decks,
/// and its documents, candidates included.
///
/// A session id comes from the request text, so the same request makes
/// the same id. Without this the next session with that request would
/// open on the deleted session's candidates. A failure is logged and
/// does not fail the delete: the session record is already gone.
async fn delete_session_artifacts(stores: &ArtifactStores, id: &str) {
    match stores.designs.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "designs deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session designs failed")
        }
    }
    match stores.decks.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "decks deleted"),
        Ok(_) => {}
        Err(error) => tracing::warn!(session_id = %id, %error, "deleting the session decks failed"),
    }
    match stores.documents.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "documents deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session documents failed")
        }
    }
    match stores.socials.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "socials deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session socials failed")
        }
    }
    match stores.prints.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "prints deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session prints failed")
        }
    }
    match stores.mailings.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "mailings deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session mailings failed")
        }
    }
    match stores.campaigns.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "campaigns deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session campaigns failed")
        }
    }
    match stores.artworks.delete_session(id).await {
        Ok(count) if count > 0 => tracing::info!(session_id = %id, count, "artworks deleted"),
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(session_id = %id, %error, "deleting the session artworks failed")
        }
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
    if let Some(count) = options.page_count
        && (count == 0 || count > PAGE_COUNT_LIMIT)
    {
        return Some(format!(
            "page_count must be between 1 and {PAGE_COUNT_LIMIT}, got {count}"
        ));
    }
    if let Some(count) = options.frame_count
        && (count == 0 || count > FRAME_COUNT_LIMIT)
    {
        return Some(format!(
            "frame_count must be between 1 and {FRAME_COUNT_LIMIT}, got {count}"
        ));
    }
    if let Some(count) = options.sheet_count
        && (count == 0 || count > SHEET_COUNT_LIMIT)
    {
        return Some(format!(
            "sheet_count must be between 1 and {SHEET_COUNT_LIMIT}, got {count}"
        ));
    }
    if let Some(count) = options.email_count
        && (count == 0 || count > EMAIL_COUNT_LIMIT)
    {
        return Some(format!(
            "email_count must be between 1 and {EMAIL_COUNT_LIMIT}, got {count}"
        ));
    }
    if let Some(count) = options.ad_count
        && (count == 0 || count > AD_COUNT_LIMIT)
    {
        return Some(format!(
            "ad_count must be between 1 and {AD_COUNT_LIMIT}, got {count}"
        ));
    }
    if let Some(count) = options.cover_count
        && (count == 0 || count > COVER_COUNT_LIMIT)
    {
        return Some(format!(
            "cover_count must be between 1 and {COVER_COUNT_LIMIT}, got {count}"
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
    for axis in ArtifactKind::ALL.into_iter().flat_map(app_axes) {
        if let Some(Some(value)) = options.axis(axis.key)
            && !axis.choices.iter().any(|(known, _)| known == value)
            && !is_custom_answer(value)
        {
            let known: Vec<&str> = axis.choices.iter().map(|(value, _)| *value).collect();
            return Some(format!(
                "{} `{value}` is not usable: type at most {CUSTOM_ANSWER_LIMIT} printable \
                 characters, or use one of {}",
                axis.key,
                known.join(", ")
            ));
        }
    }
    for key in &options.suggested {
        if axis_by_key(key).is_none() {
            return Some(format!("suggested axis `{key}` is unknown"));
        }
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
