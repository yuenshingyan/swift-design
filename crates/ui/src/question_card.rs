//! Question cards: the structured questions the agent asks, and the
//! draft answers the user builds before sending them.

use std::collections::HashMap;

use design_model::{BriefQuestion, BriefQuestionSet, QuestionAnswer, QuestionKind};
use dioxus::prelude::*;

/// The answer the user is building for one question, before it is sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DraftAnswer {
    /// The chosen option values, or the text for a text question.
    pub values: Vec<String>,
    /// The text of the `Other` field.
    pub other_text: String,
    /// True when the `Other` field is open.
    pub is_other_open: bool,
    /// True when the user chose to skip.
    pub is_skipped: bool,
}

/// Whether a question card is still open, already answered, or was
/// replaced without an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuestionCardState {
    /// The open set: the user can answer it now.
    Active,
    /// The set was answered.
    Answered,
    /// The set was superseded without an answer.
    Stale,
}

/// Chooses `value` on a select question. Single-select replaces; multi
/// toggles. Both clear the skip.
pub(crate) fn select_value(draft: &mut DraftAnswer, kind: QuestionKind, value: &str) {
    draft.is_skipped = false;
    if kind == QuestionKind::SingleSelect {
        draft.values = vec![value.to_owned()];
        return;
    }
    if let Some(position) = draft.values.iter().position(|existing| existing == value) {
        draft.values.remove(position);
    } else {
        draft.values.push(value.to_owned());
    }
}

/// Opens or closes the `Other` field. Opening a single-select clears
/// its values. Either way it clears the skip.
pub(crate) fn toggle_other(draft: &mut DraftAnswer, kind: QuestionKind) {
    draft.is_skipped = false;
    draft.is_other_open = !draft.is_other_open;
    if draft.is_other_open && kind == QuestionKind::SingleSelect {
        draft.values.clear();
    }
}

/// Marks the answer skipped, clearing the values and the other text.
pub(crate) fn skip_answer(draft: &mut DraftAnswer) {
    draft.is_skipped = true;
    draft.values.clear();
    draft.other_text.clear();
    draft.is_other_open = false;
}

/// True when a question has enough to send: a skip always counts; an
/// optional question with nothing counts; a select needs a value or a
/// non-blank other; a text question needs non-blank text.
pub(crate) fn is_answer_complete(question: &BriefQuestion, draft: &DraftAnswer) -> bool {
    if draft.is_skipped {
        return true;
    }
    let has_other = draft.is_other_open && !draft.other_text.trim().is_empty();
    let has_content = if question.kind.is_select() {
        !draft.values.is_empty() || has_other
    } else {
        !draft.other_text.trim().is_empty()
    };
    if !question.required {
        // An optional question with an empty Other field open is fine.
        return !draft.is_other_open || has_other || question.kind.is_select();
    }
    has_content
}

/// True when every question in the set is complete.
pub(crate) fn answers_are_complete(
    set: &BriefQuestionSet,
    drafts: &HashMap<String, DraftAnswer>,
) -> bool {
    set.questions.iter().take(3).all(|question| {
        drafts
            .get(&question.id)
            .map(|draft| is_answer_complete(question, draft))
            .unwrap_or(!question.required)
    })
}

/// Builds the answer for one question from its draft. Text questions
/// keep their text in `values[0]`.
pub(crate) fn draft_to_answer(question: &BriefQuestion, draft: &DraftAnswer) -> QuestionAnswer {
    if draft.is_skipped {
        return QuestionAnswer {
            question_id: question.id.clone(),
            skipped: true,
            ..QuestionAnswer::default()
        };
    }
    let other_text = if draft.is_other_open && !draft.other_text.trim().is_empty() {
        Some(draft.other_text.trim().to_owned())
    } else {
        None
    };
    if question.kind.is_select() {
        QuestionAnswer {
            question_id: question.id.clone(),
            values: draft.values.clone(),
            other_text,
            skipped: false,
        }
    } else {
        let text = draft.other_text.trim();
        QuestionAnswer {
            question_id: question.id.clone(),
            values: if text.is_empty() {
                Vec::new()
            } else {
                vec![text.to_owned()]
            },
            other_text: None,
            skipped: false,
        }
    }
}

