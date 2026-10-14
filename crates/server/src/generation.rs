//! The built-in generation engine.
//!
//! Talks to the model through `model_client`, with the user's own keys
//! and only when a run starts. The loop: read the brief, ask the model
//! for each candidate design, validate, feed every validation error
//! back for a fix round, and save the result. The studio watches it all
//! through `/events`.

use std::collections::HashMap;
use std::sync::Arc;

use design_model::{
    ArtifactKind, BriefQuestionSet, Design, QuestionSetError, validate_question_set,
};

use crate::artworks::ArtworkStore;
use crate::campaigns::CampaignStore;
use crate::candidates::{candidate_id, next_candidate_number};
use crate::concepts::{Concept, concept_input, concept_note, concept_prompt, parse_concepts};
use crate::decks::DeckStore;
use crate::designs::{DesignStore, PENDING_SCREEN_CLASS, is_pending_screen};
use crate::documents::DocumentStore;
use crate::edit_focus::{
    EditFix, EditInput, EditOrder, MergeInput, conversation_block, findings_for, findings_note,
    fix_instruction, focus_note, fresh_note, merge_note, merge_sources, referenced_indexes,
    touched_indexes,
};
use crate::edit_report::{EDIT_REPORT_PREFIX, saved_edit_report};
use crate::events::ChangeNotifier;
use crate::export::base64_encode;
use crate::history::Snapshot;
use crate::instructions::DEMO_RULES;
use crate::mailings::MailingStore;
use crate::model_client::{LogSink, ModelClient, ModelConfiguration, TextSink, UsageSink};
use crate::office::{is_office_type, office_text};
use crate::parallel_edit::EditExecution;
use crate::patch::{PATCH_FORMAT, apply_patch, parse_patch};
use crate::planner::{
    ANSWERED_QUESTION_LIMIT, app_question_set, conversation_note, parse_plan, planner_input,
    planner_prompt, recent_conversation,
};
use crate::polish::{
    DemoAudit, DemoDomRunner, PolishStop, apply_tweak, chrome_dom_runner, polish_prompt,
    polish_round_limit,
};
use crate::prints::PrintStore;
use crate::render::escape_html;
use crate::request::{SessionRequest, answered_questions_from_answers, request_input};
use crate::screenshots::{POLISH_IMAGE_LIMIT, find_chrome, screenshot_screen, supports_vision};
use crate::sessions::session_id_of_artifact;
use crate::sessions::{
    AnswerRecord, ChatMessage, RunMode, RunOptions, RunRecord, Session, SessionStore,
};
use crate::settings::SettingsStore;
use crate::socials::SocialStore;
use crate::templates::{Template, TemplateStore};
use crate::time::rfc3339_now;
use crate::uploads::UploadStore;

/// Fix rounds per candidate before giving up, by effort level.
pub(crate) fn fix_round_limit(effort: &str) -> usize {
    match effort {
        "low" => 2,
        "high" => 4,
        _ => 3,
    }
}

/// Screens a preview candidate writes before the rest are continued.
pub(crate) const PREVIEW_SCREEN_COUNT: usize = 3;

/// What a generation run did.
#[derive(Debug)]
pub enum GenerationOutcome {
    /// The run wrote these design ids.
    Wrote {
        /// The design ids written, in candidate order.
        design_ids: Vec<String>,
    },
    /// The run edited existing artifacts and compared their saved contents.
    Edited {
        /// The ids that were saved successfully, in candidate order.
        design_ids: Vec<String>,
        /// One app-generated before/after report per saved artifact.
        reports: Vec<String>,
    },
    /// The run needs the user to answer a blocking question set. The
    /// engine has written the set and moved the session to clarifying.
    NeedsClarification {
        /// The set number.
        question_set: u32,
    },
    /// The planner answered in the chat and wrote nothing.
    Replied,
}

impl GenerationOutcome {
    /// Keeps saved artifact ids and their comparison reports together.
    pub(crate) fn edited(edits: Vec<(String, String)>) -> Self {
        let (design_ids, reports) = edits.into_iter().unzip();
        Self::Edited {
            design_ids,
            reports,
        }
    }
}

/// What the model must do this run.
pub(crate) enum GenerationTask {
    /// Write the requested candidates.
    Candidates,
    /// Apply the user's latest request to each artifact named.
    Edit {
        /// The artifact ids to edit: the candidates the user pinned,
        /// the artifact open in the editor, or the chosen one.
        designs: Vec<String>,
        /// What to change, in the user's words.
        instruction: String,
        /// The windowed conversation note, or empty.
        conversation: String,
    },
    /// Continue the preview designs named here.
    Continue(Vec<String>),
    /// Combine parts of the named candidates into one new candidate.
    Merge {
        /// The candidates to take parts from, in the order pinned.
        sources: Vec<String>,
        /// Which parts to take from each, in the user's words.
        instruction: String,
    },
    /// Write the units the instruction names anew, in one artifact.
    Regenerate {
        /// The artifact whose units are rewritten.
        design: String,
        /// The request, with the `[screen N]` or `[slide N]` references.
        instruction: String,
        /// The windowed conversation note, or empty.
        conversation: String,
    },
}

