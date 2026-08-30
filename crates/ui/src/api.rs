//! HTTP calls to the Swift Design server.
//!
//! Every function returns `Result<T, String>`, except `save_design`,
//! which returns the server's full validation error list. The shared
//! domain types come from `design_model`, so the studio and the server
//! agree on one definition.
//!
//! Some endpoints are part of the client surface but not yet wired to a
//! screen, so the whole module allows dead code.
#![allow(dead_code)]

use std::collections::HashMap;

use design_model::{
    ArtifactKind, BriefQuestionSet, DECK_VIEWPORT, Deck, Design, Document, Paper, QuestionAnswer,
    WorkflowState,
};
use gloo_net::http::{Request, RequestBuilder, Response};
use serde::{Deserialize, Serialize};

/// Builds a request builder into a request, mapping the error.
fn built(builder: RequestBuilder) -> Result<Request, String> {
    builder.build().map_err(|error| error.to_string())
}

/// The server's error envelope.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// Inner part of the error envelope.
#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: String,
    #[serde(default)]
    details: Vec<String>,
}

/// Sends `request` and returns the response when it is 2xx, else the
/// server's envelope message or a status line.
async fn send_checked(request: Request, label: &str) -> Result<Response, String> {
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.ok() {
        return Ok(response);
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) => Err(envelope.error.message),
        Err(_) => Err(format!("{label} failed with status {status}")),
    }
}

/// Reads a 2xx JSON body, or the envelope message.
async fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let response = send_checked(built(Request::get(url))?, url).await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Sends a POST or DELETE with no body, checking the response.
async fn send_empty(builder: RequestBuilder, label: &str) -> Result<(), String> {
    send_checked(built(builder)?, label).await.map(|_| ())
}

// -- Sessions ------------------------------------------------------------

/// One row of `GET /sessions`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SessionSummary {
    /// The session id.
    pub id: String,
    /// The title.
    pub title: String,
    /// What the session builds.
    #[serde(default)]
    pub artifact_kind: ArtifactKind,
    /// The workflow state.
    pub state: WorkflowState,
    /// When it was created, RFC 3339.
    #[serde(default)]
    pub created_at: String,
    /// When it last changed, RFC 3339.
    pub updated_at: String,
    /// The chosen design or deck, when there is one.
    #[serde(default)]
    pub chosen_design: Option<String>,
}

/// One conversation turn, mirrored from the server.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    /// `user` or `assistant`.
    pub role: String,
    /// The turn text.
    pub content: String,
    /// The design the turn is about, when it names one.
    #[serde(default)]
    pub design: Option<String>,
    /// The number of the question set this turn posed, when it did.
    #[serde(default)]
    pub question_set: Option<u32>,
    /// True when the user pressed Finish on the design in `design`.
    #[serde(default)]
    pub is_continue: bool,
    /// When the turn was recorded, RFC 3339.
    #[serde(default)]
    pub at: Option<String>,
    /// The artifacts an assistant turn wrote, edited, or finished.
    #[serde(default)]
    pub artifacts: Vec<String>,
}

/// One recorded set of answers.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct AnswerRecord {
    /// The question set answered.
    pub question_set: u32,
    /// The answers.
    pub answers: Vec<QuestionAnswer>,
    /// When they arrived, RFC 3339.
    pub at: String,
}

/// One run record, mirrored from the server.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RunRecord {
    /// The run id.
    pub run_id: String,
    /// `briefing` or `generation`.
    pub mode: String,
    /// The outcome, when the run finished.
    #[serde(default)]
    pub result: Option<String>,
}

/// The persisted session record.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Session {
    /// The session id.
    pub id: String,
    /// The title.
    pub title: String,
    /// The user's request.
    pub request: String,
    /// What the session builds.
    #[serde(default)]
    pub artifact_kind: ArtifactKind,
    /// The workflow state.
    pub state: WorkflowState,
    /// The failure message shown in the error state.
    #[serde(default)]
    pub error: Option<String>,
    /// The design or deck the user chose.
    #[serde(default)]
    pub chosen_design: Option<String>,
    /// The run options the next run uses.
    #[serde(default)]
    pub options: SessionOptions,
}

/// Most candidates one run may write. The server rejects more.
pub const VARIATION_LIMIT: usize = 5;

