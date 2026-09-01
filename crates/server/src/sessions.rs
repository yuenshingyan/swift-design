//! Sessions: the persisted state of one brief-first design workflow.
//!
//! Each session is one project. Its id is the project slug, which is
//! also the design-id prefix its designs share. Everything for a
//! session lives under `data/sessions/{id}/`: the session record, the
//! brief revisions, the question sets, the answers, the chat, and the
//! run records. The workflow state is authoritative and changes only
//! through `apply`, which calls `design_model::transition`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use design_model::{
    ArtifactKind, BriefQuestion, BriefQuestionSet, QuestionAnswer, WorkflowError, WorkflowEvent,
    WorkflowState, app_axes, transition,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::designs::is_valid_design_id;

/// The candidate-id marker: a session id may not contain it, so a
/// session id and a candidate id never collide.
pub const CANDIDATE_MARKER: &str = "-candidate-";

/// Which engine a run uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// The generation engine: plans the turn, asks, writes, or edits.
    /// Records from before the planner say `briefing`; they read as
    /// this.
    #[serde(alias = "briefing")]
    Generation,
}

impl RunMode {
    /// The snake_case name used in JSON and env vars.
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Generation => "generation",
        }
    }
}

/// How the user set up a run: the same options across the session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunOptions {
    /// How hard to work: `low`, `medium`, or `high`.
    #[serde(default = "default_effort")]
    pub effort: String,
    /// How many candidate designs to write. `None` means the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variations: Option<usize>,
    /// How different the candidates should be: `low`, `medium`, `high`.
    #[serde(default = "default_effort")]
    pub variety: String,
    /// Template ids the candidates follow.
    #[serde(default)]
    pub templates: Vec<String>,
    /// True to write preview candidates first.
    #[serde(default = "default_true")]
    pub preview: bool,
    /// The canvases a demo run builds for, such as `desktop web` and
    /// `phone`. One design is written per canvas per variation. Empty
    /// means the default desktop canvas.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// How many slides a deck run writes. `None` leaves it to the
    /// agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide_count: Option<u32>,
    /// The scenario a deck is for, one of the presets. `None` leaves
    /// it to the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    /// Who the artifact is for, one of `AUDIENCES`. `None` leaves it to
    /// the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    /// The tone the artifact takes, one of `TONES`. `None` leaves it to
    /// the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    /// How much of a demo to build, one of `DEMO_SCOPES`. A deck says
    /// its size with `slide_count`. `None` leaves it to the agent.
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
    /// The axes whose value the planner suggested from the request, by
    /// option key. The card shows them as picked and marks them as
    /// suggested. A pick by the user removes the key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested: Vec<String>,
}

impl RunOptions {
    /// The stored value of the axis whose option is `key`, or `None`
    /// for a key that names no axis.
    pub fn axis(&self, key: &str) -> Option<&Option<String>> {
        Some(match key {
            "audience" => &self.audience,
            "tone" => &self.tone,
            "color_mode" => &self.color_mode,
            "scope" => &self.scope,
            "product_kind" => &self.product_kind,
            "data_state" => &self.data_state,
            "fidelity" => &self.fidelity,
            "slide_density" => &self.slide_density,
            "evidence_style" => &self.evidence_style,
            "document_kind" => &self.document_kind,
            "paper" => &self.paper,
            "page_density" => &self.page_density,
            "platform" => &self.platform,
            "format" => &self.format,
            "post_goal" => &self.post_goal,
            "print_kind" => &self.print_kind,
            "print_size" => &self.print_size,
            "orientation" => &self.orientation,
            "email_kind" => &self.email_kind,
            "email_format" => &self.email_format,
            "ad_kind" => &self.ad_kind,
            "ad_size" => &self.ad_size,
            "cover_kind" => &self.cover_kind,
            "cover_size" => &self.cover_size,
            _ => return None,
        })
    }

    /// The writable slot of the axis whose option is `key`.
    fn axis_slot_mut(&mut self, key: &str) -> Option<&mut Option<String>> {
        Some(match key {
            "audience" => &mut self.audience,
            "tone" => &mut self.tone,
            "color_mode" => &mut self.color_mode,
            "scope" => &mut self.scope,
            "product_kind" => &mut self.product_kind,
            "data_state" => &mut self.data_state,
            "fidelity" => &mut self.fidelity,
            "slide_density" => &mut self.slide_density,
            "evidence_style" => &mut self.evidence_style,
            "document_kind" => &mut self.document_kind,
            "paper" => &mut self.paper,
            "page_density" => &mut self.page_density,
            "platform" => &mut self.platform,
            "format" => &mut self.format,
            "post_goal" => &mut self.post_goal,
            "print_kind" => &mut self.print_kind,
            "print_size" => &mut self.print_size,
            "orientation" => &mut self.orientation,
            "email_kind" => &mut self.email_kind,
            "email_format" => &mut self.email_format,
            "ad_kind" => &mut self.ad_kind,
            "ad_size" => &mut self.ad_size,
            "cover_kind" => &mut self.cover_kind,
            "cover_size" => &mut self.cover_size,
            _ => return None,
        })
    }

    /// The app-owned axis values, as (prompt name, stored value) pairs,
    /// for the axes that apply to `kind`. An axis the user has not
    /// picked is absent, so the agent decides it.
    pub fn axes(&self, kind: ArtifactKind) -> Vec<(&'static str, &str)> {
        app_axes(kind)
            .filter_map(|axis| {
                self.axis(axis.key)
                    .and_then(|value| value.as_deref())
                    .map(|value| (axis.name, value))
            })
            .collect()
    }