/// The request, the answers, and the options one run works from.
#[derive(Clone)]
pub(crate) struct GenerationContext {
    pub(crate) request: SessionRequest,
    pub(crate) options: RunOptions,
    pub(crate) session_id: String,
}

impl GenerationContext {
    /// The effort level for this run.
    pub(crate) fn effort(&self) -> &str {
        &self.options.effort
    }

    /// The preview screen count, or `None` for complete candidates.
    pub(crate) fn preview_screens(&self) -> Option<usize> {
        self.options.preview.then_some(PREVIEW_SCREEN_COUNT)
    }
}

/// Why a generation run stopped without writing a design.
pub(crate) enum GenerationStop {
    /// The run failed with this message.
    Failed(String),
    /// The model asked for a blocking detail. The engine writes the set
    /// and returns the session to clarifying.
    NeedsClarification(BriefQuestionSet),
}

/// What the planner decided for this turn.
enum PlanStep {
    /// Write or edit: run this task.
    Task(GenerationTask),
    /// A question set was written; its number.
    Asked(u32),
    /// A chat reply only.
    Replied,
}

/// The artifacts the latest user turn is about: the candidates it
/// pinned, else the artifact it was sent from in the editor, else the
/// chosen one. Empty when it names none.
fn edit_targets(messages: &[ChatMessage], chosen: Option<&str>) -> Vec<String> {
    let latest = messages.iter().rev().find(|message| message.role == "user");
    if let Some(message) = latest {
        if !message.pinned.is_empty() {
            return message.pinned.clone();
        }
        if let Some(design) = &message.design {
            return vec![design.clone()];
        }
    }
    chosen.map(str::to_owned).into_iter().collect()
}

/// How many questions the user answered since the last turn that
/// wrote artifacts: the answers that belong to the current request.
/// Every answer counts while nothing was written yet.
pub(crate) fn answers_since_last_write(
    messages: &[ChatMessage],
    records: &[AnswerRecord],
) -> usize {
    let last_write = messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant" && !message.artifacts.is_empty())
        .and_then(|message| message.at.clone());
    records
        .iter()
        .filter(|record| {
            last_write
                .as_deref()
                .is_none_or(|written| record.at.as_str() > written)
        })
        .map(|record| record.answers.len())
        .sum()
}

/// The text of the latest user turn.
fn latest_user_text(messages: &[ChatMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.clone())
}

impl From<String> for GenerationStop {
    fn from(message: String) -> Self {
        GenerationStop::Failed(message)
    }
}

/// The built-in engine: the model client plus the stores it writes.
#[derive(Clone)]
pub struct GenerationEngine {
    pub(crate) model: ModelClient,
    pub(crate) designs: DesignStore,
    /// The deck store, for deck sessions. `None` refuses deck runs.
    pub(crate) decks: Option<DeckStore>,
    /// The document store, for document sessions. `None` refuses
    /// document runs.
    pub(crate) documents: Option<DocumentStore>,
    /// The social store, for social sessions. `None` refuses social
    /// runs.
    pub(crate) socials: Option<SocialStore>,
    /// The print store, for print sessions. `None` refuses print runs.
    pub(crate) prints: Option<PrintStore>,
    /// The mailing store, for mailing sessions. `None` refuses mailing
    /// runs.
    pub(crate) mailings: Option<MailingStore>,
    /// The campaign store, for campaign sessions. `None` refuses
    /// campaign runs.
    pub(crate) campaigns: Option<CampaignStore>,
    /// The artwork store, for artwork sessions. `None` refuses artwork
    /// runs.
    pub(crate) artworks: Option<ArtworkStore>,
    pub(crate) sessions: SessionStore,
    address: String,
    pub(crate) notifier: ChangeNotifier,
    progress_sink: Option<ProgressSink>,
    design_progress_sink: Option<DesignProgressSink>,
    templates: Option<TemplateStore>,
    uploads: Option<UploadStore>,
    demo_dom_runner: DemoDomRunner,
}

/// A sink for how far the current turn is, 0 to 100, shared with the
/// run status. A turn starts at 0.
pub type ProgressSink = Arc<dyn Fn(u8) + Send + Sync>;

/// A sink for how far one design the turn writes is, 0 to 100, by design
/// id: a candidate being written, a design being continued or edited.
pub type DesignProgressSink = Arc<dyn Fn(&str, u8) + Send + Sync>;