/// The run options of one session, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOptions {
    /// `low`, `medium`, or `high`.
    pub effort: String,
    /// How many candidates a run writes, 1 to `VARIATION_LIMIT`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variations: Option<usize>,
    /// How different the candidates are: `low`, `medium`, or `high`.
    pub variety: String,
    /// Template ids the candidates follow.
    #[serde(default)]
    pub templates: Vec<String>,
    /// True to write preview candidates first.
    pub preview: bool,
    /// The canvases a demo run builds for. One design per canvas per
    /// variation. Empty means the default desktop canvas.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// How many slides a deck run writes. `None` leaves it to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_count: Option<u32>,
    /// The scenario a deck is for, one of the presets. `None` leaves
    /// it to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    /// Who the artifact is for, one of `AUDIENCES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// The tone the artifact takes, one of `TONES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    /// How much of a demo to build, one of `DEMO_SCOPES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Light or dark, one of `COLOR_MODES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<String>,
    /// What kind of product a demo shows, one of `PRODUCT_KINDS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_kind: Option<String>,
    /// What state a demo's screens are in, one of `DATA_STATES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_state: Option<String>,
    /// How finished a demo looks, one of `FIDELITIES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity: Option<String>,
    /// How much goes on one slide, one of `SLIDE_DENSITIES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_density: Option<String>,
    /// How much a deck or a document leans on data, one of
    /// `EVIDENCE_STYLES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_style: Option<String>,
    /// What kind of document to write, one of `DOCUMENT_KINDS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_kind: Option<String>,
    /// The paper a document is laid out on, one of `PAPERS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
    /// How much goes on one page, one of `SLIDE_DENSITIES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_density: Option<String>,
    /// How many pages a document run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    /// The axes the planner filled from the request, by option key.
    /// The card marks them as suggested until the user picks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested: Vec<String>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            effort: "medium".to_owned(),
            variations: None,
            variety: "medium".to_owned(),
            templates: Vec::new(),
            preview: true,
            platforms: Vec::new(),
            slide_count: None,
            scenario: None,
            audience: None,
            tone: None,
            scope: None,
            color_mode: None,
            product_kind: None,
            data_state: None,
            fidelity: None,
            slide_density: None,
            evidence_style: None,
            document_kind: None,
            paper: None,
            page_density: None,
            page_count: None,
            suggested: Vec::new(),
        }
    }
}

impl SessionOptions {
    /// How many candidates the next run writes. Mirrors the server
    /// default so the studio shows the number the run will use.
    pub fn variation_count(&self) -> usize {
        self.variations.unwrap_or(2).clamp(1, VARIATION_LIMIT)
    }
}

/// The full view of one session.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct SessionView {
    /// The session record.
    pub session: Session,
    /// Every question set asked, in order.
    #[serde(default)]
    pub question_sets: Vec<BriefQuestionSet>,
    /// The number of the open question set, when one is.
    #[serde(default)]
    pub open_question_set: Option<u32>,
    /// Every recorded answer set.
    #[serde(default)]
    pub answers: Vec<AnswerRecord>,
    /// The conversation.
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// The run records.
    #[serde(default)]
    pub runs: Vec<RunRecord>,
    /// The designs that belong to this session. Empty unless the session
    /// is a demo session.
    #[serde(default)]
    pub designs: Vec<DesignSummary>,
    /// The decks that belong to this session. Empty unless the session
    /// is a deck session.
    #[serde(default)]
    pub decks: Vec<DeckSummary>,
    /// The documents that belong to this session. Empty unless the
    /// session is a document session.
    #[serde(default)]
    pub documents: Vec<DocumentSummary>,
}

/// Body of `POST /sessions`.
#[derive(Debug, Serialize)]
pub struct CreateSessionRequest<'value> {
    /// The user's request.
    pub request: &'value str,
    /// `demo`, `deck`, or `document`.
    pub artifact_kind: &'value str,
    /// How hard to work.
    pub options: CreateOptions<'value>,
}

/// The run options sent with a new session.
#[derive(Debug, Serialize)]
pub struct CreateOptions<'value> {
    /// `low`, `medium`, or `high`.
    pub effort: &'value str,
    /// True to write preview candidates first.
    pub preview: bool,
    /// Template ids the candidates follow.
    pub templates: &'value [String],
}

/// Reply of `POST /sessions`: the created session.
#[derive(Debug, Deserialize)]
struct CreatedSession {
    id: String,
}

/// Lists every session, newest change first.
pub async fn fetch_sessions() -> Result<Vec<SessionSummary>, String> {
    get_json("/sessions").await
}

