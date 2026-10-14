//! The session workspace: the conversation on the left, the brief and
//! the candidate canvas on the right. A long-poll on `GET /events`
//! keeps both live.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;

use crate::api;
use crate::canvas::{
    CandidateCanvas, candidate_label, cards_from_artworks, cards_from_campaigns, cards_from_decks,
    cards_from_designs, cards_from_documents, cards_from_mailings, cards_from_prints,
    cards_from_socials, queued_finishes,
};
use crate::chat::{mention_at, recall_prompt, remove_mention, use_follow_thread, watch_caret};
use crate::chat_controls::{ModelChip, SendButton, with_effort};
use crate::prompt_history::{PromptHistory, prompt_entries};
use crate::question_card::{
    DraftAnswer, QaRow, QuestionCardState, QuestionSetCard, answered_entries, question_card_state,
    set_key,
};
use crate::revert::{revert_artifact, turn_start};
use crate::run_settings::{
    ArtworkQuestions, CampaignQuestions, DeckQuestions, DocumentQuestions, MailingQuestions,
    PrintQuestions, RunSettings, SharedQuestions, SocialQuestions, app_answers,
};
use crate::settings::{SettingsPanel, pause_briefly};
use crate::status::{RunStatusCard, working_label};
use design_model::{
    AdSize, ArtifactKind, CoverSize, DECK_VIEWPORT, EmailFormat, Format, Orientation, Paper,
    PrintSize, QuestionAnswer, Viewport, WorkflowState,
};

/// The status line for the session's current state.
pub(crate) fn progress_label(state: WorkflowState, run: Option<&api::AgentRun>) -> String {
    let running = run.is_some_and(|run| run.is_running);
    match state {
        WorkflowState::Intake => "Reading your request…".to_owned(),
        WorkflowState::Clarifying if running => "Preparing questions…".to_owned(),
        WorkflowState::Clarifying => "Waiting for your answers".to_owned(),
        WorkflowState::Generating => run
            .and_then(|run| generation_step(&run.log_tail))
            .map(str::to_owned)
            .or_else(|| run.map(working_label))
            .unwrap_or_else(|| "Generating…".to_owned()),
        WorkflowState::Reviewing => "Ready for review".to_owned(),
        WorkflowState::Stopped => "Stopped before it finished".to_owned(),
        WorkflowState::Error => "Stopped: the run failed".to_owned(),
    }
}

/// The generation step named by the last log line, when it matches one.
pub(crate) fn generation_step(log_tail: &str) -> Option<&'static str> {
    let last = log_tail
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())?;
    let lower = last.to_ascii_lowercase();
    if lower.contains("brief") {
        Some("Reading the brief")
    } else if lower.contains("validat") {
        Some("Validating the output")
    } else if lower.contains("polish") {
        Some("Polishing the design")
    } else if lower.contains("slide") {
        Some("Writing slides")
    } else if lower.contains("page") {
        Some("Writing pages")
    } else if lower.contains("frame") {
        Some("Writing frames")
    } else if lower.contains("sheet") {
        Some("Writing sheets")
    } else if lower.contains("email") {
        Some("Writing emails")
    } else if lower.contains(" ads") || lower.contains(" ad ") {
        Some("Writing ads")
    } else if lower.contains("candidate") || lower.contains("screen") || lower.contains("writ") {
        Some("Writing screens")
    } else {
        None
    }
}

/// True when the canvas should show: while generating or reviewing, or
/// whenever any design or deck exists.
pub(crate) fn is_canvas_shown(state: WorkflowState, has_designs: bool) -> bool {
    matches!(state, WorkflowState::Generating | WorkflowState::Reviewing) || has_designs
}

mod helpers;
mod workspace;

pub(crate) use helpers::{CandidateMention, candidate_mentions, pin_candidates};
pub(crate) use workspace::SessionWorkspace;

#[cfg(test)]
use helpers::{chat_placeholder, is_chat_open, is_start_offered, pinned_reference};

#[cfg(test)]
mod tests;
