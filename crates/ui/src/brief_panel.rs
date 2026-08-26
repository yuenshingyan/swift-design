//! The brief panel: the versioned design brief, its three distinct
//! groups (confirmed facts, assumptions, open questions), the editable
//! fields, the revision list, and the approve/generate actions.

use design_model::{BriefRevision, BriefSection, DesignBrief, WorkflowState};
use dioxus::prelude::*;

use crate::api;

/// Which brief actions are enabled in the current state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BriefActions {
    /// Approve the brief and generate.
    pub can_approve: bool,
    /// Generate with the recorded assumptions.
    pub can_generate_with_assumptions: bool,
    /// Edit the brief fields.
    pub can_edit: bool,
    /// Critique the design.
    pub can_critique: bool,
    /// Retry after an error.
    pub can_retry: bool,
    /// Send a chat turn.
    pub can_chat: bool,
}

/// The actions for `state`. `can_proceed` is the question set's flag.
pub(crate) fn brief_actions_for(state: WorkflowState, can_proceed: bool) -> BriefActions {
    use WorkflowState::*;
    match state {
        Intake => BriefActions::default(),
        Clarifying => BriefActions {
            can_generate_with_assumptions: can_proceed,
            can_chat: true,
            ..BriefActions::default()
        },
        BriefReady | AwaitingApproval => BriefActions {
            can_approve: true,
            can_generate_with_assumptions: true,
            can_edit: true,
            can_chat: true,
            ..BriefActions::default()
        },
        Generating => BriefActions {
            can_edit: true,
            ..BriefActions::default()
        },
        Reviewing => BriefActions {
            can_edit: true,
            can_critique: true,
            can_chat: true,
            ..BriefActions::default()
        },
        Error => BriefActions {
            can_retry: true,
            can_chat: true,
            ..BriefActions::default()
        },
    }
}

/// The three brief groups, cleaned of blanks and duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BriefGroups {
    /// Facts the user confirmed.
    pub facts: Vec<String>,
    /// Choices the app or the agent made.
    pub assumptions: Vec<String>,
    /// Questions still open.
    pub open: Vec<String>,
}

/// The three groups of `brief`, trimmed, de-duplicated, order kept.
pub(crate) fn facts_assumptions_open(brief: &DesignBrief) -> BriefGroups {
    BriefGroups {
        facts: clean(&brief.confirmed_facts),
        assumptions: clean(&brief.assumptions),
        open: clean(&brief.open_questions),
    }
}

/// Trims, drops blanks, and de-duplicates a list, keeping order.
fn clean(items: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if !trimmed.is_empty() && !seen.iter().any(|kept| kept == trimmed) {
            seen.push(trimmed.to_owned());
        }
    }
    seen
}

/// Splits textarea text into a list, one item per non-blank line. Used
/// by the brief editor.
#[allow(dead_code)]
pub(crate) fn lines_to_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Joins a list back into textarea text, one item per line.
#[allow(dead_code)]
pub(crate) fn list_to_lines(items: &[String]) -> String {
    items.join("\n")
}

/// The label for a revision row: `r3 · user edit · 14:02`.
pub(crate) fn revision_label(entry: &BriefRevision) -> String {
    let time = entry.at.get(11..16).unwrap_or("");
    format!(
        "r{} · {} · {time}",
        entry.revision,
        entry.source.as_str().replace('_', " ")
    )
}

/// The readable name of a workflow state.
pub(crate) fn state_label(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Intake => "Intake",
        WorkflowState::Clarifying => "Clarifying",
        WorkflowState::BriefReady => "Brief ready",
        WorkflowState::AwaitingApproval => "Awaiting approval",
        WorkflowState::Generating => "Generating",
        WorkflowState::Reviewing => "Reviewing",
        WorkflowState::Error => "Error",
    }
}

/// The badge class suffix for a state.
fn state_class(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Generating => "generating",
        WorkflowState::Reviewing => "reviewing",
        WorkflowState::Error => "error",
        _ => "",
    }
}