/// Creates a session and returns its id.
pub async fn create_session(request: &CreateSessionRequest<'_>) -> Result<String, String> {
    let builder = Request::post("/sessions")
        .json(request)
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /sessions").await?;
    response
        .json::<CreatedSession>()
        .await
        .map(|created| created.id)
        .map_err(|error| error.to_string())
}

/// Fetches one session view.
pub async fn fetch_session(id: &str) -> Result<SessionView, String> {
    get_json(&format!("/sessions/{id}")).await
}

/// Body of `POST /sessions/{id}/messages`.
#[derive(Serialize)]
struct MessageRequest<'value> {
    content: &'value str,
    #[serde(skip_serializing_if = "Option::is_none")]
    design: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<&'value str>,
    /// The candidates the turn edits, pinned with `@` in the session chat.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pinned: &'value [String],
}

/// Appends a user turn to the conversation.
pub async fn send_session_message(
    id: &str,
    content: &str,
    design: Option<&str>,
) -> Result<(), String> {
    post_message(id, content, design, &[]).await
}

/// Appends a user turn that edits the pinned candidates.
pub async fn send_session_message_about(
    id: &str,
    content: &str,
    pinned: &[String],
) -> Result<(), String> {
    post_message(id, content, None, pinned).await
}

async fn post_message(
    id: &str,
    content: &str,
    design: Option<&str>,
    pinned: &[String],
) -> Result<(), String> {
    let builder = Request::post(&format!("/sessions/{id}/messages"))
        .json(&MessageRequest {
            content,
            design,
            action: None,
            pinned,
        })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /sessions/messages")
        .await
        .map(|_| ())
}

/// Asks the app to write the remaining screens or slides of a preview
/// artifact from its outline. The server starts the run itself.
pub async fn continue_artifact(session_id: &str, artifact_id: &str) -> Result<(), String> {
    let builder = Request::post(&format!("/sessions/{session_id}/messages"))
        .json(&MessageRequest {
            content: "Write the rest from the outline.",
            design: Some(artifact_id),
            action: Some("continue"),
            pinned: &[],
        })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /sessions/messages continue")
        .await
        .map(|_| ())
}

/// Asks the app to write one unit of an artifact anew: `unit` is
/// `screen` or `slide`, `number` is one-based. The server starts the
/// run itself, without a planner turn.
pub async fn regenerate_unit(
    session_id: &str,
    artifact_id: &str,
    unit: &str,
    number: usize,
) -> Result<(), String> {
    let content = format!("[{unit} {number}] Write this {unit} anew.");
    let builder = Request::post(&format!("/sessions/{session_id}/messages"))
        .json(&MessageRequest {
            content: &content,
            design: Some(artifact_id),
            action: Some("regenerate"),
            pinned: &[],
        })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /sessions/messages regenerate")
        .await
        .map(|_| ())
}

/// Body of `POST /sessions/{id}/answers`.
#[derive(Serialize)]
struct AnswersRequest<'value> {
    question_set: u32,
    answers: &'value [QuestionAnswer],
}

/// Sends the user's answers for one question set.
pub async fn send_session_answers(
    id: &str,
    question_set: u32,
    answers: &[QuestionAnswer],
) -> Result<(), String> {
    let builder = Request::post(&format!("/sessions/{id}/answers"))
        .json(&AnswersRequest {
            question_set,
            answers,
        })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /sessions/answers")
        .await
        .map(|_| ())
}

/// Replaces the run options of one session. The server rejects a
/// variation count outside 1 to `VARIATION_LIMIT`, and any change while
/// the session generates.
pub async fn save_session_options(id: &str, options: &SessionOptions) -> Result<(), String> {
    let builder = Request::put(&format!("/sessions/{id}/options"))
        .json(options)
        .map_err(|error| error.to_string())?;
    send_checked(builder, "PUT /sessions/{id}/options")
        .await
        .map(|_| ())
}

/// Writes candidates now, without more questions.
pub async fn generate_now(id: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/sessions/{id}/generate")),
        "generate",
    )
    .await
}

/// Retries a failed session.
pub async fn retry_session(id: &str) -> Result<(), String> {
    send_empty(Request::post(&format!("/sessions/{id}/retry")), "retry").await
}

/// Deletes a session. The designs stay.
pub async fn delete_session(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/sessions/{id}")),
        "DELETE /sessions",
    )
    .await
}

