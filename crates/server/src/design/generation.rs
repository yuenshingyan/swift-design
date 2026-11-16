//! Demo candidate generation, edits, continuation, review, and live previews.

use std::sync::Arc;

use design_model::Design;

use crate::candidates::{candidate_id, next_candidate_number};
use crate::design::core::is_pending_screen;
use crate::design::patch::{apply_patch, parse_patch};
use crate::design::polish::{DemoAudit, apply_tweak, polish_prompt};
use crate::edit_focus::{
    EditFix, EditInput, EditOrder, MergeInput, findings_for, fix_instruction, focus_note,
    fresh_note, merge_sources, touched_indexes,
};
use crate::edit_report::saved_edit_report;
use crate::model_client::LogSink;
use crate::parallel_edit::EditExecution;
use crate::polish::{PolishStop, polish_round_limit};
use crate::screenshots::{POLISH_IMAGE_LIMIT, find_chrome, screenshot_screen, supports_vision};
use crate::sessions::ChatMessage;
use crate::sessions::session_id_of_artifact;

use crate::generation::artifacts::*;
use crate::generation::*;
use prompts::*;
use streaming::*;

/// The screens each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type ChunkBoard = Arc<std::sync::Mutex<Vec<Vec<design_model::Screen>>>>;

mod continue_run;
mod edit_requirements;
pub(crate) mod live_preview;
pub(crate) mod prompts;
mod request;
pub(crate) mod streaming;
mod work;

#[cfg(test)]
mod tweaks_tests;