/// The brief panel for one session.
#[component]
pub(crate) fn BriefPanel(
    session_id: String,
    state: WorkflowState,
    brief: Option<DesignBrief>,
    revisions: Vec<BriefRevision>,
    actions: BriefActions,
    on_error: EventHandler<String>,
) -> Element {
    let Some(brief) = brief else {
        return rsx! {
            section { class: "brief-panel",
                div { class: "brief-head",
                    span { class: "kicker", "Brief" }
                    span { class: "state-badge {state_class(state)}", "{state_label(state)}" }
                }
                p { class: "lede", "The brief appears here once the questions are answered." }
            }
        };
    };
    let groups = facts_assumptions_open(&brief);
    let revision = brief.revision;
    let assumption_count = groups.assumptions.len();
    let approve = {
        let id = session_id.clone();
        move |_| {
            let id = id.clone();
            let on_error = on_error;
            spawn(async move {
                if let Err(error) = api::approve_brief(&id).await {
                    on_error.call(error);
                }
            });
        }
    };
    let generate = {
        let id = session_id.clone();
        move |_| {
            let id = id.clone();
            let on_error = on_error;
            spawn(async move {
                if let Err(error) = api::generate_with_assumptions(&id).await {
                    on_error.call(error);
                }
            });
        }
    };
    rsx! {
        section { class: "brief-panel",
            div { class: "brief-head",
                span { class: "kicker", "Brief" }
                span { class: "state-badge {state_class(state)}", "{state_label(state)}" }
                span { class: "rev", "rev {revision}" }
            }
            BriefGroupsView { groups: groups.clone() }
            BriefFields { brief: brief.clone() }
            if !revisions.is_empty() {
                div { class: "revision-list",
                    span { class: "kicker", "Revisions" }
                    for entry in revisions.iter() {
                        div { key: "{entry.revision}",
                            class: if entry.revision == revision { "history-row current" } else { "history-row" },
                            "{revision_label(entry)}"
                        }
                    }
                }
            }
            div { class: "brief-actions",
                button {
                    class: "primary",
                    disabled: !actions.can_approve,
                    onclick: approve,
                    "Approve brief and generate"
                }
                button {
                    class: "secondary",
                    disabled: !actions.can_generate_with_assumptions,
                    onclick: generate,
                    "Generate with assumptions"
                }
                if actions.can_generate_with_assumptions && assumption_count > 0 {
                    span { class: "brief-note", "{assumption_count} assumptions will be used" }
                }
            }
        }
    }
}

/// The three distinct brief groups.
#[component]
fn BriefGroupsView(groups: BriefGroups) -> Element {
    rsx! {
        div { class: "brief-groups",
            BriefGroup { class: "facts", title: "Confirmed by you", items: groups.facts }
            BriefGroup { class: "assumptions", title: "Assumed", items: groups.assumptions }
            BriefGroup { class: "open", title: "Still open", items: groups.open }
        }
    }
}

/// One brief group block.
#[component]
fn BriefGroup(class: String, title: String, items: Vec<String>) -> Element {
    rsx! {
        div { class: "brief-group {class}",
            span { class: "brief-group-title", "{title}" }
            if items.is_empty() {
                p { class: "brief-list-empty", "none" }
            } else {
                ul { class: "brief-list",
                    for item in items {
                        li { "{item}" }
                    }
                }
            }
        }
    }
}

/// The read-only brief fields. Editing is done from the chat for now.
#[component]
fn BriefFields(brief: DesignBrief) -> Element {
    rsx! {
        div { class: "brief-fields",
            BriefField { label: "Target artifact", value: brief.target_artifact }
            BriefField { label: "Target platform", value: brief.target_platform }
            BriefField { label: "Audience", value: brief.audience }
            BriefField { label: "Primary job", value: brief.primary_job }
            BriefField { label: "Success criterion", value: brief.success_criterion }
            BriefField { label: "User problem", value: brief.user_problem }
            BriefField { label: "Visual direction", value: brief.visual_direction }
            if !brief.required_sections.is_empty() {
                div { class: "brief-field",
                    span { class: "brief-field-label", "Required sections" }
                    for section in brief.required_sections.iter() {
                        SectionRow { section: section.clone() }
                    }
                }
            }
        }
    }
}