    /// Fills the blank axes of `kind` from `suggestions`, as (option
    /// key, value) pairs, and records them as suggested. An axis the
    /// user has already answered keeps its answer. A key or a value
    /// the fixed lists do not carry is ignored. Returns the keys that
    /// were filled.
    pub fn suggest(&mut self, kind: ArtifactKind, suggestions: &[(String, String)]) -> Vec<String> {
        let mut filled = Vec::new();
        for (key, value) in suggestions {
            let Some(axis) = app_axes(kind).find(|axis| axis.key == key) else {
                continue;
            };
            if !axis.choices.iter().any(|(known, _)| known == value) {
                continue;
            }
            let Some(slot) = self.axis_slot_mut(key) else {
                continue;
            };
            if slot.is_some() {
                continue;
            }
            *slot = Some(value.clone());
            if !self.suggested.iter().any(|known| known == key) {
                self.suggested.push(key.clone());
            }
            filled.push(key.clone());
        }
        filled
    }
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            effort: default_effort(),
            variations: None,
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
            variety: default_effort(),
            templates: Vec::new(),
            preview: true,
        }
    }
}

impl RunOptions {
    /// The candidate count to write: the chosen count, or two.
    pub fn variation_count(&self) -> usize {
        self.variations.unwrap_or(2).max(1)
    }
}

/// The effort or variety level when none is set.
fn default_effort() -> String {
    "medium".to_owned()
}

/// The default for a boolean field that is on when absent.
fn default_true() -> bool {
    true
}

/// One conversation turn kept with the session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    /// `user` or `assistant`.
    pub role: String,
    /// The turn text.
    pub content: String,
    /// The design open in the editor when the user sent this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,
    /// The candidates the user pinned with `@` in the session chat. The
    /// turn edits each of them. Empty when the user pinned none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<String>,
    /// The number of the question set this turn posed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_set: Option<u32>,
    /// True when the user asked to finish the preview named in
    /// `design`. A plain message with a design open is an edit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_continue: bool,
    /// True when the user asked to write the units named in the content
    /// anew, in the artifact named in `design`. The run skips the
    /// planner and rewrites them without their old markup.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_regenerate: bool,
    /// When the turn was recorded, RFC 3339. Set on append.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The artifacts an assistant turn wrote, edited, or finished. The
    /// studio reverts the turn by restoring each one's snapshot from
    /// before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

impl ChatMessage {
    /// A user turn, optionally about the design open in the editor.
    pub fn user(content: &str, design: Option<&str>) -> Self {
        Self {
            role: "user".to_owned(),
            content: content.to_owned(),
            design: design.map(str::to_owned),
            question_set: None,
            is_continue: false,
            is_regenerate: false,
            at: None,
            artifacts: Vec::new(),
            pinned: Vec::new(),
        }
    }

    /// A user turn that asks to finish the preview `design`.
    pub fn continue_request(content: &str, design: &str) -> Self {
        Self {
            is_continue: true,
            ..Self::user(content, Some(design))
        }
    }

    /// A user turn that asks to write the units named in `content`
    /// anew, in `design`.
    pub fn regenerate_request(content: &str, design: &str) -> Self {
        Self {
            is_regenerate: true,
            ..Self::user(content, Some(design))
        }
    }

    /// An assistant turn.
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: content.to_owned(),
            design: None,
            question_set: None,
            is_continue: false,
            is_regenerate: false,
            at: None,
            artifacts: Vec::new(),
            pinned: Vec::new(),
        }
    }

    /// The same user turn, naming the candidates the user pinned.
    pub fn with_pinned(mut self, pinned: Vec<String>) -> Self {
        self.pinned = pinned;
        self
    }

    /// The same turn, naming the artifacts it wrote.
    pub fn with_artifacts(mut self, artifacts: Vec<String>) -> Self {
        self.artifacts = artifacts;
        self
    }

    /// An assistant turn that posed question set `number`.
    pub fn assistant_questions(content: &str, number: u32) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: content.to_owned(),
            design: None,
            question_set: Some(number),
            is_continue: false,
            is_regenerate: false,
            at: None,
            artifacts: Vec::new(),
            pinned: Vec::new(),
        }
    }
}

/// One recorded set of answers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerRecord {
    /// The question set answered.
    pub question_set: u32,
    /// The answers.
    pub answers: Vec<QuestionAnswer>,
    /// When the answers arrived, as an RFC 3339 UTC string.
    pub at: String,
}

/// One generation or briefing run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique id: `{unix_seconds}-{n}`.
    pub run_id: String,
    /// Which engine ran.
    pub mode: RunMode,
    /// `built-in` or `custom`.
    pub runtime: String,
    /// Provider name for a built-in run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model id for a built-in run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When it started, as an RFC 3339 UTC string.
    pub started_at: String,
    /// When it finished, when it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// The outcome: `succeeded`, `failed`, `asked_questions`,
    /// `brief_presented`, `needs_clarification`, or `stopped`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The failure message, when the run failed. Ids only, never paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The design ids the run wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

/// The persisted session record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// The session id, which is the project slug.
    pub id: String,
    /// A short title, from the request.
    pub title: String,
    /// The user's request, in their words.
    pub request: String,
    /// What the session builds: a software demo (designs) or a deck.
    /// Set at creation; the brief carries the same value.
    #[serde(default)]
    pub artifact_kind: ArtifactKind,
    /// Where the session is in the workflow.
    pub state: WorkflowState,
    /// The state to return to when the session resumes from `stopped`
    /// or `error`. Records written before the stop state existed name
    /// this `state_before_error`.
    #[serde(
        default,
        alias = "state_before_error",
        skip_serializing_if = "Option::is_none"
    )]
    pub resume_state: Option<WorkflowState>,
    /// The failure message shown in the error state. Ids only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the session was created, as an RFC 3339 UTC string.
    pub created_at: String,
    /// When it last changed, as an RFC 3339 UTC string.
    pub updated_at: String,
    /// The design the user chose from the candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_design: Option<String>,
    /// The number of the newest question set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_question_set: Option<u32>,
    /// The run options for this session.
    #[serde(default)]
    pub options: RunOptions,
}

