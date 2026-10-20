use std::collections::HashMap;

use design_model::WorkflowState;

use crate::api::AgentRun;

use super::{
    candidate_mentions, chat_placeholder, generation_step, is_canvas_shown, is_chat_open,
    is_start_offered, pin_candidates, pinned_reference, progress_label,
};

#[test]
fn run_settings_only_appear_inside_the_open_question_card() {
    let source = include_str!("workspace.rs");
    assert_eq!(source.matches("RunSettings {").count(), 1);
    let question_card = source
        .split("if let Some((_number, set)) = open_set_card {")
        .nth(1)
        .expect("the workbench renders an open question card")
        .split("if !is_asking && is_canvas_shown")
        .next()
        .expect("the question card precedes the canvas");
    assert!(question_card.contains("RunSettings {"));
}

fn ids() -> Vec<String> {
    vec![
        "talk-candidate-1".to_owned(),
        "talk-candidate-2".to_owned(),
        "talk-candidate-3".to_owned(),
    ]
}

#[test]
fn an_empty_query_lists_all_candidates_then_each_one() {
    let rows = candidate_mentions(&ids(), "");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].key, "all");
    assert_eq!(rows[0].ids, ids());
    assert_eq!(rows[2].label, "Candidate 2");
    assert_eq!(rows[2].ids, vec!["talk-candidate-2".to_owned()]);
}

#[test]
fn a_query_matches_the_number_or_the_word_all() {
    let rows = candidate_mentions(&ids(), "3");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "3");
    let rows = candidate_mentions(&ids(), "al");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, "all");
    let one = vec!["talk-candidate-1".to_owned()];
    assert_eq!(candidate_mentions(&one, "").len(), 1);
}

#[test]
fn pins_hold_each_candidate_once_and_name_them_in_the_reference() {
    let mut pinned = vec!["talk-candidate-2".to_owned()];
    pin_candidates(&mut pinned, &ids());
    assert_eq!(pinned.len(), 3);
    assert_eq!(pinned[0], "talk-candidate-2");
    assert_eq!(
        pinned_reference(&pinned).as_deref(),
        Some("[candidate 2] [candidate 1] [candidate 3]")
    );
    assert_eq!(pinned_reference(&[]), None);
}

#[test]
fn the_chat_stays_open_while_a_run_works_and_queues_prompts() {
    assert!(is_chat_open(WorkflowState::Intake, false));
    assert!(is_chat_open(WorkflowState::Reviewing, false));
    assert!(is_chat_open(WorkflowState::Error, false));
    // A stopped session takes a message: it resumes on the way in.
    assert!(is_chat_open(WorkflowState::Stopped, false));
    assert!(is_chat_open(WorkflowState::Generating, false));
    assert!(is_chat_open(WorkflowState::Generating, true));
    assert!(is_chat_open(WorkflowState::Reviewing, true));
}

#[test]
fn start_is_offered_only_when_a_run_should_be_active_and_none_is() {
    assert!(is_start_offered(WorkflowState::Intake, false, false));
    assert!(is_start_offered(WorkflowState::Generating, false, false));
    assert!(is_start_offered(WorkflowState::Clarifying, false, false));
    assert!(!is_start_offered(WorkflowState::Clarifying, false, true));
    assert!(!is_start_offered(WorkflowState::Clarifying, true, false));
    assert!(!is_start_offered(WorkflowState::Error, false, false));
    assert!(!is_start_offered(WorkflowState::Stopped, false, false));
    assert!(!is_start_offered(WorkflowState::Reviewing, false, false));
}

fn run(log: &str) -> AgentRun {
    AgentRun {
        is_running: true,
        exit_code: None,
        log_tail: log.to_owned(),
        active_agent: None,
        session_id: None,
        mode: None,
        context_tokens: 0,
        total_tokens: 0,
        context_window: 0,
        progress: Some(40),
        designs: HashMap::new(),
    }
}

#[test]
fn progress_labels_follow_the_state() {
    assert_eq!(
        progress_label(WorkflowState::Intake, None),
        "Reading your request…"
    );
    assert_eq!(
        progress_label(WorkflowState::Clarifying, Some(&run(""))),
        "Preparing questions…"
    );
    assert_eq!(
        progress_label(WorkflowState::Clarifying, None),
        "Waiting for your answers"
    );
    assert_eq!(
        progress_label(WorkflowState::Reviewing, None),
        "Ready for review"
    );
}

#[test]
fn a_stop_and_a_failure_read_differently() {
    // The whole point of the stopped state: it must not read as a
    // fault anywhere the user looks.
    let stopped = progress_label(WorkflowState::Stopped, None);
    let failed = progress_label(WorkflowState::Error, None);
    assert_ne!(stopped, failed);
    assert!(!stopped.contains("fail"));
    assert!(failed.contains("failed"));
    assert!(chat_placeholder(WorkflowState::Generating).contains("queue"));
    assert!(chat_placeholder(WorkflowState::Stopped).contains("carry on"));
    assert!(chat_placeholder(WorkflowState::Error).contains("failed"));
}

#[test]
fn generation_steps_come_from_the_log_tail() {
    assert_eq!(
        generation_step("validating screen 3"),
        Some("Validating the output")
    );
    assert_eq!(
        generation_step("candidate 1: requesting"),
        Some("Writing screens")
    );
    assert_eq!(generation_step("writing slide 4"), Some("Writing slides"));
    assert_eq!(generation_step("writing page 4"), Some("Writing pages"));
    assert_eq!(generation_step("writing frame 4"), Some("Writing frames"));
    assert_eq!(generation_step("something else"), None);
}

#[test]
fn generation_falls_back_to_the_working_label() {
    assert_eq!(
        progress_label(WorkflowState::Generating, Some(&run("thinking hard"))),
        "Working… 40%"
    );
}

#[test]
fn the_canvas_shows_while_generating_or_when_designs_exist() {
    assert!(is_canvas_shown(WorkflowState::Generating, false));
    assert!(is_canvas_shown(WorkflowState::Reviewing, false));
    assert!(is_canvas_shown(WorkflowState::Clarifying, true));
    assert!(!is_canvas_shown(WorkflowState::Clarifying, false));
}