/// One item's share of a turn's progress, 0.0 to 1.0. Several items
/// (candidates, continued designs) each report their own share; the mean
/// goes to the `ProgressSink`.
pub(crate) type ShareSink = Arc<dyn Fn(f32) + Send + Sync>;

/// What every continue of one turn shares.
struct ContinueShared<'a> {
    /// The HTTP client.
    client: &'a reqwest::Client,
    /// The run's context.
    context: &'a GenerationContext,
    /// The user's source files, loaded once.
    attachments: Arc<Attachments>,
    /// The progress group the continues report into.
    group: ProgressGroup,
    /// The run log.
    log: &'a LogSink,
}

/// The continues in flight and the ids started so far.
#[derive(Default)]
struct ContinueBatch {
    /// The running continues.
    tasks: tokio::task::JoinSet<(String, Result<usize, String>)>,
    /// Every id started, in order.
    started: Vec<String>,
}

/// The progress of a set of artifacts that grows while the run works.
pub(crate) struct ProgressGroup {
    /// One fraction per share, in start order.
    shares: Arc<std::sync::Mutex<Vec<f32>>>,
    /// The run's progress sink.
    sink: Option<ProgressSink>,
    /// The per-artifact progress sink.
    design_sink: Option<DesignProgressSink>,
    /// The percentage the group starts from.
    base: u8,
    /// The percentage span the group fills.
    span: u8,
}

impl ProgressGroup {
    /// Adds an artifact and returns its share.
    pub(crate) fn share(&self, design_id: &str) -> ShareSink {
        let index = match self.shares.lock() {
            Ok(mut shares) => {
                shares.push(0.0);
                shares.len() - 1
            }
            Err(_) => 0,
        };
        let shares = Arc::clone(&self.shares);
        let sink = self.sink.clone();
        let design_sink = self.design_sink.clone();
        let design_id = design_id.to_owned();
        let (base, span) = (self.base, self.span);
        Arc::new(move |fraction: f32| {
            let fraction = fraction.clamp(0.0, 1.0);
            if let Some(design_sink) = &design_sink {
                design_sink(&design_id, (fraction * 100.0) as u8);
            }
            let Ok(mut shares) = shares.lock() else {
                return;
            };
            if let Some(slot) = shares.get_mut(index) {
                *slot = fraction;
            }
            let mean = shares.iter().sum::<f32>() / shares.len().max(1) as f32;
            if let Some(sink) = &sink {
                sink(base.saturating_add((f32::from(span) * mean) as u8));
            }
        })
    }
}

/// The screens each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type ChunkBoard = Arc<std::sync::Mutex<Vec<Vec<design_model::Screen>>>>;

/// The share of a design request that the first valid draft completes;
/// the polish rounds fill the rest.
pub(crate) const DRAFT_SHARE: f32 = 0.6;

/// Screens the engine asks for in one continuation request. Small
/// chunks keep each reply short, and the design grows on the canvas
/// after every chunk.
pub(crate) const CONTINUE_CHUNK_SCREENS: usize = 3;

/// The share of a continuation that writing the screens completes; the
/// polish rounds fill the rest.
pub(crate) const CONTINUE_DRAFT_SHARE: f32 = 0.85;

/// The artifact ids of the continue requests at the end of `messages`,
/// newest last, without repeats.
///
/// The walk stops at the first user message that is not a continue: a
/// request from an earlier turn is that turn's business. Pressing Finish
/// on three candidates in a row therefore continues all three.
pub(crate) fn trailing_continue_ids(messages: &[ChatMessage]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for message in messages.iter().rev() {
        if message.role != "user" {
            continue;
        }
        if !message.is_continue {
            break;
        }
        if let Some(id) = &message.design
            && !ids.contains(id)
        {
            ids.push(id.clone());
        }
    }
    ids.reverse();
    ids
}

/// The artifact and the instruction of a regenerate request, when the
/// latest user turn is one.
fn trailing_regenerate(messages: &[ChatMessage]) -> Option<(String, String)> {
    let latest = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")?;
    if !latest.is_regenerate {
        return None;
    }
    let design = latest.design.clone()?;
    Some((design, latest.content.clone()))
}

mod artifacts;
mod attachments;
mod engine;
mod streaming;

pub(crate) use artifacts::{
    ArtifactRequest, Validated, candidate_template, failure_message, stop_to_string, template_note,
};
pub(crate) use attachments::{ATTACHMENT_TEXT_LIMIT_BYTES, Attachments, user_content_with_images};
pub(crate) use streaming::{ContinueChunk, complete_array_items, continue_chunks, writing_effort};

#[cfg(test)]
mod tests;