/// One row of `GET /sessions`.
#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    /// The session id.
    pub id: String,
    /// The title.
    pub title: String,
    /// What the session builds.
    pub artifact_kind: ArtifactKind,
    /// The workflow state.
    pub state: WorkflowState,
    /// When it was created, RFC 3339.
    pub created_at: String,
    /// When it last changed, RFC 3339.
    pub updated_at: String,
    /// The chosen design or deck, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_design: Option<String>,
}

/// What a new session is made from.
#[derive(Clone, Debug)]
pub struct NewSession<'a> {
    /// The session id, which is the project slug.
    pub id: &'a str,
    /// A short title.
    pub title: &'a str,
    /// The user's request, in their words.
    pub request: &'a str,
    /// The run options.
    pub options: RunOptions,
    /// What the session builds.
    pub artifact_kind: ArtifactKind,
}

impl<'a> NewSession<'a> {
    /// A demo session with default options.
    pub fn demo(id: &'a str, title: &'a str, request: &'a str) -> Self {
        Self {
            id,
            title,
            request,
            options: RunOptions::default(),
            artifact_kind: ArtifactKind::Demo,
        }
    }

    /// The same session with these run options.
    pub fn with_options(mut self, options: RunOptions) -> Self {
        self.options = options;
        self
    }

    /// The same session building this kind of artifact.
    pub fn with_kind(mut self, artifact_kind: ArtifactKind) -> Self {
        self.artifact_kind = artifact_kind;
        self
    }
}

/// The full view of one session for `GET /sessions/{id}`.
#[derive(Clone, Debug, Serialize)]
pub struct SessionView {
    /// The session record.
    pub session: Session,
    /// Every question set asked, in order.
    pub question_sets: Vec<BriefQuestionSet>,
    /// The number of the question set still open, when one is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_question_set: Option<u32>,
    /// Every recorded answer set.
    pub answers: Vec<AnswerRecord>,
    /// The conversation.
    pub messages: Vec<ChatMessage>,
    /// The run records.
    pub runs: Vec<RunRecord>,
    /// The designs that belong to this session. Empty unless the session
    /// is a demo session.
    pub designs: Vec<crate::designs::DesignSummary>,
    /// The decks that belong to this session. Empty unless the session
    /// is a deck session.
    pub decks: Vec<crate::decks::DeckSummary>,
    /// The documents that belong to this session. Empty unless the
    /// session is a document session.
    pub documents: Vec<crate::documents::DocumentSummary>,
    /// The socials that belong to this session. Empty unless the
    /// session is a social session.
    pub socials: Vec<crate::socials::SocialSummary>,
    /// The prints that belong to this session. Empty unless the
    /// session is a print session.
    pub prints: Vec<crate::prints::PrintSummary>,
    /// The mailings that belong to this session. Empty unless the
    /// session is a mailing session.
    pub mailings: Vec<crate::mailings::MailingSummary>,
    /// The campaigns that belong to this session. Empty unless the
    /// session is a campaign session.
    pub campaigns: Vec<crate::campaigns::CampaignSummary>,
    /// The artworks that belong to this session. Empty unless the
    /// session is an artwork session.
    pub artworks: Vec<crate::artworks::ArtworkSummary>,
}

/// What went wrong in a session operation.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// No session with that id.
    #[error("no session `{id}`: create it with POST /sessions")]
    NotFound {
        /// The missing id.
        id: String,
    },
    /// A session already exists with that id.
    #[error("session `{id}` already exists")]
    AlreadyExists {
        /// The id in use.
        id: String,
    },
    /// The workflow refused the event.
    #[error(transparent)]
    Workflow(#[from] WorkflowError),
    /// A storage failure. The message names no path.
    #[error("session storage failed: {0}")]
    Io(String),
}

impl SessionError {
    /// Wraps a storage error without leaking a path into the message.
    fn io(error: impl std::fmt::Display) -> Self {
        SessionError::Io(error.to_string())
    }
}

/// Filesystem-backed session storage. A per-store lock serializes the
/// read-modify-write of a session record.
#[derive(Clone)]
pub struct SessionStore {
    directory: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl SessionStore {
    /// Creates a store over `directory`. The directory is created on
    /// the first write.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    fn session_directory(&self, id: &str) -> PathBuf {
        self.directory.join(id)
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("session.json")
    }

    fn question_set_path(&self, id: &str, number: u32) -> PathBuf {
        self.session_directory(id)
            .join("question-sets")
            .join(format!("{number}.json"))
    }

    fn answers_path(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("answers.json")
    }

    fn messages_path(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("messages.json")
    }

    fn runs_directory(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("runs")
    }

    /// Creates a session in the intake state. Fails when one exists.
    pub async fn create(&self, new: NewSession<'_>) -> Result<Session, SessionError> {
        let _guard = self.write_lock.lock().await;
        let id = new.id;
        if self.session_path(id).exists() {
            return Err(SessionError::AlreadyExists { id: id.to_owned() });
        }
        let now = crate::time::rfc3339_now();
        let session = Session {
            id: id.to_owned(),
            title: new.title.to_owned(),
            request: new.request.to_owned(),
            artifact_kind: new.artifact_kind,
            state: WorkflowState::Intake,
            resume_state: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
            chosen_design: None,
            latest_question_set: None,
            options: new.options,
        };
        self.write_session(&session).await?;
        Ok(session)
    }