/// The one-line summary of an answer, for a collapsed card.
pub(crate) fn answer_summary(question: &BriefQuestion, answer: &QuestionAnswer) -> String {
    if answer.skipped {
        return "Use your best judgment".to_owned();
    }
    let mut parts: Vec<String> = answer
        .values
        .iter()
        .map(|value| {
            question
                .options
                .iter()
                .find(|option| &option.value == value)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| value.clone())
        })
        .collect();
    if let Some(other) = &answer.other_text {
        parts.push(format!("+ {other}"));
    }
    if parts.is_empty() {
        "—".to_owned()
    } else {
        parts.join(", ")
    }
}

/// The state of the set numbered `set_number` against the answers and
/// the open set number.
pub(crate) fn question_card_state(
    set_number: u32,
    answered_sets: &[u32],
    open_set: Option<u32>,
) -> QuestionCardState {
    if open_set == Some(set_number) {
        QuestionCardState::Active
    } else if answered_sets.contains(&set_number) {
        QuestionCardState::Answered
    } else {
        QuestionCardState::Stale
    }
}

/// A stable key for a question set: its title and the question ids. The
/// draft map resets when it changes.
pub(crate) fn set_key(set: &BriefQuestionSet) -> String {
    let ids: Vec<&str> = set
        .questions
        .iter()
        .map(|question| question.id.as_str())
        .collect();
    format!("{}|{}", set.title, ids.join(","))
}

/// The card for the open question set: up to three questions and a
/// `Send answers` button, enabled once every question is complete.
#[component]
pub(crate) fn QuestionSetCard(
    set: BriefQuestionSet,
    drafts: Signal<HashMap<String, DraftAnswer>>,
    is_busy: bool,
    on_submit: EventHandler<Vec<QuestionAnswer>>,
) -> Element {
    let questions: Vec<BriefQuestion> = set.questions.iter().take(3).cloned().collect();
    let ready = answers_are_complete(&set, &drafts.read());
    let submit_set = set.clone();
    let submit = move |_| {
        let answers: Vec<QuestionAnswer> = submit_set
            .questions
            .iter()
            .take(3)
            .map(|question| {
                let draft = drafts.read().get(&question.id).cloned().unwrap_or_default();
                draft_to_answer(question, &draft)
            })
            .collect();
        on_submit.call(answers);
    };
    rsx! {
        div { class: "question-set",
            p { class: "question-set-title", "{set.title}" }
            p { class: "question-set-message", "{set.message}" }
            div { class: "question-cards",
                for question in questions {
                    QuestionCard {
                        key: "{question.id}",
                        question: question.clone(),
                        draft: drafts.read().get(&question.id).cloned().unwrap_or_default(),
                        on_change: {
                            let id = question.id.clone();
                            move |next: DraftAnswer| {
                                drafts.write().insert(id.clone(), next);
                            }
                        },
                    }
                }
            }
            div { class: "question-set-actions",
                button {
                    class: "primary",
                    disabled: !ready || is_busy,
                    onclick: submit,
                    "Send answers"
                }
                span { class: "question-hint",
                    "Required questions need an answer or Use your best judgment."
                }
            }
        }
    }
}

