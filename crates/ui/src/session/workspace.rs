use super::helpers::{
    chat_placeholder, is_chat_open, is_start_offered, pinned_reference, set_answers,
};
use super::*;

#[component]
pub(crate) fn SessionWorkspace(
    session_id: String,
    on_open_design: EventHandler<String>,
    on_open_deck: EventHandler<String>,
    on_open_document: EventHandler<String>,
    on_open_social: EventHandler<String>,
    on_open_print: EventHandler<String>,
    on_open_mailing: EventHandler<String>,
    on_open_campaign: EventHandler<String>,
    on_open_artwork: EventHandler<String>,
    on_home: EventHandler<()>,
) -> Element {
    let mut view = use_signal(|| Option::<api::SessionView>::None);
    let mut designs = use_signal(Vec::<api::DesignSummary>::new);
    let mut decks = use_signal(Vec::<api::DeckSummary>::new);
    let mut documents = use_signal(Vec::<api::DocumentSummary>::new);
    let mut socials = use_signal(Vec::<api::SocialSummary>::new);
    let mut prints = use_signal(Vec::<api::PrintSummary>::new);
    let mut mailings = use_signal(Vec::<api::MailingSummary>::new);
    let mut campaigns = use_signal(Vec::<api::CampaignSummary>::new);
    let mut artworks = use_signal(Vec::<api::ArtworkSummary>::new);
    let mut run = use_signal(|| Option::<api::AgentRun>::None);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let is_configuring = use_signal(|| false);
    let mut draft = use_signal(String::new);
    // ↑ and ↓ walk the prompts sent before, as a shell does.
    let mut history = use_signal(PromptHistory::default);
    let drafts = use_signal(HashMap::<String, DraftAnswer>::new);
    let mut drafts_key = use_signal(String::new);
    let mut revision = use_signal(|| 0u64);
    let mut error = use_signal(|| Option::<String>::None);
    // The candidates pinned with `@` for the next message, the `@` menu
    // row, the draft the menu was closed on, and the caret.
    let mut pinned = use_signal(Vec::<String>::new);
    let mut mention_row = use_signal(|| 0usize);
    let mut mention_closed_on = use_signal(|| Option::<String>::None);
    let caret = use_signal(|| 0usize);

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
                    if let Ok(list) = api::fetch_document_list().await {
                        documents.set(list);
                    }
                    if let Ok(list) = api::fetch_social_list().await {
                        socials.set(list);
                    }
                    if let Ok(list) = api::fetch_print_list().await {
                        prints.set(list);
                    }
                    if let Ok(list) = api::fetch_mailing_list().await {
                        mailings.set(list);
                    }
                    if let Ok(list) = api::fetch_campaign_list().await {
                        campaigns.set(list);
                    }
                    if let Ok(list) = api::fetch_artwork_list().await {
                        artworks.set(list);
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

    use_follow_thread(
        "session-thread",
        view().map(|session| session.messages.len() + 1),
    );

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
    let prompts = prompt_entries(&session_view.messages);
    let state = session.state;
    let can_skip = session_view
        .question_sets
        .last()
        .map(|set| set.can_proceed_with_assumptions)
        .unwrap_or(false);
    let run_value = run();
    let is_running = run_value.as_ref().is_some_and(|run| run.is_running);
    let can_chat = is_chat_open(state, is_running);
    let send_label = if is_running || state == WorkflowState::Generating {
        "Queue"
    } else {
        "Send"
    };

    let artifact_kind = session.artifact_kind;
    let cards = match session.artifact_kind {
        ArtifactKind::Demo => {
            cards_from_designs(&designs(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Deck => {
            cards_from_decks(&decks(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Document => {
            cards_from_documents(&documents(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Social => {
            cards_from_socials(&socials(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Print => {
            cards_from_prints(&prints(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Mailing => {
            cards_from_mailings(&mailings(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Campaign => {
            cards_from_campaigns(&campaigns(), &session_id, session.chosen_design.as_deref())
        }
        ArtifactKind::Artwork => {
            cards_from_artworks(&artworks(), &session_id, session.chosen_design.as_deref())
        }
    };
    // A placeholder before the first card takes the kind's canvas: the
    // paper the options name for a document, the format they name for
    // a social or a mailing, the size and orientation they name for a
    // print, the ad size they name for a campaign, the cover size they
    // name for an artwork, the deck canvas, or the desktop.
    let blank_viewport = match session.artifact_kind {
        ArtifactKind::Demo => Viewport::default(),
        ArtifactKind::Deck => DECK_VIEWPORT,
        ArtifactKind::Document => session
            .options
            .paper
            .as_deref()
            .and_then(Paper::from_name)
            .unwrap_or_default()
            .viewport(),
        ArtifactKind::Social => session
            .options
            .format
            .as_deref()
            .and_then(Format::from_name)
            .unwrap_or_default()
            .viewport(),
        ArtifactKind::Print => {
            let size = session
                .options
                .print_size
                .as_deref()
                .and_then(PrintSize::from_name)
                .unwrap_or_default();
            session
                .options
                .orientation
                .as_deref()
                .and_then(Orientation::from_name)
                .unwrap_or_default()
                .apply(size.viewport())
        }
        ArtifactKind::Mailing => session
            .options
            .email_format
            .as_deref()
            .and_then(EmailFormat::from_name)
            .unwrap_or_default()
            .viewport(),
        ArtifactKind::Campaign => session
            .options
            .ad_size
            .as_deref()
            .and_then(AdSize::from_name)
            .unwrap_or_default()
            .viewport(),
        ArtifactKind::Artwork => session
            .options
            .cover_size
            .as_deref()
            .and_then(CoverSize::from_name)
            .unwrap_or_default()
            .viewport(),
    };
    // The `@` menu: the candidates that match what follows the `@`.
    let candidate_ids: Vec<String> = cards.iter().map(|card| card.id.clone()).collect();
    let mention = if candidate_ids.is_empty() || mention_closed_on() == Some(draft()) {
        None
    } else {
        mention_at(&draft(), caret())
    };
    let mention_rows: Vec<CandidateMention> = mention
        .as_ref()
        .map(|(_, query)| candidate_mentions(&candidate_ids, query))
        .unwrap_or_default();
    // Picks row `row` of the menu: its candidates are pinned, and the
    // `@` and its query leave the draft.
    let pick_mention = {
        let rows = mention_rows.clone();
        let start = mention.as_ref().map(|(start, _)| *start);
        move |row: usize| {
            if let (Some(start), Some(entry)) = (start, rows.get(row)) {
                draft.set(remove_mention(&draft(), start, caret()));
                mention_row.set(0);
                pin_candidates(&mut pinned.write(), &entry.ids);
            }
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
    // The turns reverted in this view, so the button does not offer
    // the same revert twice.
    let mut reverted = use_signal(HashSet::<usize>::new);
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
            history.write().reset();
            let session_id = session_id.clone();
            // The pins were for this message. The next one starts clean.
            let targets = std::mem::take(&mut *pinned.write());
            let content = match pinned_reference(&targets) {
                Some(reference) => format!("{reference} {text}"),
                None => text,
            };
            // Every message is a turn: the planner answers, asks,
            // writes, or edits the pinned or chosen candidates.
            spawn(async move {
                let sent = api::send_session_message_about(&session_id, &content, &targets).await;
                if let Err(message) = sent {
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

    let has_queued = session_view
        .messages
        .iter()
        .any(|message| message.is_queued);
    let can_start = is_start_offered(state, is_running, open_set.is_some())
        || (has_queued && !is_running && state == WorkflowState::Reviewing);

    rsx! {
        main { class: "session",
            section { class: "conversation",
                div { class: "studio-head",
                    button { class: "back", onclick: move |_| on_home.call(()), "‹ Sessions" }
                    span { class: "studio-title", "{session.title}" }
                }
                div { id: "session-thread", class: "thread",
                    div { class: "bubble user", "{session.request}" }
                    for (index, message) in session_view.messages.iter().enumerate() {
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
                            // A turn that wrote artifacts can be reverted
                            // while nothing runs: each artifact goes back to
                            // its snapshot from before the turn.
                            let has_restore_points = message
                                .snapshot_stamps
                                .as_ref()
                                .is_none_or(|snapshots| {
                                    message.artifacts.iter().all(|id| snapshots.contains_key(id))
                                });
                            let revert_since = (!message.artifacts.is_empty() && has_restore_points
                                && !is_running && !has_queued && state != WorkflowState::Generating)
                                .then(|| turn_start(&session_view.messages, index))
                                .flatten();
                            let is_reverted = reverted().contains(&index);
                            rsx! {
                                div { class: "{bubble_class}",
                                    if message.is_queued {
                                        small {
                                            class: "queued-prompt",
                                            title: "Runs after the current turn, using the latest saved candidate",
                                            "Queued"
                                        }
                                    }
                                    p { "{message.content}" }
                                }
                                if let Some(since) = revert_since {
                                    button {
                                        class: "revert-turn",
                                        disabled: is_reverted,
                                        title: "Put every artifact of this turn back to how it was before it",
                                        onclick: {
                                            let artifacts = message.artifacts.clone();
                                            let snapshots = message.snapshot_stamps.clone(); // The setup card asked the app's
                                            let kind = session.artifact_kind;
                                            move |_| {
                                                let artifacts = artifacts.clone();
                                                let snapshots = snapshots.clone();
                                                let since = since.clone();
                                                spawn(async move {
                                                    for id in &artifacts {
                                                        let stamp = snapshots
                                                            .as_ref()
                                                            .and_then(|snapshots| snapshots.get(id))
                                                            .map(String::as_str);
                                                        let done = revert_artifact(kind, id, &since, stamp).await;
                                                        if let Err(message) = done {
                                                            error.set(Some(message));
                                                            return;
                                                        }
                                                    }
                                                    reverted.write().insert(index);
                                                });
                                            }
                                        },
                                        if is_reverted {
                                            "Reverted"
                                        } else {
                                            "Revert this turn"
                                        }
                                    }
                                }
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
                    if let Some(reference) = pinned_reference(&pinned()) {
                        div { class: "context-chip mono",
                            span { "{reference}" }
                            button {
                                title: "Drop the pinned candidates",
                                onclick: move |_| pinned.write().clear(),
                                "×"
                            }
                        }
                    }
                    if !mention_rows.is_empty() {
                        div { class: "mention-menu", role: "listbox",
                            for (row, entry) in mention_rows.iter().enumerate() {
                                button {
                                    key: "{entry.key}",
                                    class: if row == mention_row() { "mention-item active" } else { "mention-item" },
                                    role: "option",
                                    onmousedown: move |event: Event<MouseData>| event.prevent_default(),
                                    onclick: {
                                        let mut pick_mention = pick_mention.clone();
                                        move |_| pick_mention(row)
                                    },
                                    span { class: "mono", "{entry.key}" }
                                    span { "{entry.label}" }
                                }
                            }
                        }
                    }
                    textarea {
                        placeholder: chat_placeholder(state),
                        disabled: !can_chat,
                        value: "{draft}",
                        oninput: move |event: FormEvent| {
                            draft.set(event.value());
                            mention_row.set(0);
                            history.write().reset();
                            watch_caret(caret);
                        },
                        onkeyup: move |_| watch_caret(caret),
                        onmouseup: move |_| watch_caret(caret),
                        onkeydown: {
                            let mut pick_mention = pick_mention.clone();
                            let row_count = mention_rows.len();
                            let prompts = prompts.clone();
                            move |event: KeyboardEvent| {
                                if row_count == 0
                                    && recall_prompt(&event, &prompts, &mut history.write(), &mut draft)
                                {
                                    return;
                                }
                                if row_count > 0 {
                                    match event.key() {
                                        Key::ArrowDown => {
                                            event.prevent_default();
                                            mention_row.set((mention_row() + 1) % row_count);
                                            return;
                                        }
                                        Key::ArrowUp => {
                                            event.prevent_default();
                                            mention_row.set((mention_row() + row_count - 1) % row_count);
                                            return;
                                        }
                                        Key::Enter | Key::Tab => {
                                            event.prevent_default();
                                            pick_mention(mention_row());
                                            return;
                                        }
                                        Key::Escape => {
                                            mention_closed_on.set(Some(draft()));
                                            return;
                                        }
                                        _ => {}
                                    }
                                }
                                if event.key() == Key::Enter && !event.modifiers().shift() {
                                    event.prevent_default();
                                    send_message.call(());
                                }
                            }
                        },
                    }
                    div { class: "chat-controls",
                        ModelChip {
                            settings,
                            is_configuring,
                            effort: Some(session.options.effort.clone()),
                            on_effort: {
                                let session_id = session_id.clone();
                                let options = session.options.clone();
                                move |level: String| {
                                    let next = with_effort(&options, &level);
                                    if let Some(view) = view.write().as_mut() {
                                        view.session.options = next.clone();
                                    }
                                    let session_id = session_id.clone();
                                    spawn(async move {
                                        let saved = api::save_session_options(&session_id, &next).await;
                                        if let Err(message) = saved {
                                            error.set(Some(message));
                                        }
                                    });
                                }
                            },
                        }
                        SendButton {
                            label: send_label,
                            is_enabled: can_chat && !draft.read().trim().is_empty(),
                            on_send: move |_| send_message.call(()),
                        }
                    }
                }
            }
            section { class: "workbench",
                // The open question set owns the workbench while it is
                // open: it is the one thing waiting on the user.
                if let Some((_number, set)) = open_set_card {
                    div { class: "workbench-questions",
                        QuestionSetCard {
                            key: "{_number}",
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
                                if session.artifact_kind == ArtifactKind::Document {
                                    DocumentQuestions {
                                        session_id: session_id.clone(),
                                        options: session.options.clone(),
                                        on_error: move |message| error.set(Some(message)),
                                    }
                                }
                                if session.artifact_kind == ArtifactKind::Social {
                                    SocialQuestions {
                                        session_id: session_id.clone(),
                                        options: session.options.clone(),
                                        on_error: move |message| error.set(Some(message)),
                                    }
                                }
                                if session.artifact_kind == ArtifactKind::Print {
                                    PrintQuestions {
                                        session_id: session_id.clone(),
                                        options: session.options.clone(),
                                        on_error: move |message| error.set(Some(message)),
                                    }
                                }
                                if session.artifact_kind == ArtifactKind::Mailing {
                                    MailingQuestions {
                                        session_id: session_id.clone(),
                                        options: session.options.clone(),
                                        on_error: move |message| error.set(Some(message)),
                                    }
                                }
                                if session.artifact_kind == ArtifactKind::Campaign {
                                    CampaignQuestions {
                                        session_id: session_id.clone(),
                                        options: session.options.clone(),
                                        on_error: move |message| error.set(Some(message)),
                                    }
                                }
                                if session.artifact_kind == ArtifactKind::Artwork {
                                    ArtworkQuestions {
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
                if !is_asking && is_canvas_shown(state, !cards.is_empty()) {
                    CandidateCanvas {
                        session_id: session_id.clone(),
                        cards,
                        run_designs,
                        queued,
                        on_error: move |message| error.set(Some(message)),
                        revision: revision(),
                        chosen: session.chosen_design.clone(),
                        kind: artifact_kind,
                        blank_viewport,
                        on_open: move |(kind, id): (ArtifactKind, String)| match kind {
                            ArtifactKind::Demo => on_open_design.call(id),
                            ArtifactKind::Deck => on_open_deck.call(id),
                            ArtifactKind::Document => on_open_document.call(id),
                            ArtifactKind::Social => on_open_social.call(id),
                            ArtifactKind::Print => on_open_print.call(id),
                            ArtifactKind::Mailing => on_open_mailing.call(id),
                            ArtifactKind::Campaign => on_open_campaign.call(id),
                            ArtifactKind::Artwork => on_open_artwork.call(id),
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
                        on_fork: move |artifact_id: String| {
                            spawn(async move {
                                let forked = match artifact_kind {
                                    ArtifactKind::Demo => api::fork_design(&artifact_id).await,
                                    ArtifactKind::Deck => api::fork_deck(&artifact_id).await,
                                    ArtifactKind::Document => api::fork_document(&artifact_id).await,
                                    ArtifactKind::Social => api::fork_social(&artifact_id).await,
                                    ArtifactKind::Print => api::fork_print(&artifact_id).await,
                                    ArtifactKind::Mailing => api::fork_mailing(&artifact_id).await,
                                    ArtifactKind::Campaign => api::fork_campaign(&artifact_id).await,
                                    ArtifactKind::Artwork => api::fork_artwork(&artifact_id).await,
                                };
                                if let Err(message) = forked {
                                    error.set(Some(message));
                                }
                            });
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