    /// Reads a session record. `Ok(None)` means none exists.
    pub async fn read(&self, id: &str) -> Result<Option<Session>, SessionError> {
        match tokio::fs::read_to_string(self.session_path(id)).await {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(SessionError::io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::io(error)),
        }
    }

    /// Reads a session or fails with `NotFound`.
    async fn require(&self, id: &str) -> Result<Session, SessionError> {
        self.read(id)
            .await?
            .ok_or_else(|| SessionError::NotFound { id: id.to_owned() })
    }

    async fn write_session(&self, session: &Session) -> Result<(), SessionError> {
        let directory = self.session_directory(&session.id);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(SessionError::io)?;
        let json = serde_json::to_string_pretty(session).map_err(SessionError::io)?;
        crate::files::write_atomically(&self.session_path(&session.id), &json)
            .await
            .map_err(SessionError::io)
    }

    /// Lists every session, newest change first.
    pub async fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SessionError::io(error)),
        };
        let mut summaries = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(SessionError::io)? {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(session) = self.read(&id).await? {
                summaries.push(SessionSummary {
                    id: session.id,
                    title: session.title,
                    artifact_kind: session.artifact_kind,
                    state: session.state,
                    created_at: session.created_at,
                    updated_at: session.updated_at,
                    chosen_design: session.chosen_design,
                });
            }
        }
        summaries.sort_by(|first, second| second.updated_at.cmp(&first.updated_at));
        Ok(summaries)
    }

    /// Removes a session and everything under it.
    pub async fn delete(&self, id: &str) -> Result<bool, SessionError> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::remove_dir_all(self.session_directory(id)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SessionError::io(error)),
        }
    }

    /// Renames a session directory, so its id keeps matching its
    /// designs. `Ok(false)` when there is no session `old`.
    pub async fn rename(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        let _guard = self.write_lock.lock().await;
        let Some(mut session) = self.read(old).await? else {
            return Ok(false);
        };
        tokio::fs::rename(self.session_directory(old), self.session_directory(new))
            .await
            .map_err(SessionError::io)?;
        session.id = new.to_owned();
        session.updated_at = crate::time::rfc3339_now();
        self.write_session(&session).await?;
        Ok(true)
    }

    /// Applies a workflow event through `design_model::transition`, the
    /// only place the state changes. Records the previous state on an
    /// error, and clears the error on recovery.
    pub async fn apply(&self, id: &str, event: WorkflowEvent) -> Result<Session, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut session = self.require(id).await?;
        let next = transition(session.state, event)?;
        // A halt records where to come back to; a resume clears both
        // the marker and any failure message.
        if matches!(event, WorkflowEvent::RunFailed | WorkflowEvent::RunStopped) {
            session.resume_state = Some(session.state);
        }
        if matches!(event, WorkflowEvent::Recovered { .. }) {
            session.resume_state = None;
            session.error = None;
        }
        session.state = next;
        session.updated_at = crate::time::rfc3339_now();
        self.write_session(&session).await?;
        Ok(session)
    }

    /// Edits the non-state fields of a session under the lock.
    /// Clears `artifact_id` off the session that chose it.
    ///
    /// A deleted candidate must not stay the session's choice: the
    /// planner reads that field to decide which artifact an edit turn
    /// changes.
    pub async fn forget_artifact(&self, artifact_id: &str) {
        let session_id = session_id_of_artifact(artifact_id).to_owned();
        let _ = self
            .update(&session_id, |session| {
                if session.chosen_design.as_deref() == Some(artifact_id) {
                    session.chosen_design = None;
                }
            })
            .await;
    }

    pub async fn update(
        &self,
        id: &str,
        edit: impl FnOnce(&mut Session),
    ) -> Result<Session, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut session = self.require(id).await?;
        edit(&mut session);
        session.updated_at = crate::time::rfc3339_now();
        self.write_session(&session).await?;
        Ok(session)
    }

    /// The state to resume into after a stop or an error: the state the
    /// run halted in, or intake. A halt in `generating` resumes in
    /// `reviewing`: a run that starts in `generating` writes candidates
    /// without a planner turn, and that overwrote finished candidates
    /// when the user's next message was an edit.
    pub async fn recovery_target(&self, id: &str) -> Result<WorkflowState, SessionError> {
        let session = self.require(id).await?;
        Ok(match session.resume_state {
            Some(WorkflowState::Generating) => WorkflowState::Reviewing,
            Some(state) => state,
            None => WorkflowState::Intake,
        })
    }

    /// Writes the next question set. Returns its number.
    pub async fn write_question_set(
        &self,
        id: &str,
        set: &BriefQuestionSet,
    ) -> Result<u32, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut session = self.require(id).await?;
        let number = session.latest_question_set.unwrap_or(0) + 1;
        let directory = self.session_directory(id).join("question-sets");
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(SessionError::io)?;
        let json = serde_json::to_string_pretty(set).map_err(SessionError::io)?;
        crate::files::write_atomically(&self.question_set_path(id, number), &json)
            .await
            .map_err(SessionError::io)?;
        session.latest_question_set = Some(number);
        session.updated_at = crate::time::rfc3339_now();
        self.write_session(&session).await?;
        Ok(number)
    }

    /// Reads one question set.
    pub async fn read_question_set(
        &self,
        id: &str,
        number: u32,
    ) -> Result<Option<BriefQuestionSet>, SessionError> {
        match tokio::fs::read_to_string(self.question_set_path(id, number)).await {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(SessionError::io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::io(error)),
        }
    }

    /// Every question set, in order.
    pub async fn question_sets(&self, id: &str) -> Result<Vec<BriefQuestionSet>, SessionError> {
        let session = self.require(id).await?;
        let mut sets = Vec::new();
        for number in 1..=session.latest_question_set.unwrap_or(0) {
            if let Some(set) = self.read_question_set(id, number).await? {
                sets.push(set);
            }
        }
        Ok(sets)
    }

    /// Records a set of answers.
    pub async fn record_answers(
        &self,
        id: &str,
        question_set: u32,
        answers: Vec<QuestionAnswer>,
    ) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut records = self.answers(id).await?;
        records.push(AnswerRecord {
            question_set,
            answers,
            at: crate::time::rfc3339_now(),
        });
        self.write_json(&self.answers_path(id), &records).await
    }

    /// Every recorded answer set.
    pub async fn answers(&self, id: &str) -> Result<Vec<AnswerRecord>, SessionError> {
        self.read_json(&self.answers_path(id)).await
    }

    /// The questions and the user's latest answer for each, joined
    /// across every set. A later answer for the same question wins.
    pub async fn answered_questions(
        &self,
        id: &str,
    ) -> Result<Vec<(BriefQuestion, QuestionAnswer)>, SessionError> {
        let sets = self.question_sets(id).await?;
        let records = self.answers(id).await?;
        let mut joined: Vec<(BriefQuestion, QuestionAnswer)> = Vec::new();
        for record in records {
            let Some(set) = sets.get((record.question_set as usize).wrapping_sub(1)) else {
                continue;
            };
            for answer in record.answers {
                let Some(question) = set
                    .questions
                    .iter()
                    .find(|question| question.id == answer.question_id)
                else {
                    continue;
                };
                joined.retain(|(existing, _)| existing.id != question.id);
                joined.push((question.clone(), answer));
            }
        }
        Ok(joined)
    }

    /// Appends one conversation turn.
    pub async fn append_message(
        &self,
        id: &str,
        mut message: ChatMessage,
    ) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut messages = self.messages(id).await?;
        if message.at.is_none() {
            message.at = Some(crate::time::rfc3339_now());
        }
        messages.push(message);
        self.write_json(&self.messages_path(id), &messages).await
    }

    /// The conversation.
    pub async fn messages(&self, id: &str) -> Result<Vec<ChatMessage>, SessionError> {
        self.read_json(&self.messages_path(id)).await
    }

    /// Records the start of a run. Returns its id.
    pub async fn start_run(&self, id: &str, mut record: RunRecord) -> Result<String, SessionError> {
        let _guard = self.write_lock.lock().await;
        let directory = self.runs_directory(id);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(SessionError::io)?;
        let existing = self.runs(id).await?.len();
        record.run_id = format!("{}-{}", crate::time::unix_now_seconds(), existing + 1);
        let json = serde_json::to_string_pretty(&record).map_err(SessionError::io)?;
        crate::files::write_atomically(&directory.join(format!("{}.json", record.run_id)), &json)
            .await
            .map_err(SessionError::io)?;
        Ok(record.run_id)
    }

    /// Records the end of a run.
    pub async fn finish_run(
        &self,
        id: &str,
        run_id: &str,
        result: &str,
        error: Option<String>,
        artifacts: Vec<String>,
    ) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let path = self.runs_directory(id).join(format!("{run_id}.json"));
        let Some(mut record) = self.read_json_file::<RunRecord>(&path).await? else {
            return Ok(());
        };
        record.finished_at = Some(crate::time::rfc3339_now());
        record.result = Some(result.to_owned());
        record.error = error;
        record.artifacts = artifacts;
        let json = serde_json::to_string_pretty(&record).map_err(SessionError::io)?;
        crate::files::write_atomically(&path, &json)
            .await
            .map_err(SessionError::io)
    }

    /// Stops every run the last process left unfinished. A session in
    /// `generating` whose latest run has no end was killed with the
    /// server: nothing will finish it, and the session refuses messages
    /// until it halts. Returns the ids stopped.
    pub async fn stop_orphaned_runs(&self) -> Result<Vec<String>, SessionError> {
        let mut stopped = Vec::new();
        for summary in self.list().await? {
            if summary.state != WorkflowState::Generating {
                continue;
            }
            let orphans: Vec<RunRecord> = self
                .runs(&summary.id)
                .await?
                .into_iter()
                .filter(|record| record.finished_at.is_none())
                .collect();
            for record in &orphans {
                self.finish_run(
                    &summary.id,
                    &record.run_id,
                    "stopped",
                    Some("the server stopped while the run worked".to_owned()),
                    Vec::new(),
                )
                .await?;
            }
            self.apply(&summary.id, WorkflowEvent::RunStopped).await?;
            stopped.push(summary.id);
        }
        Ok(stopped)
    }

    /// Every run record, oldest first.
    pub async fn runs(&self, id: &str) -> Result<Vec<RunRecord>, SessionError> {
        let mut entries = match tokio::fs::read_dir(self.runs_directory(id)).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SessionError::io(error)),
        };
        let mut records = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(SessionError::io)? {
            if let Some(record) = self.read_json_file::<RunRecord>(&entry.path()).await? {
                records.push(record);
            }
        }
        records.sort_by(|first, second| first.run_id.cmp(&second.run_id));
        Ok(records)
    }

    async fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), SessionError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(SessionError::io)?;
        }
        let json = serde_json::to_string_pretty(value).map_err(SessionError::io)?;
        crate::files::write_atomically(path, &json)
            .await
            .map_err(SessionError::io)
    }

    async fn read_json<T: for<'de> Deserialize<'de> + Default>(
        &self,
        path: &Path,
    ) -> Result<T, SessionError> {
        Ok(self.read_json_file(path).await?.unwrap_or_default())
    }

    async fn read_json_file<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
    ) -> Result<Option<T>, SessionError> {
        match tokio::fs::read_to_string(path).await {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(SessionError::io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::io(error)),
        }
    }
}

