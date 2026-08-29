//! The session workspace: the conversation on the left, the brief and
//! the candidate canvas on the right. A long-poll on `GET /events`
//! keeps both live.

use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;

use crate::api;
use crate::canvas::{CandidateCanvas, cards_from_decks, cards_from_designs, queued_finishes};
use crate::chat_controls::{ModelChip, SendButton};
use crate::question_card::{
    DraftAnswer, QaRow, QuestionCardState, QuestionSetCard, answered_entries, question_card_state,
    set_key,
};
use crate::run_settings::{DeckQuestions, RunSettings, SharedQuestions, app_answers};
use crate::settings::{SettingsPanel, pause_briefly};
use crate::status::{RunStatusCard, working_label};
use design_model::{ArtifactKind, QuestionAnswer, WorkflowState};

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

/// The session workspace.
#[component]
pub(crate) fn SessionWorkspace(
    session_id: String,
    on_open_design: EventHandler<String>,
    on_open_deck: EventHandler<String>,
    on_home: EventHandler<()>,
) -> Element {
    let mut view = use_signal(|| Option::<api::SessionView>::None);
    let mut designs = use_signal(Vec::<api::DesignSummary>::new);
    let mut decks = use_signal(Vec::<api::DeckSummary>::new);
    let mut run = use_signal(|| Option::<api::AgentRun>::None);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let is_configuring = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let drafts = use_signal(HashMap::<String, DraftAnswer>::new);
    let mut drafts_key = use_signal(String::new);
    let mut revision = use_signal(|| 0u64);
    let mut error = use_signal(|| Option::<String>::None);

    // The live loop: refetch on every server change.
    {
        let session_id = session_id.clone();
        use_future(move || {
            let session_id = session_id.clone();
            async move {
                let mut seen = 0u64;
                loop {
                    match api::fetch_session(&session_id).await {
                        Ok(fetched) => view.set(Some(fetched)),
                        Err(message) => error.set(Some(message)),
                    }
                    if let Ok(list) = api::fetch_design_list().await {
                        designs.set(list);
                    }
                    if let Ok(list) = api::fetch_deck_list().await {
                        decks.set(list);
                    }
                    if let Ok(fetched) = api::fetch_agent_run().await {
                        run.set(Some(fetched));
                    }
                    if let Ok(fetched) = api::fetch_settings().await {
                        settings.set(Some(fetched));
                    }
                    revision.set(seen);
                    match api::wait_for_change(seen).await {
                        Ok(next) => seen = next,
                        Err(_) => pause_briefly().await,
                    }
                }
            }
        });
    }

    // Reset the answer drafts when the open question set changes.
    if let Some(view) = view()
        && let Some(open) = view.open_question_set
        && let Some(set) = view.question_sets.get((open as usize).saturating_sub(1))
    {
        let key = set_key(set);
        if *drafts_key.read() != key {
            drafts_key.set(key);
            drafts.clone().set(HashMap::new());
        }
    }

    let Some(session_view) = view() else {
        // A failed fetch must not hide behind the loading line.
        let failure = error();
        return rsx! {
            main { class: "session",
                if let Some(message) = failure {
                    p { class: "error", "{message}" }
                } else {
                    p { class: "lede", "Loading…" }
                }
            }
        };
    };
    let session = session_view.session.clone();
    let state = session.state;
    let can_skip = session_view
        .question_sets
        .last()
        .map(|set| set.can_proceed_with_assumptions)
        .unwrap_or(false);
    let run_value = run();
    let is_running = run_value.as_ref().is_some_and(|run| run.is_running);
    let can_chat = is_chat_open(state, is_running);

    let cards = match session.artifact_kind {
        ArtifactKind::Demo => {
            cards_from_designs(&designs(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Deck => {
            cards_from_decks(&decks(), &session_id, session.chosen_design.as_deref())
        }
    };
    // Only a running turn reports progress. The finished run keeps its
    // last percentages, and reading those left every card marked
    // `writing` with its Finish button hidden.
    let run_designs = run_value
        .as_ref()
        .filter(|run| run.is_running)
        .map(|run| run.designs.clone())
        .unwrap_or_default();
    // A pressed Finish shows as queued at once, before the poll brings
    // the message back, and stays queued while the run holds it.
    let mut pressed = use_signal(HashSet::<String>::new);
    let mut queued = queued_finishes(&session_view.messages, is_running);
    queued.extend(pressed().into_iter());

    let answered_sets: Vec<u32> = session_view
        .answers
        .iter()
        .map(|record| record.question_set)
        .collect();

    let send_message = use_callback({
        let session_id = session_id.clone();
        move |_: ()| {
            let text = draft.read().trim().to_owned();
            if text.is_empty() {
                return;
            }
            draft.set(String::new());
            let session_id = session_id.clone();
            // Every message is a turn: the planner answers, asks,
            // writes, or edits the chosen artifact.
            spawn(async move {
                if let Err(message) = api::send_session_message(&session_id, &text, None).await {
                    error.set(Some(message));
                }
            });
        }
    });

    // A message from the session left behind belongs to that session.
    use_effect(use_reactive!(|session_id| {
        let _ = session_id;
        error.set(None);
    }));

    let open_set = session_view.open_question_set;
    // The set the workbench asks: the open one, with its number, when it
    // is still unanswered. `None` leaves the workbench to the run
    // settings and the candidates.
    let open_set_card = open_set.and_then(|number| {
        if answered_sets.contains(&number) {
            return None;
        }
        session_view
            .question_sets
            .get((number as usize).saturating_sub(1))
            .map(|set| (number, set.clone()))
    });
    let is_asking = open_set_card.is_some();
    let skip_questions = use_callback({
        let session_id = session_id.clone();
        move |_: ()| {
            let session_id = session_id.clone();
            spawn(async move {
                if let Err(message) = api::generate_now(&session_id).await {
                    error.set(Some(message));
                }
            });
        }
    });
    let submit_answers = use_callback({
        let session_id = session_id.clone();
        move |answers: Vec<QuestionAnswer>| {
            let Some(question_set) = open_set else {
                return;
            };
            let session_id = session_id.clone();
            spawn(async move {
                if let Err(message) =
                    api::send_session_answers(&session_id, question_set, &answers).await
                {
                    error.set(Some(message));
                }
            });
        }
    });

    // Both the stopped card and the failed card resume through this, so
    // it is a callback rather than a closure one of them would move.
    let resume = use_callback({
        let session_id = session_id.clone();
        move |_: ()| {
            let session_id = session_id.clone();
            spawn(async move {
                let _ = api::retry_session(&session_id).await;
            });
        }
    });

    let start = {
        let session_id = session_id.clone();
        move |_| {
            let session_id = session_id.clone();
            spawn(async move {
                let _ = api::start_agent_run(&session_id).await;
            });
        }
    };

    let can_start = is_start_offered(state, is_running, open_set.is_some());

    rsx! {
        main { class: "session",
            section { class: "conversation",
                div { class: "studio-head",
                    button { class: "back", onclick: move |_| on_home.call(()), "‹ Sessions" }
                    span { class: "studio-title", "{session.title}" }
                }
                div { class: "thread",
                    div { class: "bubble user", "{session.request}" }
                    for message in session_view.messages.iter() {
                        {
                            let bubble_class = if message.role == "user" {
                                "bubble user"
                            } else {
                                "bubble agent"
                            };
                            let question_set = message
                                .question_set
                                .and_then(|number| {
                                    session_view
                                        .question_sets
                                        .get((number as usize).saturating_sub(1))
                                        .map(|set| (number, set.clone()))
                                });
                            rsx! {
                                div { class: "{bubble_class}", "{message.content}" }
                                if let Some((number, set)) = question_set {
                                    {
                                        let card_state = question_card_state(
                                            number,
                                            &answered_sets,
                                            session_view.open_question_set,
                                        );
                                        match card_state {
                                            // The open set is asked in the
                                            // workbench, not here. The chat
                                            // keeps the record of what was
                                            // answered.
                                            QuestionCardState::Active => rsx! {},
                                            QuestionCardState::Answered => rsx! {
                                                div { class: "thread-answers",
                                                    // The setup card asked the app's
                                                    // questions too. Their answers live
                                                    // on the options, so the first
                                                    // record reads them from there.
                                                    if number == 1 {
                                                        for entry in app_answers(session.artifact_kind, &session.options) {
                                                            QaRow {
                                                                question: entry.question.clone(),
                                                                answer: entry.answer.clone(),
                                                                is_assumed: entry.is_assumed,
                                                            }
                                                        }
                                                    }
                                                    for entry in set_answers(&set, &session_view.answers, number) {
                                                        QaRow {
                                                            question: entry.question.clone(),
                                                            answer: entry.answer.clone(),
                                                            is_assumed: entry.is_assumed,
                                                        }
                                                    }
                                                }
                                            },
                                            QuestionCardState::Stale => rsx! {
                                                p { class: "chat-note", "Superseded." }
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // A stop is not a failure: it gets a plain card and
                    // a Resume button, with nothing to report.
                    if state == WorkflowState::Stopped {
                        div { class: "stopped-card",
                            p { class: "stopped-title", "The run stopped" }
                            p { "Nothing was lost. Resume to carry on from where it stopped." }
                            button {
                                class: "primary",
                                onclick: move |_| resume.call(()),
                                "Resume"
                            }
                        }
                    }
                    if state == WorkflowState::Error {
                        div { class: "error-card",
                            p { class: "error-title", "The run failed" }
                            if let Some(message) = &session.error {
                                p { "{message}" }
                            }
                            button {
                                class: "primary",
                                onclick: move |_| resume.call(()),
                                "Retry"
                            }
                        }
                    }
                    if let Some(run) = &run_value {
                        div { class: "status-card",
                            p { class: "progress-step", "{progress_label(state, Some(run))}" }
                            RunStatusCard { run: run.clone() }
                        }
                    }
                    if can_start {
                        button { class: "secondary start-run", onclick: start, "Start the agent" }
                    }
                }
                div { class: "chat-box",
                    if is_configuring() {
                        div { class: "chat-settings",
                            SettingsPanel { settings, is_configuring }
                        }
                    }
                    textarea {
                        placeholder: chat_placeholder(state),
                        disabled: !can_chat,
                        value: "{draft}",
                        oninput: move |event: FormEvent| draft.set(event.value()),
                        onkeydown: move |event: KeyboardEvent| {
                            if event.key() == Key::Enter && !event.modifiers().shift() {
                                event.prevent_default();
                                send_message.call(());
                            }
                        },
                    }
                    div { class: "chat-controls",
                        ModelChip { settings, is_configuring }
                        SendButton {
                            label: "Send",
                            is_enabled: can_chat && !draft.read().trim().is_empty(),
                            on_send: move |_| send_message.call(()),
                        }
                    }
                }
            }
            section { class: "workbench",
                // The open question set owns the workbench while it is
                // open: it is the one thing waiting on the user.
                if let Some((number, set)) = open_set_card {
                    div { class: "workbench-questions",
                        QuestionSetCard {
                            key: "{number}",
                            set,
                            drafts,
                            is_busy: is_running,
                            can_skip,
                            on_submit: move |answers| submit_answers.call(answers),
                            on_skip: move |_| skip_questions.call(()),
                            // The app's own questions sit in the same
                            // grid as the agent's, as Swift Deck did.
                            // The recurring axes are shared; the rest
                            // are per kind.
                            app_questions: Some(rsx! {
                                SharedQuestions {
                                    session_id: session_id.clone(),
                                    kind: session.artifact_kind,
                                    options: session.options.clone(),
                                    on_error: move |message| error.set(Some(message)),
                                }
                                if session.artifact_kind == ArtifactKind::Deck {
                                    DeckQuestions {
                                        session_id: session_id.clone(),
                                        options: session.options.clone(),
                                        on_error: move |message| error.set(Some(message)),
                                    }
                                }
                            }),
                            // A demo's run settings belong with the
                            // app's cards: the card is open on the
                            // first turn, so this is the only place
                            // they appear before the first candidates.
                            app_settings: if session.artifact_kind == ArtifactKind::Demo { Some(rsx! {
                                RunSettings {
                                    session_id: session_id.clone(),
                                    options: session.options.clone(),
                                    on_error: move |message| error.set(Some(message)),
                                }
                            }) } else { None },
                        }
                    }
                }
                // Before the first candidates, the demo's run settings sit
                // where the candidates will be.
                if !is_asking && session.artifact_kind == ArtifactKind::Demo && cards.is_empty()
                    && can_chat
                {
                    RunSettings {
                        session_id: session_id.clone(),
                        options: session.options.clone(),
                        on_error: move |message| error.set(Some(message)),
                    }
                }
                if !is_asking && is_canvas_shown(state, !cards.is_empty()) {
                    CandidateCanvas {
                        session_id: session_id.clone(),
                        cards,
                        run_designs,
                        queued,
                        on_error: move |message| error.set(Some(message)),
                        revision: revision(),
                        chosen: session.chosen_design.clone(),
                        on_open: move |(kind, id): (ArtifactKind, String)| match kind {
                            ArtifactKind::Demo => on_open_design.call(id),
                            ArtifactKind::Deck => on_open_deck.call(id),
                        },
                        on_continue: {
                            let session_id = session_id.clone();
                            move |artifact_id: String| {
                                let session_id = session_id.clone();
                                pressed.write().insert(artifact_id.clone());
                                spawn(async move {
                                    let sent = api::continue_artifact(&session_id, &artifact_id).await;
                                    if let Err(message) = sent {
                                        pressed.write().remove(&artifact_id);
                                        error.set(Some(message));
                                    }
                                });
                            }
                        },
                    }
                }
                if let Some(message) = error() {
                    p { class: "error", "{message}" }
                }
            }
        }
    }
}

/// True when a run should be active and none is, so the studio offers
/// `Start the agent`: reading the request, drafting after the answers,
/// or generating. Not while a question set waits for the user.
fn is_start_offered(state: WorkflowState, is_running: bool, has_open_set: bool) -> bool {
    if is_running {
        return false;
    }
    match state {
        WorkflowState::Intake | WorkflowState::Generating => true,
        WorkflowState::Clarifying => !has_open_set,
        _ => false,
    }
}

/// True when the chat accepts a message: not while a run is active,
/// since a message sent then would wait unread, and not while the
/// session generates. After an error the chat stays open: the next
/// message is the retry.
fn is_chat_open(state: WorkflowState, is_running: bool) -> bool {
    !is_running && state != WorkflowState::Generating
}

/// The answers the user gave to question set `number`, as Q and A text.
fn set_answers(
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
fn chat_placeholder(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Intake => "Reply, or add detail…",
        WorkflowState::Clarifying => "Answer the questions above, or reply here…",
        WorkflowState::Generating => "Generating… send after it finishes",
        WorkflowState::Reviewing => "Ask for a change, or for new candidates…",
        WorkflowState::Stopped => "The run stopped. Send a message to carry on…",
        WorkflowState::Error => "The run failed. Send a message to try again…",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        chat_placeholder, generation_step, is_canvas_shown, is_chat_open, is_start_offered,
        progress_label,
    };
    use design_model::WorkflowState;

    #[test]
    fn the_chat_is_open_except_while_a_run_works() {
        assert!(is_chat_open(WorkflowState::Intake, false));
        assert!(is_chat_open(WorkflowState::Reviewing, false));
        assert!(is_chat_open(WorkflowState::Error, false));
        // A stopped session takes a message: it resumes on the way in.
        assert!(is_chat_open(WorkflowState::Stopped, false));
        assert!(!is_chat_open(WorkflowState::Generating, false));
        assert!(!is_chat_open(WorkflowState::Reviewing, true));
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

    fn run(log: &str) -> crate::api::AgentRun {
        crate::api::AgentRun {
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
            designs: std::collections::HashMap::new(),
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
}
