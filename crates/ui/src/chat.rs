//! The editor's chat column: the conversation, scoped to the open design.
//!
//! The user asks for changes in words. Each message carries the design id,
//! so the engine applies the request to that design. A context chip names
//! the node the user last clicked or right-clicked, and is prepended to
//! the next message as `[screen N, node a/b <tag.class>: text]`.

use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::{ModelChip, SendButton};
use crate::status::RunStatusCard;
use crate::studio::{SettingsPanel, pause_briefly};
use crate::uploads::{AttachButton, AttachmentChips};

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

/// The chat column for one design.
#[component]
pub fn DesignChat(
    design_id: String,
    /// The element reference to prepend to the next message.
    context: Signal<Option<String>>,
    /// Called before a message is sent, so the editor can save local
    /// edits first.
    on_before_send: EventHandler<()>,
    /// Called when a run finishes, so the editor can reload the design.
    on_run_finished: EventHandler<()>,
) -> Element {
    let mut brief = use_signal(|| Option::<api::Brief>::None);
    let mut agent_run = use_signal(|| Option::<api::AgentRun>::None);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let mut is_configuring = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut was_running = use_signal(|| false);
    // The source files the agent reads, attached from this chat box or
    // anywhere else: uploads are shared by every project.
    let mut uploads = use_signal(Vec::<api::UploadSummary>::new);
    let refresh_uploads = use_callback(move |_: ()| {
        spawn(async move {
            match api::fetch_uploads().await {
                Ok(listing) => uploads.set(listing),
                Err(message) => error.set(Some(message)),
            }
        });
    });

    // The live loop: refresh, then wait for the next change.
    use_future(move || async move {
        let mut seen = 0u64;
        loop {
            if let Ok(fetched) = api::fetch_brief().await {
                brief.set(fetched);
            }
            refresh_uploads.call(());
            if let Ok(fetched) = api::fetch_agent_run().await {
                if was_running() && !fetched.is_running {
                    on_run_finished.call(());
                }
                was_running.set(fetched.is_running);
                agent_run.set(Some(fetched));
            }
            if let Ok(fetched) = api::fetch_settings().await {
                settings.set(Some(fetched));
            }
            match api::wait_for_change(seen).await {
                Ok(current) => seen = current,
                Err(_) => pause_briefly().await,
            }
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
        let design_id = design_id.clone();
        move |_: ()| {
            let text = draft().trim().to_owned();
            if text.is_empty() {
                return;
            }
            let content = match context() {
                Some(reference) => format!("{reference} {text}"),
                None => text,
            };
            let design_id = design_id.clone();
            draft.set(String::new());
            context.set(None);
            on_before_send.call(());
            spawn(async move {
                match api::send_message(&content, Some(&design_id)).await {
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
        }
    });

    let current_settings = settings().and_then(|view| view.current);
    let is_model_chosen = current_settings.is_some();
    let show_setup = settings().is_some() && is_configuring();
    let is_running = agent_run().is_some_and(|run| run.is_running);
    let can_send = is_model_chosen && !draft().trim().is_empty();

    rsx! {
        section { class: "conversation editor-chat",
            div { class: "thread",
                if brief().is_none() {
                    p { class: "chat-note",
                        "No conversation yet. Ask for a change below, for example "
                        "“make the title on screen 2 bigger”."
                    }
                }
                if let Some(current_brief) = brief() {
                    for (index, message) in current_brief.messages.iter().enumerate() {
                        div {
                            key: "{index}",
                            class: if message.role == "user" { "bubble user" } else { "bubble agent" },
                            p { "{message.content}" }
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
                if let Some(reference) = context() {
                    div { class: "context-chip mono",
                        span { "{reference}" }
                        button {
                            title: "Drop this reference",
                            onclick: move |_| context.set(None),
                            "×"
                        }
                    }
                }
                textarea {
                    placeholder: if is_running { "Working… your next request queues up." } else { "Ask for a change: “make this title bigger”, “swap screens 2 and 4”…" },
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
                        ModelChip { settings, is_configuring }
                        SendButton {
                            label: "Send",
                            is_enabled: can_send,
                            on_send: move |_| send.call(()),
                        }
                    }
                }
            }
        }
    }
}
