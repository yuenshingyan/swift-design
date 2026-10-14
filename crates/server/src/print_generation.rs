//! The print half of the built-in generation engine.
//!
//! A print session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for prints: the print prompts, the print
//! patch, the print store, and sheet-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Orientation, Print, PrintSize, Sheet};

use crate::candidates::{candidate_id, next_candidate_number};
use crate::concepts::{Concept, concept_note};
use crate::edit_focus::{
    EditFix, EditInput, EditOrder, MergeInput, conversation_block, findings_for, findings_note,
    fix_instruction, focus_note, fresh_note, merge_note, merge_sources, referenced_indexes,
    touched_indexes,
};
use crate::edit_report::saved_edit_report;
use crate::events::ChangeNotifier;
use crate::generation::{
    ArtifactRequest, Attachments, CONTINUE_DRAFT_SHARE, ContinueChunk, DRAFT_SHARE,
    GenerationContext, GenerationEngine, GenerationOutcome, GenerationStop, GenerationTask,
    ShareSink, candidate_template, complete_array_items, continue_chunks, failure_message,
    stop_to_string, template_note, trailing_continue_ids, user_content_with_images, writing_effort,
};
use crate::instructions::PRINT_RULES;
use crate::model_client::LogSink;
use crate::parallel_edit::EditExecution;
use crate::polish::{PolishStop, can_audit, polish_round_limit};
use crate::print_patch::{PATCH_FORMAT, apply_patch, parse_patch};
use crate::print_polish::{dom_findings, polish_prompt};
use crate::prints::{PENDING_SHEET_CLASS, PrintStore, is_pending_sheet};
use crate::render::escape_html;
use crate::request::{SessionRequest, request_input};
use crate::screenshots::{POLISH_IMAGE_LIMIT, find_chrome, screenshot_sheet, supports_vision};
use crate::templates::Template;

/// The sheets each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type PrintChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Sheet>>>>;

/// What one print candidate call needs.
struct PrintCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` sheets plus the outline.
    preview_sheets: Option<usize>,
    /// The id the candidate is saved under.
    print_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

mod live;
mod pipeline;
mod prompts;

use live::*;
use prompts::*;

#[cfg(test)]
mod tests;