/// One question, rendered by its kind.
#[component]
pub(crate) fn QuestionCard(
    question: BriefQuestion,
    draft: DraftAnswer,
    on_change: EventHandler<DraftAnswer>,
) -> Element {
    let kind = question.kind;
    rsx! {
        div { class: "question-card",
            div { class: "question-head",
                span { class: "question-label", "{question.label}" }
                if question.required {
                    span { class: "badge required", "Required" }
                }
            }
            if let Some(rationale) = &question.rationale {
                p { class: "question-rationale", "{rationale}" }
            }
            match kind {
                QuestionKind::ShortText | QuestionKind::LongText => rsx! {
                    textarea {
                        class: "answer-textarea",
                        rows: if kind == QuestionKind::LongText { 3 } else { 1 },
                        placeholder: "Type your answer…",
                        value: "{draft.other_text}",
                        oninput: {
                            let draft = draft.clone();
                            move |event: FormEvent| {
                                let mut next = draft.clone();
                                next.other_text = event.value();
                                next.is_skipped = false;
                                on_change.call(next);
                            }
                        },
                    }
                },
                _ => rsx! {
                    div { class: "option-chips",
                        for option in question.options.iter() {
                            button {
                                key: "{option.value}",
                                class: if draft.values.contains(&option.value) { "option-chip selected" } else { "option-chip" },
                                onclick: {
                                    let draft = draft.clone();
                                    let value = option.value.clone();
                                    move |_| {
                                        let mut next = draft.clone();
                                        select_value(&mut next, kind, &value);
                                        on_change.call(next);
                                    }
                                },
                                "{option.label}"
                            }
                        }
                        if question.allow_other {
                            button {
                                class: if draft.is_other_open { "option-chip other selected" } else { "option-chip other" },
                                onclick: {
                                    let draft = draft.clone();
                                    move |_| {
                                        let mut next = draft.clone();
                                        toggle_other(&mut next, kind);
                                        on_change.call(next);
                                    }
                                },
                                "Other…"
                            }
                        }
                    }
                },
            }
            if question.allow_other && draft.is_other_open {
                input {
                    class: "other-input",
                    placeholder: "Describe it…",
                    value: "{draft.other_text}",
                    oninput: {
                        let draft = draft.clone();
                        move |event: FormEvent| {
                            let mut next = draft.clone();
                            next.other_text = event.value();
                            on_change.call(next);
                        }
                    },
                }
            }
            button {
                class: if draft.is_skipped { "option-chip skip selected" } else { "option-chip skip" },
                onclick: {
                    let draft = draft.clone();
                    move |_| {
                        let mut next = draft.clone();
                        skip_answer(&mut next);
                        on_change.call(next);
                    }
                },
                "Use your best judgment"
            }
        }
    }
}

