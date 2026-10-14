use super::*;

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
    /// True while the prompt waits for its own turn.
    #[serde(default)]
    pub is_queued: bool,
    /// When the turn was recorded, RFC 3339.
    #[serde(default)]
    pub at: Option<String>,
    /// The artifacts an assistant turn wrote, edited, or finished.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Exact pre-edit snapshots for saved edit reports. None for older reports.
    #[serde(default)]
    pub snapshot_stamps: Option<HashMap<String, String>>,
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
    /// The platform a social is posted on, one of `PLATFORMS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// The canvas a social is laid out on, one of `FORMATS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// What a social is for, one of `POST_GOALS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_goal: Option<String>,
    /// How many frames a social run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_count: Option<u32>,
    /// What kind of print piece to lay out, one of `PRINT_KINDS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_kind: Option<String>,
    /// The paper size a print is laid out on, one of `PRINT_SIZES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_size: Option<String>,
    /// How a print's sheets are turned, one of `ORIENTATIONS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    /// How many sheets a print run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet_count: Option<u32>,
    /// What kind of email to write, one of `EMAIL_KINDS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_kind: Option<String>,
    /// The canvas an email is laid out on, one of `EMAIL_FORMATS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_format: Option<String>,
    /// How many emails a mailing run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_count: Option<u32>,
    /// What kind of ad to write, one of `AD_KINDS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ad_kind: Option<String>,
    /// The canvas an ad is laid out on, one of `AD_SIZES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ad_size: Option<String>,
    /// How many ads a campaign run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ad_count: Option<u32>,
    /// What kind of cover to write, one of `COVER_KINDS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_kind: Option<String>,
    /// The canvas a cover is laid out on, one of `COVER_SIZES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_size: Option<String>,
    /// How many covers an artwork run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_count: Option<u32>,
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
            platform: None,
            format: None,
            post_goal: None,
            frame_count: None,
            print_kind: None,
            print_size: None,
            orientation: None,
            sheet_count: None,
            email_kind: None,
            email_format: None,
            email_count: None,
            ad_kind: None,
            ad_size: None,
            ad_count: None,
            cover_kind: None,
            cover_size: None,
            cover_count: None,
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
    /// The socials that belong to this session. Empty unless the
    /// session is a social session.
    #[serde(default)]
    pub socials: Vec<SocialSummary>,
    /// The prints that belong to this session. Empty unless the
    /// session is a print session.
    #[serde(default)]
    pub prints: Vec<PrintSummary>,
    /// The mailings that belong to this session. Empty unless the
    /// session is a mailing session.
    #[serde(default)]
    pub mailings: Vec<MailingSummary>,
    /// The campaigns that belong to this session. Empty unless the
    /// session is a campaign session.
    #[serde(default)]
    pub campaigns: Vec<CampaignSummary>,
    /// The artworks that belong to this session. Empty unless the
    /// session is an artwork session.
    #[serde(default)]
    pub artworks: Vec<ArtworkSummary>,
}

/// Body of `POST /sessions`.
#[derive(Debug, Serialize)]
pub struct CreateSessionRequest<'value> {
    /// The user's request.
    pub request: &'value str,
    /// `demo`, `deck`, `document`, `social`, `print`, `mailing`,
    /// `campaign`, or `artwork`.
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