// -- Designs -------------------------------------------------------------

/// One row of `GET /designs`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DesignSummary {
    /// Design id used in `/designs/{id}` routes.
    pub id: String,
    /// Design title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The px canvas of every screen.
    #[serde(default)]
    pub viewport: design_model::Viewport,
    /// Number of screens.
    pub screen_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder screens a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl DesignSummary {
    /// True when the design is a preview that waits for its screens.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.screen_count
    }

    /// True when the design still owes screens.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The CSS aspect-ratio for the design's viewport.
    pub fn aspect_ratio(&self) -> String {
        self.viewport.aspect_ratio_css()
    }
}

/// Fetches the design listing.
pub async fn fetch_design_list() -> Result<Vec<DesignSummary>, String> {
    get_json("/designs").await
}

/// Fetches one design.
pub async fn fetch_design(id: &str) -> Result<Design, String> {
    get_json(&format!("/designs/{id}")).await
}

/// The CSS class the server puts on a placeholder screen.
const PENDING_SCREEN_CLASS: &str = "swift-design-pending";

/// How many screens of the design are placeholders.
pub fn pending_screen_count(design: &Design) -> usize {
    design
        .screens
        .iter()
        .filter(|screen| screen.html.contains(PENDING_SCREEN_CLASS))
        .count()
}

/// Saves one design as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_design(id: &str, design: &Design) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/designs/{id}"))
        .header("x-swift-design-author", "user")
        .json(design)
        .map_err(|error| vec![error.to_string()])?
        .send()
        .await
        .map_err(|error| vec![error.to_string()])?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) if !envelope.error.details.is_empty() => Err(envelope.error.details),
        Ok(envelope) => Err(vec![envelope.error.message]),
        Err(_) => Err(vec![format!(
            "PUT /designs/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one design.
pub async fn delete_design(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/designs/{id}")),
        "DELETE /designs",
    )
    .await
}

/// Response of a fork: the id of the new candidate.
#[derive(Debug, Deserialize)]
struct ForkResponse {
    id: String,
}

/// Copies one design candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_design(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/designs/{id}/fork")))?;
    let response = send_checked(request, "POST /designs/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one deck candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_deck(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/decks/{id}/fork")))?;
    let response = send_checked(request, "POST /decks/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one document candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_document(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/documents/{id}/fork")))?;
    let response = send_checked(request, "POST /documents/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Response of `GET /designs/{id}/authors`.
#[derive(Debug, Deserialize)]
struct AuthorsResponse {
    user_paths: Vec<String>,
}

/// Fetches the field paths the user changed in this design.
pub async fn fetch_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/designs/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// One row of `GET /designs/{id}/history`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HistorySnapshot {
    /// File stem of the snapshot.
    pub stamp: String,
    /// When it was taken, RFC 3339.
    pub saved_at: String,
    /// Size of the snapshot file.
    pub size_bytes: u64,
}

/// Fetches the saved snapshots of one design.
pub async fn fetch_design_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/designs/{id}/history")).await
}

/// Writes one snapshot back as the current design.
pub async fn restore_design_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/designs/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}

// -- Decks ---------------------------------------------------------------

/// One row of `GET /decks`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeckSummary {
    /// Deck id used in `/decks/{id}` routes.
    pub id: String,
    /// Deck title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// Number of slides.
    pub slide_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder slides a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl DeckSummary {
    /// True when the deck is a preview that waits for its slides.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.slide_count
    }

    /// True when the deck still owes slides.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The CSS aspect-ratio of every deck: 16:9.
    pub fn aspect_ratio(&self) -> String {
        DECK_VIEWPORT.aspect_ratio_css()
    }
}

/// Fetches the deck listing.
pub async fn fetch_deck_list() -> Result<Vec<DeckSummary>, String> {
    get_json("/decks").await
}

/// Fetches one deck.
pub async fn fetch_deck(id: &str) -> Result<Deck, String> {
    get_json(&format!("/decks/{id}")).await
}

/// How many slides of the deck are placeholders.
pub fn pending_slide_count(deck: &Deck) -> usize {
    deck.slides
        .iter()
        .filter(|slide| slide.html.contains(PENDING_SCREEN_CLASS))
        .count()
}

