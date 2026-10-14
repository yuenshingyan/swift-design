use super::*;

/// True when a run should be active and none is, so the studio offers
/// `Start the agent`: reading the request, drafting after the answers,
/// or generating. Not while a question set waits for the user.
pub(crate) fn is_start_offered(state: WorkflowState, is_running: bool, has_open_set: bool) -> bool {
    if is_running {
        return false;
    }
    match state {
        WorkflowState::Intake | WorkflowState::Generating => true,
        WorkflowState::Clarifying => !has_open_set,
        _ => false,
    }
}

/// Keeps chat open in every state. Busy runs queue prompts; halted runs resume on send.
pub(crate) fn is_chat_open(_state: WorkflowState, _is_running: bool) -> bool {
    true
}

/// The answers the user gave to question set `number`, as Q and A text.
pub(crate) fn set_answers(
    set: &design_model::BriefQuestionSet,
    records: &[api::AnswerRecord],
    number: u32,
) -> Vec<design_model::AnsweredQuestion> {
    let matching: Vec<api::AnswerRecord> = records
        .iter()
        .filter(|record| record.question_set == number)
        .cloned()
        .collect();
    answered_entries(std::slice::from_ref(set), &matching)
}

/// The chat box placeholder for a state.
/// One row of the `@` menu: the candidates it pins, with its key and
/// label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateMention {
    /// `all`, or the candidate number.
    pub(crate) key: String,
    /// `All candidates`, or the card name.
    pub(crate) label: String,
    /// The ids the row pins.
    pub(crate) ids: Vec<String>,
}

/// The rows a query matches: `All candidates` first when there is more
/// than one, then each candidate whose number starts with the query.
/// Every row for an empty query. At most eight.
pub(crate) fn candidate_mentions(ids: &[String], query: &str) -> Vec<CandidateMention> {
    let query = query.trim().to_lowercase();
    let mut rows = Vec::new();
    if ids.len() > 1 && (query.is_empty() || "all".starts_with(&query)) {
        rows.push(CandidateMention {
            key: "all".to_owned(),
            label: "All candidates".to_owned(),
            ids: ids.to_vec(),
        });
    }
    for id in ids {
        let label = candidate_label(id);
        let number = label.rsplit(' ').next().unwrap_or_default().to_owned();
        if query.is_empty() || number.starts_with(&query) {
            rows.push(CandidateMention {
                key: number,
                label,
                ids: vec![id.clone()],
            });
        }
    }
    rows.truncate(8);
    rows
}

/// Adds `ids` to the pins, each once, in the order they came.
pub(crate) fn pin_candidates(pinned: &mut Vec<String>, ids: &[String]) {
    for id in ids {
        if !pinned.contains(id) {
            pinned.push(id.clone());
        }
    }
}

/// The reference the pinned candidates put in front of a message, like
/// `[candidate 2] [candidate 3]`. `None` with no pins.
pub(crate) fn pinned_reference(pinned: &[String]) -> Option<String> {
    if pinned.is_empty() {
        return None;
    }
    let parts: Vec<String> = pinned
        .iter()
        .map(|id| format!("[{}]", candidate_label(id).to_lowercase()))
        .collect();
    Some(parts.join(" "))
}

pub(crate) fn chat_placeholder(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Intake => "Reply, or add detail…",
        WorkflowState::Clarifying => "Answer the questions above, or reply here…",
        WorkflowState::Generating => "Add a prompt to the queue…",
        WorkflowState::Reviewing => "Ask for a change, or for new candidates… @ pins a candidate",
        WorkflowState::Stopped => "The run stopped. Send a message to carry on…",
        WorkflowState::Error => "The run failed. Send a message to try again…",
    }
}