/// Checks that a design or deck write is allowed now: the owning session
/// must be generating (agent writes), or generating or reviewing (user
/// writes from the editor). The message names the session state so the
/// caller can report it.
pub async fn write_access(
    sessions: &SessionStore,
    artifact_id: &str,
    is_user: bool,
) -> Result<(), String> {
    let session_id = session_id_of_artifact(artifact_id);
    let session = sessions
        .read(session_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no session `{session_id}`: create it with POST /sessions"))?;
    let allowed = session.state == WorkflowState::Generating
        || (is_user && session.state == WorkflowState::Reviewing);
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "writes to `{artifact_id}` are not allowed while session `{session_id}` is in state `{}`",
            session.state
        ))
    }
}

/// The run mode for a workflow state: generation in every state that
/// can take a turn or is writing, and nothing in a halted one. A
/// stopped or failed session resumes before it runs again.
pub fn run_mode_for(state: WorkflowState) -> Option<RunMode> {
    match state {
        state if state.is_halted() => None,
        _ => Some(RunMode::Generation),
    }
}

/// True for ids that are valid session ids: a valid design id that is
/// not a candidate id, so a session and a candidate never collide.
pub fn is_valid_session_id(id: &str) -> bool {
    is_valid_design_id(id) && !id.contains(CANDIDATE_MARKER)
}

