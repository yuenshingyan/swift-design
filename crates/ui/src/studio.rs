//! The studio: a conversation column beside a live canvas.
//!
//! The conversation holds the brief, the chat, and the run status. The
//! canvas shows the agent's open questions and the designs and candidates
//! as the agent writes them. A long-poll on `GET /events` keeps both live, so the
//! screen updates the moment the agent acts. Swift Design makes no LLM
//! API calls; the user's own agent does the writing.

use std::collections::HashMap;

use dioxus::document;
use dioxus::prelude::*;

use crate::TopbarContext;
use crate::api;
use crate::chat_controls::{ModelChip, SendButton};
use crate::icons;
use crate::select::Select;
use crate::status::{RunStatusCard, working_label};
use crate::uploads::{AttachButton, AttachmentChips};

/// The screen a card shows after one arrow press: `step` of `-1` or
/// `1` from `current`, clamped to `1..=count`.
pub(crate) fn stepped_screen(current: usize, step: i32, count: usize) -> usize {
    let count = count.max(1);
    let next = if step < 0 {
        current.saturating_sub(1)
    } else {
        current + 1
    };
    next.clamp(1, count)
}

/// Waits two seconds. Used to back off after a failed poll.
pub(crate) async fn pause_briefly() {
    let mut sleeper = document::eval("setTimeout(() => dioxus.send(0), 2000);");
    let _ = sleeper.recv::<i32>().await;
}

/// The project a design belongs to: its id up to any candidate suffix.
pub(crate) fn design_project(id: &str) -> String {
    match id.find("-candidate-") {
        Some(position) => id[..position].to_owned(),
        None => id.to_owned(),
    }
}

/// Designs grouped by project, in listing order.
pub(crate) fn project_groups(
    designs: &[api::DesignSummary],
) -> Vec<(String, Vec<api::DesignSummary>)> {
    let mut groups: Vec<(String, Vec<api::DesignSummary>)> = Vec::new();
    for design in designs {
        let project = design_project(&design.id);
        match groups.iter_mut().find(|(group, _)| *group == project) {
            Some((_, members)) => members.push(design.clone()),
            None => groups.push((project, vec![design.clone()])),
        }
    }
    groups
}

/// The card caption under the design name: `Candidate N` or `Chosen`,
/// then the screen count. A preview says how many screens are written of
/// the plan.
fn card_label(design: &api::DesignSummary) -> String {
    let detail = if design.is_preview() {
        format!(
            "preview {} of {} screens",
            design.screen_count, design.outline_count
        )
    } else {
        format!("{} screens", design.screen_count)
    };
    format!("{} · {detail}", candidate_name(&design.id))
}

/// `Candidate N` for a candidate id, `Chosen` for the project design.
fn candidate_name(id: &str) -> String {
    match id.rfind("-candidate-") {
        Some(position) => format!("Candidate {}", &id[position + "-candidate-".len()..]),
        None => "Chosen".to_owned(),
    }
}

/// The brief's length for the meta line: `10–15 screens`. `None` when
/// the user has not chosen a length or left it to the model.
fn length_label(brief: &api::Brief) -> Option<String> {
    let length = brief.length.as_deref()?;
    let (min, max) = length.split_once('-')?;
    if min == max {
        return Some(format!("{min} screens"));
    }
    Some(format!("{min}–{max} screens"))
}

/// The run settings shown as tags under the brief: effort, scenario,
/// length, variations, variety, and `previews` when the run writes
/// previews first.
fn brief_tags(brief: &api::Brief) -> Vec<String> {
    let mut tags = vec![format!("effort {}", brief.effort)];
    if let Some(scenario) = &brief.scenario {
        tags.push(scenario.clone());
    }
    if let Some(length) = length_label(brief) {
        tags.push(length);
    }
    if let Some(count) = brief.variations {
        tags.push(format!("{count} variations"));
    }
    if let Some(variety) = &brief.variety {
        tags.push(format!("variety {variety}"));
    }
    if brief.preview {
        tags.push("previews".to_owned());
    }
    tags
}

/// Toggles `option` in one question's answer set. `You decide` stands
/// alone: choosing it clears the others, and choosing an option
/// removes it.
fn toggle_option(selected: &mut Vec<String>, option: &str) {
    if let Some(position) = selected.iter().position(|chosen| chosen == option) {
        selected.remove(position);
        return;
    }
    if option == "You decide" {
        selected.clear();
    } else {
        selected.retain(|chosen| chosen != "You decide");
    }
    selected.push(option.to_owned());
}

/// True when the user asked to continue `design_id` and no plain user
/// turn followed: the request still waits for the engine. Mirrors the
/// server's queue rule.
pub(crate) fn is_continue_pending(messages: &[api::ChatMessage], design_id: &str) -> bool {
    for message in messages.iter().rev() {
        if message.role != "user" {
            continue;
        }
        if message.action.as_deref() != Some("continue") {
            return false;
        }
        if message.design.as_deref() == Some(design_id) {
            return true;
        }
    }
    false
}

/// Scrolls the conversation to its newest message. Runs after the
/// thread renders, when the message count changes.
const SCROLL_TO_LATEST: &str = "\
const scrollThread = () => {
  const thread = document.querySelector('.thread');
  if (thread) { thread.scrollTop = thread.scrollHeight; }
};
scrollThread();
setTimeout(scrollThread, 120);
";