/// Saves one deck as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_deck(id: &str, deck: &Deck) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/decks/{id}"))
        .header("x-swift-design-author", "user")
        .json(deck)
        .map_err(|error| vec![error.to_string()])?
        .send()
        .await
        .map_err(|error| vec![error.to_string()])?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) if !envelope.error.details.is_empty() => Err(envelope.error.details),
        Ok(envelope) => Err(vec![envelope.error.message]),
        Err(_) => Err(vec![format!("PUT /decks/{id} failed with status {status}")]),
    }
}

/// Deletes one deck.
pub async fn delete_deck(id: &str) -> Result<(), String> {
    send_empty(Request::delete(&format!("/decks/{id}")), "DELETE /decks").await
}

/// Fetches the field paths the user changed in this deck.
pub async fn fetch_deck_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/decks/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one deck.
pub async fn fetch_deck_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/decks/{id}/history")).await
}

/// Writes one snapshot back as the current deck.
pub async fn restore_deck_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/decks/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}

// -- Documents -----------------------------------------------------------

/// One row of `GET /documents`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DocumentSummary {
    /// Document id used in `/documents/{id}` routes.
    pub id: String,
    /// Document title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The paper the pages are laid out on.
    #[serde(default)]
    pub paper: Paper,
    /// Number of pages.
    pub page_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder pages a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl DocumentSummary {
    /// True when the document is a preview that waits for its pages.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.page_count
    }

    /// True when the document still owes pages.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every page.
    pub fn viewport(&self) -> design_model::Viewport {
        self.paper.viewport()
    }

    /// The CSS aspect-ratio of the document's paper.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the document listing.
pub async fn fetch_document_list() -> Result<Vec<DocumentSummary>, String> {
    get_json("/documents").await
}

/// Fetches one document.
pub async fn fetch_document(id: &str) -> Result<Document, String> {
    get_json(&format!("/documents/{id}")).await
}

/// Saves one document as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_document(id: &str, document: &Document) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/documents/{id}"))
        .header("x-swift-design-author", "user")
        .json(document)
        .map_err(|error| vec![error.to_string()])?
        .send()
        .await
        .map_err(|error| vec![error.to_string()])?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) if !envelope.error.details.is_empty() => Err(envelope.error.details),
        Ok(envelope) => Err(vec![envelope.error.message]),
        Err(_) => Err(vec![format!(
            "PUT /documents/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one document.
pub async fn delete_document(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/documents/{id}")),
        "DELETE /documents",
    )
    .await
}

/// Fetches the field paths the user changed in this document.
pub async fn fetch_document_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/documents/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one document.
pub async fn fetch_document_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/documents/{id}/history")).await
}

/// Writes one snapshot back as the current document.
pub async fn restore_document_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/documents/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}

// -- Templates -----------------------------------------------------------

/// One row of `GET /templates`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TemplateSummary {
    /// Template id.
    pub id: String,
    /// The name the user gave the template.
    pub name: String,
    /// When it was saved, RFC 3339.
    pub saved_at: String,
    /// Theme name.
    pub theme: String,
    /// How many example screens it holds.
    pub screen_count: usize,
    /// True when a new session starts with this template picked.
    #[serde(default)]
    pub is_default: bool,
}

/// Body of `POST /templates/extract`: one of `url` and `uploads`.
#[derive(Serialize)]
struct ExtractTemplateRequest<'value> {
    name: &'value str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<&'value str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    uploads: &'value [String],
    scope: &'value str,
}

/// Makes a template from a website or from uploaded brand files in
/// `scope`: the model reads them and answers with a theme.
pub async fn extract_template(
    name: &str,
    url: Option<&str>,
    uploads: &[String],
    scope: &str,
) -> Result<TemplateSummary, String> {
    let builder = Request::post("/templates/extract")
        .json(&ExtractTemplateRequest {
            name,
            url,
            uploads,
            scope,
        })
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /templates/extract").await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Body of `PUT /templates/{id}/default`.
#[derive(Serialize)]
struct DefaultTemplateRequest {
    is_default: bool,
}

/// Marks a template as one every new session starts with, or clears
/// the mark.
pub async fn set_default_template(id: &str, is_default: bool) -> Result<(), String> {
    let builder = Request::put(&format!("/templates/{id}/default"))
        .json(&DefaultTemplateRequest { is_default })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "PUT /templates/default")
        .await
        .map(|_| ())
}