/// One labelled brief field. Blank values are hidden.
#[component]
fn BriefField(label: String, value: String) -> Element {
    if value.trim().is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "brief-field",
            span { class: "brief-field-label", "{label}" }
            span { class: "brief-field-value", "{value}" }
        }
    }
}

/// One required-section row.
#[component]
fn SectionRow(section: BriefSection) -> Element {
    rsx! {
        div { class: "section-row",
            span { class: "section-name", "{section.name}" }
            span { class: "section-content", "{section.content}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use design_model::RevisionSource;

    #[test]
    fn approve_is_enabled_only_when_the_brief_awaits_approval() {
        assert!(brief_actions_for(WorkflowState::AwaitingApproval, false).can_approve);
        assert!(brief_actions_for(WorkflowState::BriefReady, false).can_approve);
        assert!(!brief_actions_for(WorkflowState::Clarifying, false).can_approve);
        assert!(!brief_actions_for(WorkflowState::Reviewing, false).can_approve);
    }

    #[test]
    fn generate_with_assumptions_follows_the_set_flag_while_clarifying() {
        assert!(brief_actions_for(WorkflowState::Clarifying, true).can_generate_with_assumptions);
        assert!(!brief_actions_for(WorkflowState::Clarifying, false).can_generate_with_assumptions);
        assert!(brief_actions_for(WorkflowState::BriefReady, false).can_generate_with_assumptions);
    }

    #[test]
    fn critique_is_enabled_only_while_reviewing() {
        assert!(brief_actions_for(WorkflowState::Reviewing, false).can_critique);
        assert!(!brief_actions_for(WorkflowState::Generating, false).can_critique);
    }

    #[test]
    fn retry_is_enabled_only_in_error() {
        assert!(brief_actions_for(WorkflowState::Error, false).can_retry);
        assert!(!brief_actions_for(WorkflowState::Reviewing, false).can_retry);
    }

    #[test]
    fn editing_stays_enabled_during_generation() {
        assert!(brief_actions_for(WorkflowState::Generating, false).can_edit);
        assert!(!brief_actions_for(WorkflowState::Intake, false).can_edit);
    }

    #[test]
    fn chat_is_off_during_intake_and_generation() {
        assert!(!brief_actions_for(WorkflowState::Intake, false).can_chat);
        assert!(!brief_actions_for(WorkflowState::Generating, false).can_chat);
        assert!(brief_actions_for(WorkflowState::Reviewing, false).can_chat);
    }

    #[test]
    fn groups_keep_facts_assumptions_and_open_questions_apart() {
        let brief = DesignBrief {
            confirmed_facts: vec!["  Web  ".to_owned(), "Web".to_owned(), String::new()],
            assumptions: vec!["Investors".to_owned()],
            open_questions: vec!["Colors?".to_owned()],
            ..DesignBrief::default()
        };
        let groups = facts_assumptions_open(&brief);
        assert_eq!(groups.facts, vec!["Web"]);
        assert_eq!(groups.assumptions, vec!["Investors"]);
        assert_eq!(groups.open, vec!["Colors?"]);
    }

    #[test]
    fn lines_round_trip_to_lists() {
        assert_eq!(lines_to_list("a\n\n b \n"), vec!["a", "b"]);
        assert_eq!(list_to_lines(&["a".to_owned(), "b".to_owned()]), "a\nb");
    }

    #[test]
    fn revision_labels_name_the_number_and_source() {
        let entry = BriefRevision {
            revision: 3,
            source: RevisionSource::UserEdit,
            summary: "x".to_owned(),
            at: "2026-08-26T14:02:00Z".to_owned(),
        };
        assert_eq!(revision_label(&entry), "r3 · user edit · 14:02");
    }

    #[test]
    fn state_labels_are_readable() {
        assert_eq!(
            state_label(WorkflowState::AwaitingApproval),
            "Awaiting approval"
        );
        assert_eq!(state_label(WorkflowState::Error), "Error");
    }
}