/// A collapsed, answered set: one row per question with its summary.
#[component]
pub(crate) fn AnsweredCard(
    set: BriefQuestionSet,
    answers: Vec<QuestionAnswer>,
    state: QuestionCardState,
) -> Element {
    let kicker = if state == QuestionCardState::Stale {
        "Superseded"
    } else {
        "Your answers"
    };
    rsx! {
        div { class: "answered-card",
            span { class: "answered-kicker", "{kicker}" }
            for question in set.questions.iter() {
                {
                    let answer = answers.iter().find(|answer| answer.question_id == question.id);
                    let summary = answer
                        .map(|answer| answer_summary(question, answer))
                        .unwrap_or_else(|| "—".to_owned());
                    let skipped = answer.map(|answer| answer.skipped).unwrap_or(false);
                    let summary_class = if skipped {
                        "answered-summary skipped"
                    } else {
                        "answered-summary"
                    };
                    rsx! {
                        div { key: "{question.id}", class: "answered-row",
                            span { class: "answered-label", "{question.label}" }
                            span { class: "{summary_class}", "{summary}" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use design_model::QuestionOption;

    fn select(id: &str, required: bool) -> BriefQuestion {
        BriefQuestion {
            id: id.to_owned(),
            label: format!("Which {id}?"),
            rationale: None,
            kind: QuestionKind::SingleSelect,
            required,
            options: vec![
                QuestionOption {
                    value: "web".to_owned(),
                    label: "Web".to_owned(),
                },
                QuestionOption {
                    value: "app".to_owned(),
                    label: "Mobile app".to_owned(),
                },
            ],
            allow_other: true,
        }
    }

    #[test]
    fn single_select_replaces_the_value() {
        let mut draft = DraftAnswer::default();
        select_value(&mut draft, QuestionKind::SingleSelect, "app");
        select_value(&mut draft, QuestionKind::SingleSelect, "web");
        assert_eq!(draft.values, vec!["web"]);
    }

    #[test]
    fn multi_select_toggles_values() {
        let mut draft = DraftAnswer::default();
        select_value(&mut draft, QuestionKind::MultiSelect, "web");
        select_value(&mut draft, QuestionKind::MultiSelect, "app");
        select_value(&mut draft, QuestionKind::MultiSelect, "web");
        assert_eq!(draft.values, vec!["app"]);
    }

    #[test]
    fn choosing_a_value_clears_the_skip() {
        let mut draft = DraftAnswer {
            is_skipped: true,
            ..DraftAnswer::default()
        };
        select_value(&mut draft, QuestionKind::SingleSelect, "web");
        assert!(!draft.is_skipped);
    }

    #[test]
    fn skipping_clears_values_and_other_text() {
        let mut draft = DraftAnswer {
            values: vec!["web".to_owned()],
            other_text: "kiosk".to_owned(),
            is_other_open: true,
            is_skipped: false,
        };
        skip_answer(&mut draft);
        assert!(draft.is_skipped);
        assert!(draft.values.is_empty());
        assert!(draft.other_text.is_empty());
    }

    #[test]
    fn other_needs_text_to_be_complete() {
        let question = select("platform", true);
        let mut draft = DraftAnswer::default();
        toggle_other(&mut draft, QuestionKind::SingleSelect);
        assert!(!is_answer_complete(&question, &draft));
        draft.other_text = "kiosk".to_owned();
        assert!(is_answer_complete(&question, &draft));
    }

    #[test]
    fn required_questions_need_an_answer_unless_skipped() {
        let question = select("platform", true);
        assert!(!is_answer_complete(&question, &DraftAnswer::default()));
        let skipped = DraftAnswer {
            is_skipped: true,
            ..DraftAnswer::default()
        };
        assert!(is_answer_complete(&question, &skipped));
    }

    #[test]
    fn optional_questions_are_complete_when_empty() {
        let question = select("platform", false);
        assert!(is_answer_complete(&question, &DraftAnswer::default()));
    }

    #[test]
    fn text_questions_store_the_text_in_values() {
        let question = BriefQuestion {
            kind: QuestionKind::ShortText,
            options: Vec::new(),
            allow_other: false,
            ..select("goal", false)
        };
        let draft = DraftAnswer {
            other_text: "sign up".to_owned(),
            ..DraftAnswer::default()
        };
        let answer = draft_to_answer(&question, &draft);
        assert_eq!(answer.values, vec!["sign up"]);
        assert_eq!(answer.other_text, None);
    }

    #[test]
    fn a_set_is_complete_only_when_every_question_is() {
        let set = BriefQuestionSet {
            title: "T".to_owned(),
            message: "m".to_owned(),
            questions: vec![select("a", true), select("b", true)],
            can_proceed_with_assumptions: false,
        };
        let mut drafts = HashMap::new();
        drafts.insert("a".to_owned(), {
            let mut draft = DraftAnswer::default();
            select_value(&mut draft, QuestionKind::SingleSelect, "web");
            draft
        });
        assert!(!answers_are_complete(&set, &drafts));
        drafts.insert("b".to_owned(), {
            let mut draft = DraftAnswer::default();
            skip_answer(&mut draft);
            draft
        });
        assert!(answers_are_complete(&set, &drafts));
    }

    #[test]
    fn answer_summaries_use_option_labels_and_name_skips() {
        let question = select("platform", true);
        let answer = QuestionAnswer {
            question_id: "platform".to_owned(),
            values: vec!["app".to_owned(), "web".to_owned()],
            other_text: Some("kiosk".to_owned()),
            skipped: false,
        };
        assert_eq!(
            answer_summary(&question, &answer),
            "Mobile app, Web, + kiosk"
        );
        let skipped = QuestionAnswer {
            question_id: "platform".to_owned(),
            skipped: true,
            ..QuestionAnswer::default()
        };
        assert_eq!(
            answer_summary(&question, &skipped),
            "Use your best judgment"
        );
    }

    #[test]
    fn card_state_is_active_answered_or_stale() {
        assert_eq!(
            question_card_state(2, &[1], Some(2)),
            QuestionCardState::Active
        );
        assert_eq!(
            question_card_state(1, &[1], Some(2)),
            QuestionCardState::Answered
        );
        assert_eq!(
            question_card_state(3, &[1], Some(2)),
            QuestionCardState::Stale
        );
    }

    #[test]
    fn set_keys_change_with_the_question_ids() {
        let one = BriefQuestionSet {
            title: "T".to_owned(),
            message: "m".to_owned(),
            questions: vec![select("a", true)],
            can_proceed_with_assumptions: false,
        };
        let two = BriefQuestionSet {
            questions: vec![select("b", true)],
            ..one.clone()
        };
        assert_ne!(set_key(&one), set_key(&two));
    }
}