/// Body of `POST /templates`. Exactly one source id is set.
#[derive(Debug, Serialize)]
struct SaveTemplateRequest<'value> {
    #[serde(skip_serializing_if = "Option::is_none")]
    design_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deck_id: Option<&'value str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<&'value str>,
    name: &'value str,
}

/// Fetches the saved templates, newest first.
pub async fn fetch_templates() -> Result<Vec<TemplateSummary>, String> {
    get_json("/templates").await
}

/// Saves the style of one design as a template.
pub async fn save_template(design_id: &str, name: &str) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: Some(design_id),
        deck_id: None,
        document_id: None,
        name,
    })
    .await
}

/// Saves the style of one deck as a template.
pub async fn save_deck_template(deck_id: &str, name: &str) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: Some(deck_id),
        document_id: None,
        name,
    })
    .await
}

/// Saves the style of one document as a template.
pub async fn save_document_template(
    document_id: &str,
    name: &str,
) -> Result<TemplateSummary, String> {
    save_template_from(SaveTemplateRequest {
        design_id: None,
        deck_id: None,
        document_id: Some(document_id),
        name,
    })
    .await
}

/// Posts one template save request.
async fn save_template_from(request: SaveTemplateRequest<'_>) -> Result<TemplateSummary, String> {
    let builder = Request::post("/templates")
        .json(&request)
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /templates").await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Deletes one template.
pub async fn delete_template(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/templates/{id}")),
        "DELETE /templates",
    )
    .await
}

// -- Uploads -------------------------------------------------------------

/// One row of `GET /uploads`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct UploadSummary {
    /// Stored file name.
    pub name: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Content type from the extension.
    #[serde(default)]
    pub content_type: String,
    /// True for images.
    #[serde(default)]
    pub is_image: bool,
}

/// Fetches the uploads of one scope: a session id, or `DRAFT_SCOPE`
/// for the landing page.
pub async fn fetch_uploads(scope: &str) -> Result<Vec<UploadSummary>, String> {
    get_json(&format!("/uploads?session={scope}")).await
}

/// The scope of files attached before a session exists. Creating a
/// session takes them.
pub const DRAFT_SCOPE: &str = "_draft";

/// Deletes one upload.
pub async fn delete_upload(name: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/uploads/{name}")),
        "DELETE /uploads",
    )
    .await
}

// -- Events --------------------------------------------------------------

/// Response of `GET /events`.
#[derive(Debug, Deserialize)]
struct EventsResponse {
    revision: u64,
}

/// Waits up to 25 seconds for the server revision to move past `after`.
pub async fn wait_for_change(after: u64) -> Result<u64, String> {
    get_json::<EventsResponse>(&format!("/events?after={after}&wait=25"))
        .await
        .map(|events| events.revision)
}

// -- Agent runs ----------------------------------------------------------

/// State of the run, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AgentRun {
    /// True while the run is active.
    pub is_running: bool,
    /// Exit code of the last run.
    pub exit_code: Option<i32>,
    /// Tail of the run's output.
    pub log_tail: String,
    /// Label of the running (or last) model or command.
    #[serde(default)]
    pub active_agent: Option<String>,
    /// The session the run is for.
    #[serde(default)]
    pub session_id: Option<String>,
    /// `briefing` or `generation`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Input tokens of the latest request.
    #[serde(default)]
    pub context_tokens: u64,
    /// Input plus output tokens over the run.
    #[serde(default)]
    pub total_tokens: u64,
    /// Context window of the running model.
    #[serde(default)]
    pub context_window: u64,
    /// How far the current turn is, 0 to 100.
    #[serde(default)]
    pub progress: Option<u8>,
    /// How far each design the turn writes is, by id.
    #[serde(default)]
    pub designs: HashMap<String, u8>,
}

/// Body of `POST /agent-runs` and `DELETE /agent-runs`.
#[derive(Serialize)]
struct RunRequest<'value> {
    session_id: &'value str,
}

/// Fetches the state of the run.
pub async fn fetch_agent_run() -> Result<AgentRun, String> {
    get_json("/agent-runs").await
}

/// Starts a run for the named session.
pub async fn start_agent_run(session_id: &str) -> Result<(), String> {
    let builder = Request::post("/agent-runs")
        .json(&RunRequest { session_id })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /agent-runs").await.map(|_| ())
}

/// Stops the active run.
pub async fn stop_agent_run() -> Result<(), String> {
    send_empty(Request::delete("/agent-runs"), "DELETE /agent-runs").await
}

// -- Settings ------------------------------------------------------------

