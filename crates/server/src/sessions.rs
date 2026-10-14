//! Sessions: the persisted state of one brief-first design workflow.
//!
//! Each session is one project. Its id is the project slug, which is
//! also the design-id prefix its designs share. Everything for a
//! session lives under `data/sessions/{id}/`: the session record, the
//! brief revisions, the question sets, the answers, the chat, and the
//! run records. The workflow state is authoritative and changes only
//! through `apply`, which calls `design_model::transition`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "queued_messages_tests.rs"]
mod queued_messages_tests;

mod store;
#[cfg(test)]
mod tests;
pub use store::{
    AnswerRecord, NewSession, RunRecord, Session, SessionError, SessionStore, SessionSummary,
    SessionView, is_valid_session_id, new_session_id, run_mode_for, session_id_of_artifact,
    write_access,
};

use std::sync::Arc;

use design_model::{
    ArtifactKind, BriefQuestion, BriefQuestionSet, QuestionAnswer, WorkflowError, WorkflowEvent,
    WorkflowState, app_axes, transition,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::artworks::ArtworkSummary;
use crate::campaigns::CampaignSummary;
use crate::decks::DeckSummary;
use crate::designs::{DesignSummary, is_valid_design_id};
use crate::documents::DocumentSummary;
use crate::files::write_atomically;
use crate::mailings::MailingSummary;
use crate::prints::PrintSummary;
use crate::socials::SocialSummary;
use crate::time::{rfc3339_now, unix_now_seconds};

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
    /// True for known app products outside the standalone landing-page scope.
    pub fn should_have_app_window(&self) -> bool {
        self.scope.as_deref() != Some("landing_page")
            && matches!(
                self.product_kind.as_deref(),
                Some("consumer_app" | "business_app" | "developer_tool" | "dashboard")
            )
    }

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
    /// True while this prompt waits for its own turn. Running engines ignore it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_queued: bool,
    /// True when validated answers requested resumption of this waiting turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_resume: bool,
    /// When the turn was recorded, RFC 3339. Set on append.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The artifacts an assistant turn wrote, edited, or finished. The
    /// studio reverts the turn by restoring each one's snapshot from
    /// before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    /// Exact pre-edit snapshots, independent of when a queued prompt arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_stamps: Option<HashMap<String, String>>,
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
            is_queued: false,
            is_resume: false,
            snapshot_stamps: None,
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
            is_queued: false,
            is_resume: false,
            snapshot_stamps: None,
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
            is_queued: false,
            is_resume: false,
            snapshot_stamps: None,
        }
    }
}

fn run_sort_key(id: &str) -> (u64, u64) {
    id.split_once('-')
        .and_then(|(seconds, number)| Some((seconds.parse().ok()?, number.parse().ok()?)))
        .unwrap_or_default()
}