/// The studio screen for one project: conversation on the left, live
/// canvas on the right.
#[component]
pub fn Studio(
    project: String,
    on_open: EventHandler<String>,
    on_home: EventHandler<()>,
    on_rename: EventHandler<String>,
) -> Element {
    let mut brief = use_signal(|| Option::<api::Brief>::None);
    let mut open_questions = use_signal(|| Option::<Vec<api::Question>>::None);
    let mut designs = use_signal(Vec::<api::DesignSummary>::new);
    let mut agent_run = use_signal(|| Option::<api::AgentRun>::None);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let mut is_configuring = use_signal(|| false);
    let mut chosen = use_signal(Vec::<Vec<String>>::new);
    let mut questions_key = use_signal(String::new);
    let mut revision = use_signal(|| 0u64);
    let mut is_loaded = use_signal(|| false);
    let mut draft = use_signal(String::new);
    // The source files the agent reads. Uploads are shared by every
    // project, so the chips match the landing page.
    let mut uploads = use_signal(Vec::<api::UploadSummary>::new);
    let compose_effort = use_signal(|| "medium".to_owned());
    let mut pending_delete = use_signal(|| Option::<String>::None);
    // The screen each card shows, 1-based, by design id. Absent means 1.
    let mut card_screens = use_signal(HashMap::<String, usize>::new);
    let mut rename_draft = use_signal(|| Option::<String>::None);
    let mut error = use_signal(|| Option::<String>::None);
    let refresh_uploads = use_callback(move |_: ()| {
        spawn(async move {
            match api::fetch_uploads().await {
                Ok(listing) => uploads.set(listing),
                Err(message) => error.set(Some(message)),
            }
        });
    });

    // The live loop: refresh everything, then wait for the next change.
    use_future(move || async move {
        let mut seen = 0u64;
        loop {
            if let Ok(fetched) = api::fetch_brief().await {
                brief.set(fetched);
            }
            if let Ok(fetched) = api::fetch_questions().await {
                let key = fetched
                    .as_ref()
                    .map(|questions| {
                        questions
                            .iter()
                            .map(|question| question.question.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if key != questions_key() {
                    chosen.set(vec![
                        Vec::new();
                        fetched.as_ref().map(Vec::len).unwrap_or_default()
                    ]);
                    questions_key.set(key);
                }
                open_questions.set(fetched);
            }
            if let Ok(fetched) = api::fetch_design_list().await {
                designs.set(fetched);
            }
            if let Ok(fetched) = api::fetch_agent_run().await {
                agent_run.set(Some(fetched));
            }
            if let Ok(fetched) = api::fetch_settings().await {
                settings.set(Some(fetched));
            }
            if let Ok(fetched) = api::fetch_uploads().await {
                uploads.set(fetched);
            }
            is_loaded.set(true);
            match api::wait_for_change(seen).await {
                Ok(current) => {
                    seen = current;
                    revision.set(current);
                }
                Err(_) => pause_briefly().await,
            }
        }
    });

    // One send path: the first message writes the brief for this
    // project; later ones append to the conversation. Either way the
    // engine takes a turn.
    // Follow the conversation: jump to the newest message on load and
    // whenever a message arrives.
    let mut followed_count = use_signal(|| usize::MAX);
    use_effect(move || {
        let count = brief().map(|current| current.messages.len()).unwrap_or(0);
        if count != *followed_count.peek() {
            followed_count.set(count);
            document::eval(SCROLL_TO_LATEST);
        }
    });

    let send = use_callback({
        let project = project.clone();
        move |_: ()| {
            let text = draft().trim().to_owned();
            if text.is_empty() {
                return;
            }
            let project = project.clone();
            let is_new_brief = !brief()
                .is_some_and(|current| current.project.as_deref() == Some(project.as_str()));
            let effort = compose_effort();
            draft.set(String::new());
            spawn(async move {
                let result = if is_new_brief {
                    let request = api::BriefRequest {
                        prompt: &text,
                        project: Some(&project),
                        effort: &effort,
                        preview: true,
                        templates: &[],
                    };
                    api::save_brief(&request).await
                } else {
                    api::send_message(&text, None).await
                };
                match result {
                    Ok(()) => {
                        error.set(None);
                        // Start the engine; one that is already active picks
                        // the message up at the end of its turn.
                        if let Err(message) = api::start_agent_run().await
                            && !message.contains("already active")
                        {
                            error.set(Some(message));
                        }
                    }
                    Err(message) => error.set(Some(message)),
                }
            });
        }
    });

    let commit_rename = use_callback({
        let project = project.clone();
        move |_: ()| {
            let Some(new_name) = rename_draft() else {
                return;
            };
            let new_name = new_name.trim().to_owned();
            let project = project.clone();
            if new_name.is_empty() || new_name == project {
                rename_draft.set(None);
                return;
            }
            spawn(async move {
                match api::rename_project(&project, &new_name).await {
                    Ok(renamed) => {
                        rename_draft.set(None);
                        error.set(None);
                        on_rename.call(renamed);
                    }
                    Err(message) => error.set(Some(message)),
                }
            });
        }
    });

    // Open the setup panel once when no model is chosen yet. The user
    // can close it and come back through the model chip.
    let mut has_auto_opened_setup = use_signal(|| false);
    use_effect(move || {
        if let Some(view) = settings()
            && view.current.is_none()
            && !has_auto_opened_setup()
        {
            has_auto_opened_setup.set(true);
            is_configuring.set(true);
        }
    });
    // The top bar shows the run state while this studio is open, so a
    // live run stays visible when the thread scrolls away.
    let mut topbar = use_context::<Signal<Option<TopbarContext>>>();
    use_effect(move || {
        let context = agent_run()
            .filter(|run| run.is_running)
            .map(|run| TopbarContext {
                label: working_label(&run),
                model: settings()
                    .and_then(|view| view.current)
                    .map(|current| format!("{}/{}", current.provider, current.model)),
            });
        topbar.set(context);
    });
    use_drop(move || topbar.set(None));

    // Asks the engine for the remaining screens of a preview design, then
    // starts a run to write them.
    let continue_design = use_callback(move |id: String| {
        spawn(async move {
            match api::continue_design(&id).await {
                Ok(()) => {
                    error.set(None);
                    if let Err(message) = api::start_agent_run().await
                        && !message.contains("already active")
                    {
                        error.set(Some(message));
                    }
                }
                Err(message) => error.set(Some(message)),
            }
        });
    });
    let is_running = agent_run().is_some_and(|run| run.is_running);

    let send_answers = move |_| {
        let Some(questions) = open_questions() else {
            return;
        };
        let answers: Vec<(String, String)> = questions
            .iter()
            .enumerate()
            .map(|(index, question)| {
                let selected = chosen().get(index).cloned().unwrap_or_default();
                let answer = if selected.is_empty() {
                    "You decide".to_owned()
                } else {
                    selected.join(", ")
                };
                (question.question.clone(), answer)
            })
            .collect();
        spawn(async move {
            match api::send_answers(answers).await {
                Ok(()) => {
                    error.set(None);
                    if let Err(message) = api::start_agent_run().await
                        && !message.contains("already active")
                    {
                        error.set(Some(message));
                    }
                }
                Err(message) => error.set(Some(message)),
            }
        });
    };

    // Only this project's designs appear on the canvas, and the global
    // brief only shows when it belongs to this project.
    let mut members: Vec<api::DesignSummary> = designs()
        .iter()
        .filter(|design| design_project(&design.id) == project)
        .cloned()
        .collect();
    // The chosen design leads; candidates follow in order.
    members.sort_by_key(|design| design.id.contains("-candidate-"));
    // Per-design progress of the active run, and placeholder cards for
    // candidates the run is still writing.
    let run_designs: HashMap<String, u8> = agent_run()
        .filter(|run| run.is_running)
        .map(|run| run.designs)
        .unwrap_or_default();
    let mut pending_cards: Vec<(String, u8)> = run_designs
        .iter()
        .filter(|(id, _)| {
            design_project(id) == project && !members.iter().any(|member| &member.id == *id)
        })
        .map(|(id, percent)| (id.clone(), *percent))
        .collect();
    pending_cards.sort();
    let is_canvas_empty = members.is_empty() && pending_cards.is_empty();
    let is_brief_for_project =
        brief().is_some_and(|current| current.project.as_deref() == Some(project.as_str()));
    let current_settings = settings().and_then(|view| view.current);
    let is_model_chosen = current_settings.is_some();
    let show_setup = is_loaded() && is_configuring();
    let can_send = is_model_chosen && !draft().trim().is_empty();

    rsx! {
        main { class: "studio",
            section { class: "conversation",
                div { class: "studio-head",
                    button { class: "back", onclick: move |_| on_home.call(()),
                        span { dangerous_inner_html: icons::CHEVRON_LEFT }
                        "Projects"
                    }
                    span { class: "divider" }
                    if let Some(value) = rename_draft() {
                        input {
                            class: "rename-input mono",
                            value: "{value}",
                            autofocus: true,
                            oninput: move |event| rename_draft.set(Some(event.value())),
                            onkeydown: move |event: Event<KeyboardData>| {
                                if event.key() == Key::Enter {
                                    event.prevent_default();
                                    commit_rename.call(());
                                } else if event.key() == Key::Escape {
                                    rename_draft.set(None);
                                }
                            },
                        }
                        button {
                            class: "icon-button",
                            title: "Save the name",
                            onclick: move |_| commit_rename.call(()),
                            span { dangerous_inner_html: icons::CHECK }
                        }
                        button {
                            class: "icon-button",
                            title: "Cancel",
                            onclick: move |_| rename_draft.set(None),
                            "×"
                        }
                    } else {
                        span { class: "kicker", "{project}" }
                        button {
                            class: "rename",
                            title: "Rename project",
                            onclick: {
                                let project = project.clone();
                                move |_| rename_draft.set(Some(project.clone()))
                            },
                            span { dangerous_inner_html: icons::PENCIL }
                        }
                    }
                }
                div { class: "thread",
                    if !is_loaded() {
                        p { "Loading…" }
                    }
                    if let Some(current_brief) = brief() {
                        if is_brief_for_project {
                            div { class: "brief-summary",
                                p { class: "brief-summary-title", "{current_brief.prompt}" }
                                div { class: "brief-tags",
                                    for tag in brief_tags(&current_brief) {
                                        span { class: "badge", "{tag}" }
                                    }
                                }
                            }
                            for (index, message) in current_brief.messages.iter().enumerate() {
                                div {
                                    key: "{index}",
                                    class: if message.role == "user" { "bubble user" } else { "bubble agent" },
                                    p { "{message.content}" }
                                }
                            }
                        }
                    }
                    if let Some(run) = agent_run() {
                        RunStatusCard { run }
                    }
                    if let Some(message) = error() {
                        p { class: "error", "{message}" }
                    }
                }
                if show_setup {
                    div { class: "chat-settings",
                        SettingsPanel { settings, is_configuring }
                    }
                }
                div { class: "chat-box",
                    textarea {
                        placeholder: if is_brief_for_project { "Reply, or ask for changes…" } else { "Describe the design: subject, audience, tone…" },
                        value: "{draft()}",
                        oninput: move |event| draft.set(event.value()),
                        onkeydown: move |event: Event<KeyboardData>| {
                            if event.key() == Key::Enter && !event.modifiers().shift() {
                                event.prevent_default();
                                send.call(());
                            }
                        },
                    }
                    AttachmentChips {
                        uploads: uploads(),
                        on_changed: move |_| refresh_uploads.call(()),
                        on_error: move |message| error.set(Some(message)),
                    }
                    div { class: "chat-box-row",
                        div { class: "chat-box-left",
                            AttachButton {
                                on_uploaded: move |_| {
                                    error.set(None);
                                    refresh_uploads.call(());
                                },
                                on_error: move |message| error.set(Some(message)),
                            }
                        }
                        div { class: "chat-box-right",
                            ModelChip {
                                settings,
                                is_configuring,
                                effort: (!is_brief_for_project).then_some(compose_effort),
                            }
                            SendButton {
                                label: "Send",
                                is_enabled: can_send,
                                on_send: move |_| send.call(()),
                            }
                        }
                    }
                }
            }
            section { class: "canvas",
                if let Some(questions) = open_questions() {
                    div { class: "question-panel",
                        h3 { class: "canvas-heading", "A few questions first" }
                        div { class: "question-grid",
                            for (question_index, question) in questions.iter().cloned().enumerate() {
                                div {
                                    class: if question.options.len() > 6 { "question-card wide" } else { "question-card" },
                                    key: "{question.question}",
                                    div { class: "question-row",
                                        span { class: "question-number",
                                            {format!("{:02}", question_index + 1)}
                                        }
                                        span { class: "question-text", "{question.question}" }
                                    }
                                    div { class: "option-chips",
                                        for option in question.options.iter().cloned().chain(["You decide".to_owned()]) {
                                            button {
                                                key: "{option}",
                                                class: if chosen().get(question_index).is_some_and(|selected| selected.contains(&option)) { "option-chip selected" } else { "option-chip" },
                                                onclick: {
                                                    let option = option.clone();
                                                    move |_| {
                                                        chosen
                                                            .with_mut(|chosen| {
                                                                if let Some(selected) = chosen.get_mut(question_index) {
                                                                    toggle_option(selected, &option);
                                                                }
                                                            })
                                                    }
                                                },
                                                "{option}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        p { class: "question-hint",
                            "Pick any that apply, or type a reply in the chat."
                        }
                        button { class: "primary", onclick: send_answers, "Send answers" }
                    }
                }
                if !is_canvas_empty {
                    h3 { class: "canvas-heading",
                        "Project · {project}"
                        span { class: "count-pill", "{members.len() + pending_cards.len()}" }
                    }
                    div { class: "canvas-grid",
                        for member in members {
                            article {
                                class: "canvas-card",
                                key: "{member.id}",
                                title: "Click to open in the editor",
                                onclick: {
                                    let id = member.id.clone();
                                    move |_| {
                                        pending_delete.set(None);
                                        on_open.call(id.clone());
                                    }
                                },
                                button {
                                    class: if pending_delete().as_deref() == Some(member.id.as_str()) { "card-delete confirm" } else { "card-delete" },
                                    title: "Delete this design",
                                    onclick: {
                                        let id = member.id.clone();
                                        move |event: Event<MouseData>| {
                                            event.stop_propagation();
                                            if pending_delete().as_deref() == Some(id.as_str()) {
                                                pending_delete.set(None);
                                                let id = id.clone();
                                                spawn(async move {
                                                    let _ = api::delete_design(&id).await;
                                                });
                                            } else {
                                                pending_delete.set(Some(id.clone()));
                                            }
                                        }
                                    },
                                    if pending_delete().as_deref() == Some(member.id.as_str()) {
                                        "Delete?"
                                    } else {
                                        "×"
                                    }
                                }
                                {
                                    let shown = card_screens()
                                        .get(&member.id)
                                        .copied()
                                        .unwrap_or(1)
                                        .clamp(1, member.screen_count.max(1));
                                    let count = member.screen_count.max(1);
                                    let previous_id = member.id.clone();
                                    let next_id = member.id.clone();
                                    rsx! {
                                        div { class: "card-preview",
                                            if let Some(percent) = run_designs.get(&member.id) {
                                                div { class: "card-progress", title: "Writing… {percent}%",
                                                    div { class: "card-progress-fill", style: "width: {percent}%" }
                                                }
                                            }
                                            iframe {
                                                title: "{member.id}",
                                                src: "/designs/{member.id}/render?v={revision()}&screen={shown}",
                                            }
                                            if count > 1 {
                                                div { class: "card-pager", title: "Screen {shown} of {count}",
                                                    button {
                                                        disabled: shown <= 1,
                                                        title: "Previous screen",
                                                        onclick: move |event: Event<MouseData>| {
                                                            event.stop_propagation();
                                                            card_screens
                                                                .with_mut(|screens| {
                                                                    screens.insert(previous_id.clone(), stepped_screen(shown, -1, count));
                                                                });
                                                        },
                                                        "←"
                                                    }
                                                    button {
                                                        disabled: shown >= count,
                                                        title: "Next screen",
                                                        onclick: move |event: Event<MouseData>| {
                                                            event.stop_propagation();
                                                            card_screens
                                                                .with_mut(|screens| {
                                                                    screens.insert(next_id.clone(), stepped_screen(shown, 1, count));
                                                                });
                                                        },
                                                        "→"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                div { class: "card-footer",
                                    div { class: "card-text",
                                        span {
                                            class: "card-title",
                                            title: "{member.title}",
                                            "{member.title}"
                                        }
                                        if let Some(percent) = run_designs.get(&member.id) {
                                            span { class: "card-label card-progress-label",
                                                "writing… {percent}%"
                                            }
                                        } else {
                                            span { class: "card-label", "{card_label(&member)}" }
                                        }
                                    }
                                    if !run_designs.contains_key(&member.id) && member.is_unfinished() {
                                        {
                                            let is_continuing = is_running
                                                && brief()
                                                    .as_ref()
                                                    .is_some_and(|current| {
                                                        is_continue_pending(&current.messages, &member.id)
                                                    });
                                            rsx! {
                                                button {
                                                    class: "card-continue",
                                                    title: "Write the remaining screens of this design",
                                                    disabled: is_continuing,
                                                    onclick: {
                                                        let id = member.id.clone();
                                                        move |event: Event<MouseData>| {
                                                            event.stop_propagation();
                                                            continue_design.call(id.clone());
                                                        }
                                                    },
                                                    if is_continuing {
                                                        "Continuing…"
                                                    } else {
                                                        "Continue"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        for (id, percent) in pending_cards {
                            article { class: "canvas-card placeholder", key: "{id}",
                                div { class: "card-preview card-placeholder",
                                    div {
                                        class: "card-progress",
                                        title: "Writing… {percent}%",
                                        div {
                                            class: "card-progress-fill",
                                            style: "width: {percent}%",
                                        }
                                    }
                                    span { class: "card-placeholder-text", "Writing… {percent}%" }
                                }
                                div { class: "card-footer",
                                    div { class: "card-text",
                                        span { class: "card-title", "{candidate_name(&id)}" }
                                        span { class: "card-label card-progress-label",
                                            "writing… {percent}%"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if is_canvas_empty && open_questions().is_none() {
                    div { class: "canvas-empty",
                        span { class: "kicker", "{project}" }
                        h1 { "Designs appear here." }
                        p { class: "lede",
                            "Your agent writes candidates from the brief on the left. "
                            "Each one shows up the moment it is saved."
                        }
                    }
                }
            }
        }
    }
}

/// One row of the model list: the catalog entry, plus whether the key
/// can reach the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelRow {
    /// Model id sent to the provider.
    pub id: String,
    /// One short line that tells the user when to pick this model.
    /// Empty for a model the catalog does not list.
    pub description: String,
    /// True for the model the panel selects first.
    pub is_recommended: bool,
    /// True when the live model list holds this id, or when no live
    /// list exists.
    pub is_available: bool,
}

/// Builds the model rows from the catalog and the live model list.
///
/// An empty live list means the fetch did not run, so every catalog
/// model stays available: the panel must not claim a fact it does not
/// have. Order is the recommended model, then catalog order, then the
/// live models the catalog omits, then every unavailable model.
pub(crate) fn model_rows(catalog: &[api::CatalogModel], live: &[String]) -> Vec<ModelRow> {
    let has_live_list = !live.is_empty();
    let mut rows: Vec<ModelRow> = catalog
        .iter()
        .map(|model| ModelRow {
            id: model.id.clone(),
            description: model.description.clone(),
            is_recommended: model.is_recommended,
            is_available: !has_live_list || live.contains(&model.id),
        })
        .collect();
    for id in live {
        if catalog.iter().any(|model| &model.id == id) {
            continue;
        }
        rows.push(ModelRow {
            id: id.clone(),
            description: String::new(),
            is_recommended: false,
            is_available: true,
        });
    }
    // Sort keys only, so catalog order survives inside each group.
    rows.sort_by_key(|row| (!row.is_available, !row.is_recommended));
    rows
}

/// The model the panel selects first: the recommended one when the key
/// can reach it, else the first row it can reach.
fn first_choice(rows: &[ModelRow]) -> Option<String> {
    rows.iter()
        .find(|row| row.is_available)
        .map(|row| row.id.clone())
}

/// Every signal the three setup steps share.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SetupState {
    /// Which step the panel shows: 1, 2, or 3.
    step: Signal<u8>,
    /// The chosen provider name.
    provider_name: Signal<String>,
    /// The API key the user typed.
    api_key: Signal<String>,
    /// True while the key field shows the key as plain text.
    is_key_shown: Signal<bool>,
    /// True once the provider answered a request made with the key.
    is_key_verified: Signal<bool>,
    /// The chosen model id.
    model: Signal<String>,
    /// A model id typed by hand. It overrides the list.
    custom_model: Signal<String>,
    /// True while the custom model field is open.
    is_custom_model_open: Signal<bool>,
    /// The model ids the provider returned. Empty before a fetch.
    loaded_models: Signal<Vec<String>>,
    /// True once the model step asked the provider for its models.
    has_tried_loading_models: Signal<bool>,
    /// The login page URL, once a login started.
    login_url: Signal<Option<String>>,
    /// The code the user pasted back from the login page.
    login_code: Signal<String>,
    /// The error to show under the form.
    message: Signal<Option<String>>,
    /// True while a request is in flight.
    is_busy: Signal<bool>,
    /// True when the user pressed Next and the panel owes them the
    /// model step once the running key check succeeds.
    is_advance_requested: Signal<bool>,
}

impl SetupState {
    /// Which step the panel shows.
    fn step(&self) -> u8 {
        (self.step)()
    }

    /// The chosen provider name.
    fn provider_name(&self) -> String {
        (self.provider_name)()
    }

    /// The API key the user typed.
    fn api_key(&self) -> String {
        (self.api_key)()
    }

    /// True while the key field shows the key as plain text.
    fn is_key_shown(&self) -> bool {
        (self.is_key_shown)()
    }

    /// True once the provider answered a request made with the key.
    fn is_key_verified(&self) -> bool {
        (self.is_key_verified)()
    }

    /// The chosen model id.
    fn model(&self) -> String {
        (self.model)()
    }

    /// A model id typed by hand.
    fn custom_model(&self) -> String {
        (self.custom_model)()
    }

    /// True while the custom model field is open.
    fn is_custom_model_open(&self) -> bool {
        (self.is_custom_model_open)()
    }

    /// The model ids the provider returned.
    fn loaded_models(&self) -> Vec<String> {
        (self.loaded_models)()
    }

    /// True once the model step asked the provider for its models.
    fn has_tried_loading_models(&self) -> bool {
        (self.has_tried_loading_models)()
    }

    /// The login page URL, once a login started.
    fn login_url(&self) -> Option<String> {
        (self.login_url)()
    }

    /// The code the user pasted back from the login page.
    fn login_code(&self) -> String {
        (self.login_code)()
    }

    /// The error to show under the form.
    fn message(&self) -> Option<String> {
        (self.message)()
    }

    /// True while a request is in flight.
    fn is_busy(&self) -> bool {
        (self.is_busy)()
    }

    /// True when the user pressed Next during a running key check.
    fn is_advance_requested(&self) -> bool {
        (self.is_advance_requested)()
    }

    /// Creates the signals. Call it from the panel body only: it
    /// calls `use_signal`, so it obeys the hook rules.
    fn new() -> Self {
        Self {
            step: use_signal(|| 1u8),
            provider_name: use_signal(|| "google".to_owned()),
            api_key: use_signal(String::new),
            is_key_shown: use_signal(|| false),
            is_key_verified: use_signal(|| false),
            model: use_signal(String::new),
            custom_model: use_signal(String::new),
            is_custom_model_open: use_signal(|| false),
            loaded_models: use_signal(Vec::<String>::new),
            has_tried_loading_models: use_signal(|| false),
            login_url: use_signal(|| Option::<String>::None),
            login_code: use_signal(String::new),
            message: use_signal(|| Option::<String>::None),
            is_busy: use_signal(|| false),
            is_advance_requested: use_signal(|| false),
        }
    }

    /// Clears everything the old provider decided.
    fn reset_for_new_provider(&mut self, name: String) {
        self.provider_name.set(name);
        self.api_key.set(String::new());
        self.is_key_shown.set(false);
        self.is_key_verified.set(false);
        self.model.set(String::new());
        self.custom_model.set(String::new());
        self.is_custom_model_open.set(false);
        self.loaded_models.set(Vec::new());
        self.has_tried_loading_models.set(false);
        self.login_url.set(None);
        self.message.set(None);
        self.is_advance_requested.set(false);
    }
}

/// The model picker, as three steps: provider, access, model.
#[component]
pub(crate) fn SettingsPanel(
    settings: Signal<Option<api::SettingsView>>,
    is_configuring: Signal<bool>,
) -> Element {
    let mut state = SetupState::new();

    let providers = settings().map(|view| view.providers).unwrap_or_default();
    let Some(provider) = providers
        .iter()
        .find(|provider| provider.name == state.provider_name())
        .cloned()
        .or_else(|| providers.first().cloned())
    else {
        return rsx! {
            p { "Loading…" }
        };
    };

    // A finished login stores credentials server-side and /events
    // refreshes the settings; move on to the model step.
    use_effect(move || {
        let current = settings().and_then(|view| view.current);
        if state.step() == 2
            && state.login_url().is_some()
            && current.is_some_and(|current| {
                current.provider == state.provider_name() && current.auth != "none"
            })
        {
            state.login_url.set(None);
            state.is_key_verified.set(true);
            state.step.set(3);
        }
    });

    // Entering the model step without a verified key still needs the
    // list: a provider that needs no key, or one with saved
    // credentials, never passes through the key field.
    use_effect(move || {
        if state.step() == 3 && !state.has_tried_loading_models() {
            state.has_tried_loading_models.set(true);
            let provider = state.provider_name();
            let key = state.api_key();
            spawn(async move {
                let key = (!key.trim().is_empty()).then_some(key);
                match api::fetch_provider_models(&provider, key.as_deref()).await {
                    Ok(models) => {
                        state.loaded_models.set(models);
                        state.message.set(None);
                    }
                    Err(text) => state.message.set(Some(text)),
                }
            });
        }
    });

    let rows = model_rows(&provider.models, &state.loaded_models());
    if state.model().is_empty()
        && let Some(first) = first_choice(&rows)
    {
        state.model.set(first);
    }

    rsx! {
        div { class: "settings-panel",
            div { class: "settings-head",
                span { class: "kicker", "Set up" }
                SetupStepRail { step: state.step() }
                button {
                    class: "icon-button",
                    title: "Close",
                    onclick: move |_| is_configuring.set(false),
                    "×"
                }
            }
            div { class: "settings-form",
                if state.step() == 1 {
                    ProviderStep {
                        state,
                        providers: providers.clone(),
                        provider: provider.clone(),
                        settings,
                    }
                } else if state.step() == 2 {
                    AccessStep { state, provider: provider.clone() }
                } else {
                    ModelStep {
                        state,
                        provider: provider.clone(),
                        rows,
                        settings,
                        is_configuring,
                    }
                }
                if let Some(text) = state.message() {
                    p { class: "error", "{text}" }
                }
            }
        }
    }
}

/// The three step names, with a tick on every finished step.
#[component]
fn SetupStepRail(step: u8) -> Element {
    rsx! {
        div { class: "step-rail",
            for (number, name) in [(1u8, "Provider"), (2, "Access"), (3, "Model")] {
                if number > 1 {
                    span { class: "sep" }
                }
                span { class: if step == number { "step current" } else if step > number { "step done" } else { "step" },
                    span { class: "n",
                        if step > number {
                            "✓"
                        } else {
                            "{number}"
                        }
                    }
                    "{name}"
                }
            }
        }
    }
}

/// Step 1: pick the provider.
#[component]
fn ProviderStep(
    state: SetupState,
    providers: Vec<api::CatalogProvider>,
    provider: api::CatalogProvider,
    settings: Signal<Option<api::SettingsView>>,
) -> Element {
    let mut state = state;

    // A saved login or key for this provider skips the access step:
    // the model step loads the list with the stored credentials.
    let has_saved_credentials = settings()
        .and_then(|view| view.current)
        .is_some_and(|current| current.provider == provider.name && current.auth != "none");
    let needs_access_step = provider.needs_api_key && !has_saved_credentials;

    rsx! {
        div { class: "field provider-field",
            span { class: "field-label", "Provider" }
            Select {
                value: provider.name.clone(),
                options: providers
                    .iter()
                    .map(|entry| (entry.name.clone(), entry.label.clone()))
                    .collect::<Vec<_>>(),
                on_change: move |name| state.reset_for_new_provider(name),
            }
        }
        p { class: "agent-log", "runs on your own account · nothing leaves this machine" }
        div { class: "settings-actions",
            button {
                class: "primary",
                onclick: move |_| {
                    state.message.set(None);
                    state.step.set(if needs_access_step { 2 } else { 3 });
                },
                "Next"
            }
            if !provider.needs_api_key {
                span { class: "agent-log", "{provider.label} needs no sign-in." }
            }
            span { class: "step-count", "Step 1 of 3" }
        }
    }
}

/// Step 2: sign in, or paste an API key.
#[component]
fn AccessStep(state: SetupState, provider: api::CatalogProvider) -> Element {
    let mut state = state;
    let is_openrouter = provider.name == "openrouter";
    let is_openai = provider.name == "openai";
    let uses_callback_login = is_openrouter || is_openai;

    let get_login_link = move |_| {
        spawn(async move {
            let started = if is_openrouter {
                api::start_openrouter_login().await
            } else if is_openai {
                api::start_openai_login().await
            } else {
                api::start_login().await
            };
            match started {
                Ok(url) => {
                    state.login_url.set(Some(url));
                    state.message.set(None);
                }
                Err(text) => state.message.set(Some(text)),
            }
        });
    };

    let finish_login = move |_| {
        let code = state.login_code();
        spawn(async move {
            match api::complete_login(&code, None).await {
                Ok(()) => {
                    state.login_url.set(None);
                    state.login_code.set(String::new());
                    state.message.set(None);
                    state.is_key_verified.set(true);
                    state.step.set(3);
                }
                Err(text) => state.message.set(Some(text)),
            }
        });
    };

    // The server has no verify route. Asking the provider for its model
    // list is the one round trip that proves the key works, and the
    // model step needs that list anyway. `should_advance` is false for
    // the blur check, which only reports whether the key works.
    let verify_key = use_callback(move |should_advance: bool| {
        if state.api_key().trim().is_empty() {
            if should_advance {
                state
                    .message
                    .set(Some("enter the API key first".to_owned()));
            }
            return;
        }
        if state.is_key_verified() {
            if should_advance {
                state.step.set(3);
            }
            return;
        }
        if should_advance {
            state.is_advance_requested.set(true);
        }
        // Leaving the field starts a check of its own, so a click on
        // Next lands while that check runs. Let the running check carry
        // the request instead of starting a second one.
        if state.is_busy() {
            return;
        }
        state.message.set(None);
        state.is_busy.set(true);
        let provider = state.provider_name();
        let key = state.api_key();
        spawn(async move {
            match api::fetch_provider_models(&provider, Some(&key)).await {
                Ok(models) => {
                    state.loaded_models.set(models);
                    state.has_tried_loading_models.set(true);
                    state.is_key_verified.set(true);
                    state.model.set(String::new());
                    if state.is_advance_requested() {
                        state.step.set(3);
                    }
                }
                Err(text) => {
                    state.is_key_verified.set(false);
                    state.message.set(Some(text));
                }
            }
            state.is_advance_requested.set(false);
            state.is_busy.set(false);
        });
    });

    rsx! {
        p { class: "provider-name", "{provider.label}" }
        if provider.supports_login {
            div { class: "settings-actions",
                button { class: "primary", onclick: get_login_link,
                    if is_openrouter {
                        "Log in with OpenRouter"
                    } else if is_openai {
                        "Log in with ChatGPT"
                    } else {
                        "Log in with Claude"
                    }
                }
            }
            if let Some(url) = state.login_url() {
                div { class: "settings-login",
                    a { class: "button", href: "{url}", target: "_blank", "Open the login page" }
                    if uses_callback_login {
                        p { class: "agent-log",
                            "Finish in the new tab; this page moves on by itself."
                        }
                    } else {
                        label {
                            "Paste the code the page shows"
                            input {
                                value: "{state.login_code()}",
                                oninput: move |event| state.login_code.set(event.value()),
                            }
                        }
                        button { class: "primary", onclick: finish_login, "Complete login" }
                    }
                }
            }
            div { class: "settings-divider", "or use an API key" }
        }
        div { class: "field",
            span { class: "field-label", "API key" }
            div { class: "key-field",
                input {
                    r#type: if state.is_key_shown() { "text" } else { "password" },
                    placeholder: "paste your {provider.label} API key",
                    value: "{state.api_key()}",
                    oninput: move |event| {
                        state.api_key.set(event.value());
                        state.is_key_verified.set(false);
                    },
                    onblur: move |_| verify_key.call(false),
                }
                button {
                    class: "link-button",
                    onclick: move |_| state.is_key_shown.toggle(),
                    if state.is_key_shown() {
                        "Hide"
                    } else {
                        "Show"
                    }
                }
            }
        }
        if state.is_key_verified() {
            p { class: "key-status", "✓ key verified · stored on this machine" }
        }
        p { class: "lede",
            "Swift Design calls the provider directly from this machine. The key stays "
            "in a local settings file that git ignores. It goes to no other service."
        }
        div { class: "settings-actions",
            button { class: "primary", onclick: move |_| verify_key.call(true),
                if state.is_busy() {
                    "Checking…"
                } else {
                    "Next"
                }
            }
            button {
                onclick: move |_| {
                    state.message.set(None);
                    state.step.set(1);
                },
                "Back"
            }
            span { class: "step-count", "Step 2 of 3" }
        }
    }
}

/// Step 3: pick the model, then save.
#[component]
fn ModelStep(
    state: SetupState,
    provider: api::CatalogProvider,
    rows: Vec<ModelRow>,
    settings: Signal<Option<api::SettingsView>>,
    is_configuring: Signal<bool>,
) -> Element {
    let mut state = state;
    let live_count = state.loaded_models().len();

    let has_saved_credentials = settings()
        .and_then(|view| view.current)
        .is_some_and(|current| current.provider == provider.name && current.auth != "none");
    let needs_access_step = provider.needs_api_key && !has_saved_credentials;

    let provider_for_save = provider.name.clone();
    let save = move |_| {
        let provider = provider_for_save.clone();
        let custom = state.custom_model();
        let model = if custom.trim().is_empty() {
            state.model()
        } else {
            custom.trim().to_owned()
        };
        let key = state.api_key();
        state.is_busy.set(true);
        spawn(async move {
            let key = (!key.trim().is_empty()).then_some(key);
            match api::save_settings(&provider, &model, key.as_deref()).await {
                Ok(()) => {
                    is_configuring.set(false);
                    state.message.set(None);
                }
                Err(text) => state.message.set(Some(text)),
            }
            state.is_busy.set(false);
        });
    };

    rsx! {
        div { class: "field",
            div { class: "field-heading",
                span { "Model" }
                span { class: "model-count",
                    if live_count > 0 {
                        "{live_count} available on this key"
                    } else {
                        "curated list"
                    }
                    button {
                        class: "link-button",
                        title: "Reload the model list from {provider.label}",
                        onclick: move |_| state.has_tried_loading_models.set(false),
                        "Reload"
                    }
                }
            }
            div { class: "model-list", role: "radiogroup",
                for row in rows.iter().cloned() {
                    button {
                        key: "{row.id}",
                        class: if state.model() == row.id { "model-option selected" } else { "model-option" },
                        role: "radio",
                        aria_checked: state.model() == row.id,
                        disabled: !row.is_available,
                        onclick: {
                            let id = row.id.clone();
                            move |_| state.model.set(id.clone())
                        },
                        span { class: "model-radio" }
                        span { class: "model-option-text",
                            span { class: "model-id", "{row.id}" }
                            if !row.is_available {
                                span { class: "model-desc", "Not enabled on this key." }
                            } else if !row.description.is_empty() {
                                span { class: "model-desc", "{row.description}" }
                            }
                        }
                        if row.is_recommended {
                            span { class: "badge", "Recommended" }
                        }
                    }
                }
            }
        }
        if state.is_custom_model_open() {
            label {
                "Custom model (overrides the list)"
                input {
                    value: "{state.custom_model()}",
                    oninput: move |event| state.custom_model.set(event.value()),
                }
            }
        } else {
            div {
                button {
                    class: "link-button",
                    onclick: move |_| state.is_custom_model_open.set(true),
                    "Use another model id"
                }
            }
        }
        div { class: "settings-actions",
            button { class: "primary", disabled: state.is_busy(), onclick: save,
                "Start using Swift Design"
            }
            button {
                onclick: move |_| {
                    state.message.set(None);
                    state.step.set(if needs_access_step { 2 } else { 1 });
                },
                "Back"
            }
            span { class: "step-count", "Step 3 of 3" }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::api::{Brief, CatalogModel, ChatMessage, DesignSummary};
    use crate::studio::{
        brief_tags, candidate_name, card_label, design_project, is_continue_pending, length_label,
        model_rows, project_groups, stepped_screen, toggle_option,
    };

    fn catalog() -> Vec<CatalogModel> {
        vec![
            CatalogModel {
                id: "flash".to_owned(),
                description: "Fast drafts and quick edits.".to_owned(),
                is_recommended: false,
            },
            CatalogModel {
                id: "pro".to_owned(),
                description: "Best structure and copy.".to_owned(),
                is_recommended: true,
            },
        ]
    }

    fn ids(rows: &[crate::studio::ModelRow]) -> Vec<&str> {
        rows.iter().map(|row| row.id.as_str()).collect()
    }

    fn message(role: &str, design: Option<&str>, action: Option<&str>) -> ChatMessage {
        ChatMessage {
            role: role.to_owned(),
            content: "x".to_owned(),
            design: design.map(str::to_owned),
            action: action.map(str::to_owned),
        }
    }

    #[test]
    fn continue_requests_stay_pending_until_a_plain_user_turn() {
        let messages = vec![
            message("user", Some("talk-candidate-1"), Some("continue")),
            message("assistant", None, None),
            message("user", Some("talk-candidate-2"), Some("continue")),
        ];
        assert!(is_continue_pending(&messages, "talk-candidate-1"));
        assert!(is_continue_pending(&messages, "talk-candidate-2"));
        assert!(!is_continue_pending(&messages, "talk-candidate-3"));
        let mut ended = messages.clone();
        ended.push(message("user", None, None));
        assert!(!is_continue_pending(&ended, "talk-candidate-2"));
    }

    #[test]
    fn card_pager_steps_stay_inside_the_design() {
        assert_eq!(stepped_screen(1, 1, 8), 2);
        assert_eq!(stepped_screen(8, 1, 8), 8);
        assert_eq!(stepped_screen(1, -1, 8), 1);
        assert_eq!(stepped_screen(5, -1, 8), 4);
        assert_eq!(stepped_screen(3, 1, 0), 1);
    }

    fn brief_with_length(length: Option<&str>) -> Brief {
        Brief {
            prompt: "A talk.".to_owned(),
            scenario: None,
            length: length.map(str::to_owned),
            variations: None,
            project: None,
            effort: "medium".to_owned(),
            preview: true,
            variety: None,
            answers: Vec::new(),
            messages: Vec::new(),
        }
    }

    #[test]
    fn length_labels_name_the_screen_range() {
        assert_eq!(
            length_label(&brief_with_length(Some("10-15"))).as_deref(),
            Some("10–15 screens")
        );
        assert_eq!(
            length_label(&brief_with_length(Some("12-12"))).as_deref(),
            Some("12 screens")
        );
        assert_eq!(length_label(&brief_with_length(Some("any"))), None);
        assert_eq!(length_label(&brief_with_length(None)), None);
    }

    #[test]
    fn brief_tags_list_the_run_settings() {
        assert_eq!(
            brief_tags(&brief_with_length(None)),
            vec!["effort medium", "previews"]
        );
        let full = Brief {
            scenario: Some("pitch".to_owned()),
            variations: Some(2),
            variety: Some("high".to_owned()),
            preview: false,
            ..brief_with_length(Some("10-15"))
        };
        assert_eq!(
            brief_tags(&full),
            vec![
                "effort medium",
                "pitch",
                "10–15 screens",
                "2 variations",
                "variety high"
            ]
        );
    }

    fn summary(id: &str) -> DesignSummary {
        DesignSummary {
            pending_count: 0,
            id: id.to_owned(),
            title: "T".to_owned(),
            theme: "slate".to_owned(),
            screen_count: 3,
            outline_count: 0,
        }
    }

    #[test]
    fn designs_group_by_project() {
        let designs = [
            summary("talk-candidate-1"),
            summary("talk-candidate-2"),
            summary("other"),
        ];
        let groups = project_groups(&designs);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "talk");
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, "other");
    }

    #[test]
    fn card_labels_shorten_candidate_ids() {
        assert_eq!(
            card_label(&summary("talk-candidate-2")),
            "Candidate 2 · 3 screens"
        );
        assert_eq!(
            card_label(&summary("pitch-candidate-1-candidate-3")),
            "Candidate 3 · 3 screens"
        );
        assert_eq!(card_label(&summary("talk")), "Chosen · 3 screens");
        let preview = DesignSummary {
            outline_count: 12,
            ..summary("talk-candidate-1")
        };
        assert!(preview.is_preview());
        assert_eq!(
            card_label(&preview),
            "Candidate 1 · preview 3 of 12 screens"
        );
        let complete = DesignSummary {
            outline_count: 3,
            ..summary("talk")
        };
        assert!(!complete.is_preview());
        assert_eq!(card_label(&complete), "Chosen · 3 screens");
        assert_eq!(candidate_name("talk-candidate-3"), "Candidate 3");
        assert_eq!(candidate_name("talk"), "Chosen");
    }

    #[test]
    fn options_toggle_and_you_decide_stands_alone() {
        let mut selected = Vec::new();
        toggle_option(&mut selected, "Blue");
        toggle_option(&mut selected, "Red");
        assert_eq!(selected, vec!["Blue", "Red"]);
        toggle_option(&mut selected, "Blue");
        assert_eq!(selected, vec!["Red"]);
        toggle_option(&mut selected, "You decide");
        assert_eq!(selected, vec!["You decide"]);
        toggle_option(&mut selected, "Red");
        assert_eq!(selected, vec!["Red"]);
    }

    #[test]
    fn design_projects_strip_candidate_suffixes() {
        assert_eq!(design_project("talk-candidate-2"), "talk");
        assert_eq!(design_project("talk"), "talk");
    }

    #[test]
    fn puts_the_recommended_model_first() {
        let rows = model_rows(&catalog(), &["flash".to_owned(), "pro".to_owned()]);
        assert_eq!(ids(&rows), ["pro", "flash"]);
        assert!(rows.iter().all(|row| row.is_available));
    }

    #[test]
    fn marks_a_catalog_model_the_key_omits_as_unavailable() {
        let rows = model_rows(&catalog(), &["flash".to_owned()]);
        assert_eq!(ids(&rows), ["flash", "pro"]);
        assert!(rows[0].is_available);
        assert!(!rows[1].is_available);
    }

    #[test]
    fn appends_live_models_the_catalog_omits() {
        let rows = model_rows(
            &catalog(),
            &["flash".to_owned(), "pro".to_owned(), "nano".to_owned()],
        );
        assert_eq!(ids(&rows), ["pro", "flash", "nano"]);
        assert!(rows[2].description.is_empty());
    }

    #[test]
    fn treats_every_model_as_available_when_the_live_list_is_empty() {
        let rows = model_rows(&catalog(), &[]);
        assert_eq!(ids(&rows), ["pro", "flash"]);
        assert!(rows.iter().all(|row| row.is_available));
    }
}