/// One curated model of the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogModel {
    /// Model id.
    pub id: String,
    /// One short line telling the user when to pick it.
    pub description: String,
    /// True for the model the setup panel selects first.
    pub is_recommended: bool,
}

/// One provider of the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatalogProvider {
    /// Provider name.
    pub name: String,
    /// Provider name as the setup panel shows it.
    pub label: String,
    /// Curated model choices.
    pub models: Vec<CatalogModel>,
    /// True when the provider needs an API key.
    pub needs_api_key: bool,
    /// True when the provider supports the login flow.
    pub supports_login: bool,
}

/// The current model choice.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CurrentSettings {
    /// Chosen provider name.
    pub provider: String,
    /// Chosen model identifier.
    pub model: String,
    /// How it authenticates.
    pub auth: String,
}

/// Response of `GET /settings`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SettingsView {
    /// The model catalog.
    pub providers: Vec<CatalogProvider>,
    /// The current choice, when one was made.
    pub current: Option<CurrentSettings>,
    /// True when the server found Chrome.
    #[serde(default)]
    pub has_chrome: bool,
}

/// Fetches the model catalog and the current choice.
pub async fn fetch_settings() -> Result<SettingsView, String> {
    get_json("/settings").await
}

/// Body of `PUT /settings`.
#[derive(Debug, Serialize)]
struct SettingsRequest<'value> {
    provider: &'value str,
    model: &'value str,
    api_key: Option<&'value str>,
}

/// Saves the model choice with an optional API key.
pub async fn save_settings(
    provider: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let builder = Request::put("/settings")
        .json(&SettingsRequest {
            provider,
            model,
            api_key,
        })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "PUT /settings").await.map(|_| ())
}

/// Body of `POST /settings/models`.
#[derive(Debug, Serialize)]
struct ModelsRequest<'value> {
    provider: &'value str,
    api_key: Option<&'value str>,
}

/// Lists the provider's live models.
pub async fn fetch_provider_models(
    provider: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>, String> {
    let builder = Request::post("/settings/models")
        .json(&ModelsRequest { provider, api_key })
        .map_err(|error| error.to_string())?;
    let response = send_checked(builder, "POST /settings/models").await?;
    response.json().await.map_err(|error| error.to_string())
}

/// Response of the login-start routes.
#[derive(Debug, Deserialize)]
struct LoginStartResponse {
    authorize_url: String,
}

/// Starts a login and returns the URL to open.
async fn start_login_at(route: &str) -> Result<String, String> {
    let response = send_checked(built(Request::post(route))?, route).await?;
    response
        .json::<LoginStartResponse>()
        .await
        .map(|start| start.authorize_url)
        .map_err(|error| error.to_string())
}

/// Starts a Claude login.
pub async fn start_login() -> Result<String, String> {
    start_login_at("/settings/login/start").await
}

/// Starts an OpenRouter login.
pub async fn start_openrouter_login() -> Result<String, String> {
    start_login_at("/settings/login/openrouter/start").await
}

/// Starts a ChatGPT login.
pub async fn start_openai_login() -> Result<String, String> {
    start_login_at("/settings/login/openai/start").await
}

/// Body of `POST /settings/login/complete`.
#[derive(Debug, Serialize)]
struct LoginCompleteRequest<'value> {
    code: &'value str,
    model: Option<&'value str>,
}

/// Completes a Claude login with the pasted code.
pub async fn complete_login(code: &str, model: Option<&str>) -> Result<(), String> {
    let builder = Request::post("/settings/login/complete")
        .json(&LoginCompleteRequest { code, model })
        .map_err(|error| error.to_string())?;
    send_checked(builder, "POST /settings/login/complete")
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use crate::api::{SessionOptions, VARIATION_LIMIT};

    #[test]
    fn the_variation_count_defaults_to_two_and_stays_inside_the_limit() {
        assert_eq!(SessionOptions::default().variation_count(), 2);
        let counted = SessionOptions {
            variations: Some(4),
            ..SessionOptions::default()
        };
        assert_eq!(counted.variation_count(), 4);
        let too_many = SessionOptions {
            variations: Some(99),
            ..SessionOptions::default()
        };
        assert_eq!(too_many.variation_count(), VARIATION_LIMIT);
        let none = SessionOptions {
            variations: Some(0),
            ..SessionOptions::default()
        };
        assert_eq!(none.variation_count(), 1);
    }
}