/// The session id a design or deck belongs to: the artifact id with
/// any `-candidate-N` suffix removed.
pub fn session_id_of_artifact(artifact_id: &str) -> &str {
    match artifact_id.find(CANDIDATE_MARKER) {
        Some(position) => &artifact_id[..position],
        None => artifact_id,
    }
}

/// A fresh session id: a random version 4 UUID.
///
/// The id is a key, not a name. The name a user reads is `title`, and
/// the times are `created_at` and `updated_at`. An id built from the
/// request text made the same request produce the same id, so two
/// sessions with the same wording collided, and a deleted session left
/// its files for the next one to adopt.
pub fn new_session_id() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| anyhow::anyhow!("no randomness source: {error}"))?;
    // The version and variant bits of RFC 9562 version 4.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        id.push_str(&format!("{byte:02x}"));
    }
    Ok(id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_user_turn_keeps_the_candidates_it_pinned() {
        let message = ChatMessage::user("Bigger title.", None).with_pinned(vec![
            "talk-candidate-1".to_owned(),
            "talk-candidate-3".to_owned(),
        ]);
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["pinned"][1], "talk-candidate-3");
        let back: ChatMessage = serde_json::from_value(json).unwrap();
        assert_eq!(back, message);
        let plain = serde_json::to_value(ChatMessage::user("Hi.", None)).unwrap();
        assert!(plain.get("pinned").is_none());
    }

    #[test]
    fn a_regenerate_turn_names_its_artifact_and_round_trips() {
        let message =
            ChatMessage::regenerate_request("[screen 2] Write it anew.", "talk-candidate-1");
        assert!(message.is_regenerate);
        assert!(!message.is_continue);
        assert_eq!(message.design.as_deref(), Some("talk-candidate-1"));
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["is_regenerate"], true);
        let back: ChatMessage = serde_json::from_value(json).unwrap();
        assert_eq!(back, message);
        let plain = serde_json::to_value(ChatMessage::user("Hi.", None)).unwrap();
        assert!(plain.get("is_regenerate").is_none());
    }

    #[test]
    fn a_suggestion_fills_a_blank_axis_and_marks_it() {
        let mut options = RunOptions::default();
        let filled = options.suggest(
            ArtifactKind::Demo,
            &[
                ("product_kind".to_owned(), "developer_tool".to_owned()),
                ("color_mode".to_owned(), "dark".to_owned()),
            ],
        );
        assert_eq!(filled, ["product_kind", "color_mode"]);
        assert_eq!(options.product_kind.as_deref(), Some("developer_tool"));
        assert_eq!(options.color_mode.as_deref(), Some("dark"));
        assert_eq!(options.suggested, ["product_kind", "color_mode"]);
    }

    #[test]
    fn a_suggestion_never_replaces_an_answer_or_names_an_unknown_value() {
        let mut options = RunOptions {
            product_kind: Some("dashboard".to_owned()),
            ..RunOptions::default()
        };
        let filled = options.suggest(
            ArtifactKind::Demo,
            &[
                ("product_kind".to_owned(), "developer_tool".to_owned()),
                ("scope".to_owned(), "everything".to_owned()),
                ("vibe".to_owned(), "loud".to_owned()),
                // A deck axis is not a demo axis.
                ("audience".to_owned(), "newcomers".to_owned()),
            ],
        );
        assert!(filled.is_empty());
        assert_eq!(options.product_kind.as_deref(), Some("dashboard"));
        assert_eq!(options.scope, None);
        assert_eq!(options.audience, None);
        assert!(options.suggested.is_empty());
    }

    #[test]
    fn the_axes_of_a_demo_leave_out_the_audience_and_the_tone() {
        let options = RunOptions {
            audience: Some("newcomers".to_owned()),
            tone: Some("warm".to_owned()),
            color_mode: Some("dark".to_owned()),
            product_kind: Some("developer_tool".to_owned()),
            ..RunOptions::default()
        };
        assert_eq!(
            options.axes(ArtifactKind::Demo),
            [("Color mode", "dark"), ("Product kind", "developer_tool")]
        );
        assert_eq!(
            options.axes(ArtifactKind::Deck),
            [
                ("Color mode", "dark"),
                ("Audience", "newcomers"),
                ("Tone", "warm")
            ]
        );
    }
    use design_model::{QuestionKind, QuestionOption};

    fn store() -> (tempfile::TempDir, SessionStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("sessions"));
        (directory, store)
    }

    fn question(id: &str) -> BriefQuestion {
        BriefQuestion {
            id: id.to_owned(),
            label: format!("Which {id}?"),
            rationale: None,
            kind: QuestionKind::SingleSelect,
            required: true,
            options: vec![
                QuestionOption {
                    value: "a".to_owned(),
                    label: "A".to_owned(),
                },
                QuestionOption {
                    value: "b".to_owned(),
                    label: "B".to_owned(),
                },
            ],
            allow_other: false,
        }
    }

    fn set(id: &str) -> BriefQuestionSet {
        BriefQuestionSet {
            title: "Q".to_owned(),
            message: "m".to_owned(),
            questions: vec![question(id)],
            can_proceed_with_assumptions: true,
        }
    }

    #[tokio::test]
    async fn creating_a_session_starts_in_intake_and_survives_a_new_store() {
        let (directory, store) = store();
        let session = store
            .create(NewSession::demo(
                "finance-app",
                "Finance app",
                "Design a landing page.",
            ))
            .await
            .unwrap();
        assert_eq!(session.state, WorkflowState::Intake);
        assert_eq!(session.artifact_kind, ArtifactKind::Demo);
        assert!(
            store
                .create(NewSession::demo("finance-app", "x", "y"))
                .await
                .is_err()
        );
        let reopened = SessionStore::new(directory.path().join("sessions"));
        let loaded = reopened.read("finance-app").await.unwrap().unwrap();
        assert_eq!(loaded.request, "Design a landing page.");
        assert_eq!(loaded.state, WorkflowState::Intake);
        assert_eq!(store.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_session_rewrite_replaces_the_record_and_leaves_no_temporary() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        store
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        let names: Vec<String> = std::fs::read_dir(store.session_directory("talk"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|name| !name.contains(".writing")),
            "leftover temporary in {names:?}"
        );
        let loaded = store.read("talk").await.unwrap().unwrap();
        assert_eq!(loaded.state, WorkflowState::Generating);
    }

    #[tokio::test]
    async fn question_sets_number_in_order_and_answers_attach_to_the_latest() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        assert_eq!(
            store
                .write_question_set("talk", &set("platform"))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .write_question_set("talk", &set("audience"))
                .await
                .unwrap(),
            2
        );
        store
            .record_answers(
                "talk",
                2,
                vec![QuestionAnswer {
                    question_id: "audience".to_owned(),
                    values: vec!["a".to_owned()],
                    ..QuestionAnswer::default()
                }],
            )
            .await
            .unwrap();
        let answered = store.answered_questions("talk").await.unwrap();
        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0].0.id, "audience");
        assert_eq!(answered[0].1.values, vec!["a"]);
        assert_eq!(store.question_sets("talk").await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn apply_rejects_illegal_events_and_keeps_the_file() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        assert!(
            store
                .apply("talk", WorkflowEvent::GenerationSucceeded)
                .await
                .is_err()
        );
        let session = store.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Intake);
        let after = store
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        assert_eq!(after.state, WorkflowState::Generating);
    }

    #[tokio::test]
    async fn run_failed_remembers_the_previous_state_for_retry() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        store
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        store
            .update("talk", |session| {
                session.error = Some("boom".to_owned());
            })
            .await
            .unwrap();
        let failed = store.apply("talk", WorkflowEvent::RunFailed).await.unwrap();
        assert_eq!(failed.state, WorkflowState::Error);
        assert_eq!(failed.resume_state, Some(WorkflowState::Generating));
        // The run halted in generating; the user resumes on the canvas.
        assert_eq!(
            store.recovery_target("talk").await.unwrap(),
            WorkflowState::Reviewing
        );
        let recovered = store
            .apply(
                "talk",
                WorkflowEvent::Recovered {
                    to: WorkflowState::Generating,
                },
            )
            .await
            .unwrap();
        assert_eq!(recovered.state, WorkflowState::Generating);
        assert_eq!(recovered.error, None);
        assert_eq!(recovered.resume_state, None);
    }

    #[tokio::test]
    async fn a_stop_halts_without_recording_a_failure() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        store
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        let stopped = store
            .apply("talk", WorkflowEvent::RunStopped)
            .await
            .unwrap();
        assert_eq!(stopped.state, WorkflowState::Stopped);
        // A stop has nothing to report, so no failure message is kept.
        assert_eq!(stopped.error, None);
        assert_eq!(stopped.resume_state, Some(WorkflowState::Generating));
        assert_eq!(
            store.recovery_target("talk").await.unwrap(),
            WorkflowState::Reviewing
        );
        let resumed = store
            .apply(
                "talk",
                WorkflowEvent::Recovered {
                    to: WorkflowState::Generating,
                },
            )
            .await
            .unwrap();
        assert_eq!(resumed.state, WorkflowState::Generating);
        assert_eq!(resumed.resume_state, None);
    }

    #[tokio::test]
    async fn a_record_written_before_the_stop_state_still_resumes() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        store
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        store.apply("talk", WorkflowEvent::RunFailed).await.unwrap();
        // Records written before the rename name the field
        // `state_before_error`; the alias keeps them resumable.
        let raw = std::fs::read_to_string(store.session_path("talk")).unwrap();
        let old = raw.replace("\"resume_state\"", "\"state_before_error\"");
        assert!(old.contains("state_before_error"));
        let loaded: Session = serde_json::from_str(&old).unwrap();
        assert_eq!(loaded.resume_state, Some(WorkflowState::Generating));
    }

    #[test]
    fn a_halted_session_starts_no_run_until_it_resumes() {
        assert_eq!(run_mode_for(WorkflowState::Stopped), None);
        assert_eq!(run_mode_for(WorkflowState::Error), None);
        assert_eq!(
            run_mode_for(WorkflowState::Generating),
            Some(RunMode::Generation)
        );
    }

    #[tokio::test]
    async fn runs_record_revision_runtime_and_result() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        let run_id = store
            .start_run(
                "talk",
                RunRecord {
                    run_id: String::new(),
                    mode: RunMode::Generation,
                    runtime: "built-in".to_owned(),
                    provider: Some("openai".to_owned()),
                    model: Some("gpt-5".to_owned()),
                    started_at: crate::time::rfc3339_now(),
                    finished_at: None,
                    result: None,
                    error: None,
                    artifacts: Vec::new(),
                },
            )
            .await
            .unwrap();
        store
            .finish_run(
                "talk",
                &run_id,
                "succeeded",
                None,
                vec!["talk-candidate-1".to_owned()],
            )
            .await
            .unwrap();
        let runs = store.runs("talk").await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].mode, RunMode::Generation);
        assert_eq!(runs[0].result.as_deref(), Some("succeeded"));
        assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
    }

    #[test]
    fn old_run_modes_read_as_generation() {
        let mode: RunMode = serde_json::from_str("\"briefing\"").unwrap();
        assert_eq!(mode, RunMode::Generation);
    }

    #[tokio::test]
    async fn rename_moves_the_directory() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("old-name", "Old", "A talk."))
            .await
            .unwrap();
        assert!(store.rename("old-name", "new-name").await.unwrap());
        assert!(store.read("old-name").await.unwrap().is_none());
        let moved = store.read("new-name").await.unwrap().unwrap();
        assert_eq!(moved.id, "new-name");
        assert!(!store.rename("missing", "x").await.unwrap());
    }

    #[tokio::test]
    async fn a_session_carries_its_artifact_kind() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk.").with_kind(ArtifactKind::Deck))
            .await
            .unwrap();
        let session = store.read("talk").await.unwrap().unwrap();
        assert_eq!(session.artifact_kind, ArtifactKind::Deck);
        assert_eq!(
            store.list().await.unwrap()[0].artifact_kind,
            ArtifactKind::Deck
        );
        // A record written before the field existed loads as a demo.
        let raw = std::fs::read_to_string(store.session_path("talk")).unwrap();
        let without_kind = raw.replace("\"artifact_kind\": \"deck\",", "");
        let old: Session = serde_json::from_str(&without_kind).unwrap();
        assert_eq!(old.artifact_kind, ArtifactKind::Demo);
    }

    #[test]
    fn session_id_of_artifact_strips_the_candidate_suffix() {
        assert_eq!(session_id_of_artifact("talk-candidate-2"), "talk");
        assert_eq!(session_id_of_artifact("talk"), "talk");
        assert_eq!(
            session_id_of_artifact("finance-app-candidate-10"),
            "finance-app"
        );
    }

    #[test]
    fn session_ids_reject_candidate_suffixes() {
        assert!(is_valid_session_id("finance-app"));
        assert!(!is_valid_session_id("talk-candidate-1"));
        assert!(!is_valid_session_id("render"));
        assert!(!is_valid_session_id("Bad Id"));
    }

    #[tokio::test]
    async fn an_appended_message_is_stamped_with_its_time() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A talk."))
            .await
            .unwrap();
        store
            .append_message("talk", ChatMessage::user("Hello.", None))
            .await
            .unwrap();
        let message = store.messages("talk").await.unwrap().pop().unwrap();
        let at = message.at.expect("a time");
        assert!(at.ends_with('Z') && at.len() == 20, "{at}");
    }

    #[tokio::test]
    async fn a_run_the_last_process_left_behind_is_stopped_at_boot() {
        let (_directory, store) = store();
        store
            .create(NewSession::demo("talk", "Talk", "A landing page."))
            .await
            .unwrap();
        store
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        let record = RunRecord {
            run_id: String::new(),
            mode: RunMode::Generation,
            runtime: "built-in".to_owned(),
            provider: None,
            model: None,
            started_at: crate::time::rfc3339_now(),
            finished_at: None,
            result: None,
            error: None,
            artifacts: Vec::new(),
        };
        let run_id = store.start_run("talk", record).await.unwrap();
        assert_eq!(store.stop_orphaned_runs().await.unwrap(), vec!["talk"]);
        let session = store.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Stopped);
        let runs = store.runs("talk").await.unwrap();
        let record = runs.iter().find(|record| record.run_id == run_id).unwrap();
        assert_eq!(record.result.as_deref(), Some("stopped"));
        assert!(record.finished_at.is_some());
        // A second boot finds nothing to do.
        assert!(store.stop_orphaned_runs().await.unwrap().is_empty());
    }

    #[test]
    fn a_new_session_id_is_a_random_version_4_uuid() {
        let id = new_session_id().unwrap();
        assert_eq!(id.len(), 36);
        let groups: Vec<&str> = id.split('-').collect();
        assert_eq!(
            groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(groups[2].starts_with('4'), "version nibble: {id}");
        assert!(
            matches!(&groups[3][..1], "8" | "9" | "a" | "b"),
            "variant nibble: {id}"
        );
        // The id is a file stem and a URL segment everywhere it goes.
        assert!(is_valid_session_id(&id));
        assert_ne!(id, new_session_id().unwrap());
    }
}
