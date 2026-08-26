//! The design editor: a chat column, a live preview with a right-click
//! menu, thumbnails, and a properties sheet for the rare manual edit.
//!
//! The main path is the chat: the user asks for changes and the engine
//! rewrites the design. The preview iframe renders the screen in editable
//! mode: clicking a node selects it and puts a reference in the chat,
//! text is editable in place, and a right-click menu applies quick
//! actions in the iframe's DOM. After every change the iframe posts the
//! screen's HTML back, and this editor stores it as `screens[i].html` and
//! saves. No HTML is parsed or mutated in WASM. Field badges show who
//! wrote the title, theme, css, and notes: the agent, or the user.

use design_model::transition::MAX_TRANSITION_MS;
use design_model::{Design, Screen, Transition, TransitionAxis, TransitionEffect};
use dioxus::document;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::chat::DesignChat;
use crate::icons;
use crate::select::{Select, plain_options};
use crate::uploads::format_size;

/// Forwards preview-iframe messages into the Dioxus event loop, turns
/// arrow keys on the editor page into navigate messages, and tracks
/// thumbnail drags. Same-origin messages only.
///
/// The drag runs in JS with `elementFromPoint` because Safari does not
/// fire `pointerenter` on other elements while a mouse button is held.
const PREVIEW_LISTENER: &str = "\
window.addEventListener('message', (event) => {
  if (event.origin !== window.location.origin) { return; }
  const data = event.data;
  if (data && (data.type === 'swift-design-html' || data.type === 'swift-design-select'
      || data.type === 'swift-design-action' || data.type === 'swift-design-navigate')) {
    dioxus.send(data);
  }
});
document.addEventListener('keydown', (event) => {
  const target = event.target;
  if (target && (target.tagName === 'TEXTAREA' || target.tagName === 'INPUT' || target.tagName === 'SELECT' || target.isContentEditable)) { return; }
  if (event.key === 'ArrowRight' || event.key === 'PageDown') {
    event.preventDefault();
    dioxus.send({ type: 'swift-design-navigate', step: 1 });
  } else if (event.key === 'ArrowLeft' || event.key === 'PageUp') {
    event.preventDefault();
    dioxus.send({ type: 'swift-design-navigate', step: -1 });
  }
});
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') { dioxus.send({ type: 'swift-design-escape' }); }
});
let dragFrom = null;
let dragOver = null;
const thumbnailIndex = (element) => {
  const thumbnail = element && element.closest ? element.closest('.thumbnail[data-index]') : null;
  return thumbnail ? Number(thumbnail.dataset.index) : null;
};
const endDrag = () => {
  if (dragFrom === null) { return; }
  dragFrom = null;
  dragOver = null;
  dioxus.send({ type: 'swift-design-drag', action: 'end', screen: 0 });
};
document.addEventListener('pointerdown', (event) => {
  if (event.button !== 0 || (event.target.closest && event.target.closest('button'))) { return; }
  const index = thumbnailIndex(event.target);
  if (index === null) { return; }
  dragFrom = index;
  dragOver = index;
  dioxus.send({ type: 'swift-design-drag', action: 'start', screen: index });
});
document.addEventListener('pointermove', (event) => {
  if (dragFrom === null) { return; }
  const over = thumbnailIndex(document.elementFromPoint(event.clientX, event.clientY));
  if (over !== null && over !== dragOver) {
    dragOver = over;
    dioxus.send({ type: 'swift-design-drag', action: 'over', screen: over });
  }
});
document.addEventListener('pointerup', endDrag);
document.addEventListener('pointercancel', endDrag);
window.addEventListener('blur', endDrag);
";

/// Posts one command from the inspector into the preview iframe.
const APPLY_TO_PREVIEW: &str = "\
const message = await dioxus.recv();
const frame = document.querySelector('iframe[data-preview]');
if (frame && frame.contentWindow) {
  frame.contentWindow.postMessage(message, window.location.origin);
}
";

/// The computed styles of the selected node, as the preview reports
/// them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct NodeStyles {
    #[serde(default)]
    font_size: String,
    #[serde(default)]
    color: String,
    #[serde(default)]
    text_align: String,
    #[serde(default)]
    padding: String,
    #[serde(default)]
    src: String,
    #[serde(default)]
    is_leaf: bool,
}

/// One message posted by the editable preview page.
#[derive(Debug, Deserialize)]
struct PreviewMessage {
    /// `swift-design-html`, `swift-design-select`, `swift-design-action`,
    /// `swift-design-navigate`, `swift-design-drag`, or `swift-design-escape`.
    #[serde(rename = "type")]
    kind: String,
    /// Screen index the message is about. Absent on navigate messages.
    #[serde(default)]
    screen: usize,
    /// Node path from the screen root, like `0/2/1`. `None` for the screen.
    #[serde(default)]
    path: Option<String>,
    /// Node tag name.
    #[serde(default)]
    tag: Option<String>,
    /// Node class attribute.
    #[serde(default)]
    classes: Option<String>,
    /// Start of the node text.
    #[serde(default)]
    text: Option<String>,
    /// Computed styles of the node.
    #[serde(default)]
    styles: Option<NodeStyles>,
    /// The screen root's HTML after a change.
    #[serde(default)]
    html: Option<String>,
    /// True when the change should be saved at once.
    #[serde(default)]
    save: bool,
    /// The menu action: `ask`, `properties`, or `delete-screen`.
    #[serde(default)]
    action: Option<String>,
    /// Screens to move by, on navigate messages: 1 or -1.
    #[serde(default)]
    step: i32,
}

/// The node the user selected in the preview.
#[derive(Clone, Debug, Default, PartialEq)]
struct SelectedNode {
    path: String,
    tag: String,
    classes: String,
    text: String,
    styles: NodeStyles,
}

impl SelectedNode {
    fn from_message(message: &PreviewMessage) -> Option<Self> {
        Some(Self {
            path: message.path.clone()?,
            tag: message.tag.clone().unwrap_or_default(),
            classes: message.classes.clone().unwrap_or_default(),
            text: message.text.clone().unwrap_or_default(),
            styles: message.styles.clone().unwrap_or_default(),
        })
    }
}

/// A command the inspector sends into the preview.
#[derive(Clone, Debug, Serialize)]
struct NodeCommand {
    #[serde(rename = "type")]
    kind: &'static str,
    screen: usize,
    path: String,
    property: &'static str,
    value: String,
}

/// Loads one design, then hands it to the editor.
#[component]
pub fn Editor(design_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = design_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_design(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(design)) => rsx! {
            LoadedEditor { design_id, initial: design.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading design…" }
        },
    }
}

/// The editor for a loaded design: chat on the left, preview and
/// thumbnails on the right, the properties sheet on demand.
#[component]
fn LoadedEditor(design_id: String, initial: Design, on_back: EventHandler<()>) -> Element {
    let mut design = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut messages = use_signal(Vec::<String>::new);
    let mut preview_version = use_signal(|| 0u32);
    let mut user_paths = use_signal(Vec::<String>::new);
    let mut is_dirty = use_signal(|| false);
    let mut show_properties = use_signal(|| false);
    // The toolbar's `…` menu, and the template name being typed while
    // its save prompt is open.
    let mut is_menu_open = use_signal(|| false);
    let mut template_name = use_signal(|| Option::<String>::None);
    // The node reference the chat prepends to the next message.
    let mut chat_context = use_signal(|| Option::<String>::None);
    // Thumbnail being dragged, and a screen awaiting delete confirmation.
    let mut dragged = use_signal(|| Option::<usize>::None);
    let mut pending_screen_delete = use_signal(|| Option::<usize>::None);

    let authors_id = design_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_user_paths(&id).await {
                user_paths.set(paths);
            }
        }
    });
    // Until the settings answer, the PDF and PPTX buttons stay enabled:
    // the server answers 503 with the install hint when Chrome is missing.
    let settings = use_resource(|| async { api::fetch_settings().await.ok() });
    let can_export_pdf = settings().flatten().is_none_or(|view| view.has_chrome);

    // Saves the design and refreshes the badges. The preview reloads only
    // when `reload_preview` is true: in-place edits already show.
    let save = use_callback({
        let design_id = design_id.clone();
        move |reload_preview: bool| {
            let id = design_id.clone();
            let snapshot = design();
            spawn(async move {
                match api::save_design(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    // Reloads the design from the server after the engine edited it.
    let reload = use_callback({
        let design_id = design_id.clone();
        move |_: ()| {
            let id = design_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_design(&id).await {
                    design.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_user_paths(&id).await {
                    user_paths.set(paths);
                }
            });
        }
    });

    // Apply HTML updates, selections, and menu actions from the preview.
    use_future(move || async move {
        let mut preview_channel = document::eval(PREVIEW_LISTENER);
        while let Ok(message) = preview_channel.recv::<PreviewMessage>().await {
            match message.kind.as_str() {
                "swift-design-escape" => {
                    is_menu_open.set(false);
                    template_name.set(None);
                }
                "swift-design-navigate" => {
                    let count = design.peek().screens.len();
                    let current = selected();
                    let next = if message.step > 0 {
                        (current + 1).min(count.saturating_sub(1))
                    } else {
                        current.saturating_sub(1)
                    };
                    if next != current {
                        selected.set(next);
                        selected_node.set(None);
                    }
                }
                "swift-design-select" if message.screen < design.peek().screens.len() => {
                    selected.set(message.screen);
                    let node = SelectedNode::from_message(&message);
                    if let Some(node) = &node {
                        chat_context.set(Some(node_reference(message.screen, node)));
                    }
                    selected_node.set(node);
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        design.with_mut(|design| match design.screens.get_mut(message.screen) {
                            Some(screen) if screen.html != html => {
                                screen.html = html;
                                true
                            }
                            _ => false,
                        });
                    if is_changed {
                        is_dirty.set(true);
                    }
                    if message.save && is_dirty() {
                        save.call(false);
                    }
                }
                "swift-design-drag" => match message.action.as_deref() {
                    Some("start") if message.screen < design.peek().screens.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < design.peek().screens.len()
                        {
                            design.with_mut(|design| move_screen(&mut design.screens, from, to));
                            dragged.set(Some(to));
                            selected.set(to);
                            selected_node.set(None);
                            is_dirty.set(true);
                        }
                    }
                    Some("end") if dragged().is_some() => {
                        dragged.set(None);
                        if is_dirty() {
                            save.call(true);
                        }
                    }
                    _ => {}
                },
                "swift-design-action" => match message.action.as_deref() {
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference(message.screen, &node),
                            None => format!("[screen {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = design.with_mut(|design| {
                            if design.screens.len() > 1 && message.screen < design.screens.len() {
                                design.screens.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = design.peek().screens.len();
                            selected.set(message.screen.min(count.saturating_sub(1)));
                            selected_node.set(None);
                            save.call(true);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    });

    // Sends an inspector command into the preview iframe.
    let apply = use_callback(move |command: NodeCommand| {
        let channel = document::eval(APPLY_TO_PREVIEW);
        let _ = channel.send(command);
    });

    let screen_count = design().screens.len();
    let outline_count = design().outline.len();
    let total_fields = field_count(&design());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = design()
        .screens
        .iter()
        .enumerate()
        .map(|(index, screen)| screen_label(index, screen))
        .collect();
    let current_notes = design()
        .screens
        .get(selected())
        .and_then(|screen| screen.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: design_id.clone(),
                context: chat_context,
                on_before_send: move |_| {
                    if is_dirty() {
                        save.call(false);
                    }
                },
                on_run_finished: move |_| reload.call(()),
            }
            section { class: "editor-preview",
                div { class: "editor-toolbar",
                    button { class: "back", onclick: move |_| on_back.call(()),
                        span { dangerous_inner_html: icons::CHEVRON_LEFT }
                        "Project"
                    }
                    span { class: "divider" }
                    span { class: "preview-heading", "{selected() + 1} / {screen_count}" }
                    span { class: "badge", "agent {agent_count}" }
                    span { class: "badge you", "you {user_count}" }
                    if is_dirty() {
                        button {
                            class: "primary",
                            onclick: move |_| save.call(true),
                            "Save"
                        }
                    } else {
                        span { class: "save-state",
                            span { dangerous_inner_html: icons::CHECK }
                            "saved"
                        }
                    }
                    div { class: "actions",
                        button { onclick: move |_| show_properties.set(!show_properties()),
                            "Properties"
                        }
                        span { class: "divider" }
                        div { class: "export-group",
                            a {
                                class: "button",
                                href: "/designs/{design_id}/export",
                                title: "Export as one HTML file",
                                span { dangerous_inner_html: icons::DOWNLOAD }
                                "HTML"
                            }
                            if can_export_pdf {
                                a {
                                    class: "button",
                                    href: "/designs/{design_id}/export.pdf",
                                    title: "Export as a PDF",
                                    span { dangerous_inner_html: icons::DOWNLOAD }
                                    "PDF"
                                }
                            } else {
                                span {
                                    class: "export-cell",
                                    title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
                                    a {
                                        class: "button",
                                        "aria-disabled": "true",
                                        span { dangerous_inner_html: icons::DOWNLOAD }
                                        "PDF"
                                    }
                                }
                            }
                        }
                        button {
                            class: "toolbar-more",
                            title: "More actions",
                            onclick: move |_| is_menu_open.set(!is_menu_open()),
                            "…"
                        }
                    }
                    if is_menu_open() {
                        div {
                            class: "menu-backdrop",
                            onclick: move |_| {
                                is_menu_open.set(false);
                                template_name.set(None);
                            },
                        }
                        div { class: "toolbar-menu",
                            if let Some(name) = template_name() {
                                input {
                                    class: "template-name",
                                    placeholder: "Template name",
                                    value: "{name}",
                                    autofocus: true,
                                    oninput: move |event| template_name.set(Some(event.value())),
                                }
                                div { class: "menu-actions",
                                    button {
                                        class: "primary",
                                        disabled: name.trim().is_empty(),
                                        onclick: {
                                            let design_id = design_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let design_id = design_id.clone();
                                                spawn(async move {
                                                    match api::save_template(&design_id, &name).await {
                                                        Ok(saved) => {
                                                            template_name.set(None);
                                                            is_menu_open.set(false);
                                                            messages
                                                                .set(vec![format!("Saved the template `{}`.", saved.name)]);
                                                        }
                                                        Err(message) => messages.set(vec![message]),
                                                    }
                                                });
                                            }
                                        },
                                        "Save template"
                                    }
                                    button { onclick: move |_| template_name.set(None),
                                        "Cancel"
                                    }
                                }
                            } else {
                                button {
                                    title: "Keep this design's theme and layout style for a future design",
                                    onclick: move |_| template_name.set(Some(String::new())),
                                    "Save as template"
                                }
                            }
                        }
                    }
                }
                div { class: "editor-body",
                    for message in messages() {
                        p { class: "error", "{message}" }
                    }
                    iframe {
                        title: "Design preview",
                        "data-preview": "true",
                        src: "/designs/{design_id}/render?version={preview_version()}&editable=true&screen={selected() + 1}",
                    }
                    p { class: "preview-hint",
                        span { "Click a node to reference it in the chat and edit its text" }
                        span { class: "dot", "·" }
                        span { "right-click for quick edits" }
                        span { class: "dot", "·" }
                        span {
                            kbd { "←" }
                            " "
                            kbd { "→" }
                            " change screens"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Presenter notes"
                            span { class: "screen-no", "screen {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "What to say on this screen. Never shown on the screen.",
                            oninput: move |event| {
                                let index = selected();
                                design.with_mut(|design| {
                                    if let Some(screen) = design.screens.get_mut(index) {
                                        screen.notes = optional(event.value());
                                    }
                                });
                                is_dirty.set(true);
                            },
                        }
                    }
                    div { class: "thumbnails",
                        for (index, label) in thumbnail_labels.into_iter().enumerate() {
                            div {
                                key: "{index}",
                                class: if dragged() == Some(index) { "thumbnail current dragging" } else if index == selected() { "thumbnail current" } else { "thumbnail" },
                                title: "{label} · drag to reorder",
                                "data-index": "{index}",
                                onclick: move |_| {
                                    selected.set(index);
                                    selected_node.set(None);
                                    pending_screen_delete.set(None);
                                },
                                iframe {
                                    title: "Screen {index + 1}",
                                    tabindex: "-1",
                                    src: "/designs/{design_id}/render?version={preview_version()}&screen={index + 1}",
                                }
                                span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                if screen_count > 1 {
                                    button {
                                        class: if pending_screen_delete() == Some(index) { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                        title: "Delete this screen",
                                        onclick: move |event: Event<MouseData>| {
                                            event.stop_propagation();
                                            if pending_screen_delete() == Some(index) {
                                                pending_screen_delete.set(None);
                                                design.with_mut(|design| {
                                                    if design.screens.len() > 1 && index < design.screens.len() {
                                                        design.screens.remove(index);
                                                    }
                                                });
                                                selected.set(selected().min(screen_count.saturating_sub(2)));
                                                selected_node.set(None);
                                                save.call(true);
                                            } else {
                                                pending_screen_delete.set(Some(index));
                                            }
                                        },
                                        if pending_screen_delete() == Some(index) {
                                            "Delete?"
                                        } else {
                                            "×"
                                        }
                                    }
                                }
                            }
                        }
                        for index in screen_count..outline_count {
                            div {
                                key: "outline-{index}",
                                class: "thumbnail outline",
                                title: "{outline_title(&design(), index)} · not written yet",
                                span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                span { class: "outline-label", "outline" }
                            }
                        }
                        button {
                            class: "thumbnail add",
                            title: "Add a screen",
                            onclick: move |_| {
                                design.with_mut(|design| design.screens.push(default_screen()));
                                selected.set(screen_count);
                                selected_node.set(None);
                                save.call(true);
                            },
                            "+"
                        }
                    }
                }
            }
            if show_properties() {
                aside { class: "properties-sheet",
                    div { class: "sheet-head",
                        span { class: "kicker", "Properties" }
                        span { class: "spacer" }
                        button {
                            class: "primary",
                            onclick: move |_| save.call(true),
                            "Save"
                        }
                        button { onclick: move |_| show_properties.set(false), "Close" }
                    }
                    div { class: "sheet-body",
                        NodeInspector {
                            screen: selected(),
                            node: selected_node(),
                            on_apply: move |command: NodeCommand| apply.call(command),
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Design" }
                            label {
                                "Design title"
                                input {
                                    value: "{design().title}",
                                    oninput: move |event| {
                                        design.with_mut(|design| design.title = event.value());
                                        is_dirty.set(true);
                                    },
                                }
                            }
                        }
                        ThemeForm { design, is_dirty }
                        TransitionForm {
                            design,
                            is_dirty,
                            on_change: move |_| save.call(false),
                        }
                        HistorySection {
                            design_id: design_id.clone(),
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// The save history of the open design: one row per snapshot, newest
/// first, each with a Restore button. `on_restored` fires after a
/// restore, so the caller reloads the design.
#[component]
fn HistorySection(design_id: String, on_restored: EventHandler<()>) -> Element {
    let id_for_fetch = design_id.clone();
    let mut snapshots = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_design_history(&id).await }
    });
    let mut error = use_signal(|| Option::<String>::None);
    let rows = snapshots.read().clone();
    rsx! {
        div { class: "sheet-section",
            div { class: "head", "History" }
            if let Some(message) = error() {
                p { class: "error", "{message}" }
            }
            match rows {
                Some(Ok(rows)) if rows.is_empty() => rsx! {
                    p { class: "inspector-hint", "No earlier versions yet. Each save keeps the version before it here." }
                },
                Some(Ok(rows)) => rsx! {
                    ul { class: "history-list",
                        for row in rows {
                            li { key: "{row.stamp}", class: "history-row",
                                span { class: "mono", title: "{row.saved_at}", "{history_label(&row.saved_at)}" }
                                span { class: "attachment-size", "{format_size(row.size_bytes)}" }
                                button {
                                    title: "Make this version the current design",
                                    onclick: {
                                        let design_id = design_id.clone();
                                        let stamp = row.stamp.clone();
                                        move |_| {
                                            let design_id = design_id.clone();
                                            let stamp = stamp.clone();
                                            spawn(async move {
                                                match api::restore_design_history(&design_id, &stamp).await {
                                                    Ok(()) => {
                                                        error.set(None);
                                                        on_restored.call(());
                                                        snapshots.restart();
                                                    }
                                                    Err(message) => error.set(Some(message)),
                                                }
                                            });
                                        }
                                    },
                                    "Restore"
                                }
                            }
                        }
                    }
                },
                Some(Err(message)) => rsx! {
                    p { class: "error", "{message}" }
                },
                None => rsx! {
                    p { class: "inspector-hint", "Loading history…" }
                },
            }
        }
    }
}

/// A saved-at time for the history list: `2026-08-25 10:14:02 UTC`.
fn history_label(saved_at: &str) -> String {
    saved_at.replacen('T', " ", 1).replacen('Z', " UTC", 1)
}

/// The page transition picker: the effect between screens, the direction
/// they travel, and how long the move takes.
///
/// `None` is a real choice: a design with no transition scrolls, which is
/// what a design did before this field existed.
#[component]
fn TransitionForm(
    mut design: Signal<Design>,
    mut is_dirty: Signal<bool>,
    on_change: EventHandler<()>,
) -> Element {
    let current = design().transition;
    let effect = current.map(|transition| transition.effect);
    let axis = current.unwrap_or_default().axis;
    let duration = current.unwrap_or_default().duration_ms;
    let pick_effect = use_callback(move |value: &'static str| {
        design.with_mut(|design| {
            design.transition = effect_from_value(value).map(|effect| Transition {
                effect,
                ..design.transition.unwrap_or_default()
            });
        });
        is_dirty.set(true);
        on_change.call(());
    });
    // Direction and duration show only for effects that move: a scroll
    // page and a cut have nothing to direct or time.
    rsx! {
        div { class: "sheet-section",
            div { class: "head", "Page transition" }
            div { class: "effect-chips",
                button {
                    class: if effect.is_none() { "selected" } else { "" },
                    title: "No transition: the design scrolls",
                    onclick: move |_| pick_effect.call("scroll"),
                    "Scroll"
                }
                for (value, label) in TRANSITION_EFFECTS {
                    button {
                        key: "{value}",
                        class: if effect.map(TransitionEffect::as_str) == Some(value) { "selected" } else { "" },
                        onclick: move |_| pick_effect.call(value),
                        "{label}"
                    }
                }
            }
            if effect_uses_motion(effect) {
                div { class: "theme-grid",
                    div { class: "field",
                        span { class: "field-label", "Direction" }
                        Select {
                            value: if axis == TransitionAxis::Horizontal { "horizontal" } else { "vertical" },
                            options: vec![
                                ("vertical".to_owned(), "Vertical".to_owned()),
                                ("horizontal".to_owned(), "Horizontal".to_owned()),
                            ],
                            on_change: move |direction: String| {
                                let is_horizontal = direction == "horizontal";
                                design.with_mut(|design| {
                                    let mut transition = design.transition.unwrap_or_default();
                                    transition.axis = if is_horizontal {
                                        TransitionAxis::Horizontal
                                    } else {
                                        TransitionAxis::Vertical
                                    };
                                    design.transition = Some(transition);
                                });
                                is_dirty.set(true);
                                on_change.call(());
                            },
                        }
                    }
                    label {
                        "Duration (ms)"
                        input {
                            r#type: "number",
                            min: "0",
                            max: "{MAX_TRANSITION_MS}",
                            step: "50",
                            value: "{duration}",
                            oninput: move |event| {
                                let Ok(value) = event.value().parse::<u32>() else {
                                    return;
                                };
                                let value = value.min(MAX_TRANSITION_MS);
                                design.with_mut(|design| {
                                    let mut transition = design.transition.unwrap_or_default();
                                    transition.duration_ms = value;
                                    design.transition = Some(transition);
                                });
                                is_dirty.set(true);
                                on_change.call(());
                            },
                        }
                    }
                }
            }
            p { class: "inspector-hint",
                "Direction moves Push and Cover. Duration applies to every effect except Cut. "
                "A change saves at once and plays in the exported HTML. "
                "The editor preview shows one screen, so it does not animate."
            }
        }
    }
}

/// True when `effect` has a direction and a duration to set: every
/// effect except the scroll page (`None`) and a cut.
fn effect_uses_motion(effect: Option<TransitionEffect>) -> bool {
    !matches!(effect, None | Some(TransitionEffect::None))
}

/// The transition effects the picker offers, as (JSON value, label).
const TRANSITION_EFFECTS: [(&str, &str); 5] = [
    ("none", "Cut"),
    ("fade", "Fade"),
    ("push", "Push"),
    ("cover", "Cover"),
    ("zoom", "Zoom"),
];

/// The effect a picker value names. `None` is the scroll page.
fn effect_from_value(value: &str) -> Option<TransitionEffect> {
    match value {
        "none" => Some(TransitionEffect::None),
        "fade" => Some(TransitionEffect::Fade),
        "push" => Some(TransitionEffect::Push),
        "cover" => Some(TransitionEffect::Cover),
        "zoom" => Some(TransitionEffect::Zoom),
        _ => None,
    }
}

/// The chat reference for one node: `[screen 3, node 0/1 <h2.title>: text]`.
fn node_reference(screen_index: usize, node: &SelectedNode) -> String {
    let class = node
        .classes
        .split_whitespace()
        .next()
        .map(|class| format!(".{class}"))
        .unwrap_or_default();
    let text: String = node.text.chars().take(40).collect();
    if text.is_empty() {
        format!(
            "[screen {}, node {} <{}{class}>]",
            screen_index + 1,
            node.path,
            node.tag
        )
    } else {
        format!(
            "[screen {}, node {} <{}{class}>: {text}]",
            screen_index + 1,
            node.path,
            node.tag
        )
    }
}

/// The selected node's quick fields. Every change is sent into the
/// preview, which applies it and posts the screen's HTML back.
#[component]
fn NodeInspector(
    screen: usize,
    node: Option<SelectedNode>,
    on_apply: EventHandler<NodeCommand>,
) -> Element {
    let Some(node) = node else {
        return rsx! {
            div { class: "sheet-section",
                div { class: "head", "Node" }
                div { class: "inspector-empty",
                    span {
                        class: "glyph",
                        dangerous_inner_html: icons::DASHED_SQUARE,
                    }
                    p { class: "inspector-hint",
                        "Click a node in the preview to edit its text, size, color, alignment, or padding here."
                    }
                }
            }
        };
    };
    let path = node.path.clone();
    let command = move |property: &'static str, value: String| NodeCommand {
        kind: "swift-design-apply",
        screen,
        path: path.clone(),
        property,
        value,
    };
    let class = node
        .classes
        .split_whitespace()
        .next()
        .map(|class| format!(".{class}"))
        .unwrap_or_default();
    let color = if node.styles.color.is_empty() {
        "#000000".to_owned()
    } else {
        node.styles.color.clone()
    };
    let heading = format!("Node <{}{class}> · {}", node.tag, node.path);
    rsx! {
        div { class: "sheet-section inspector",
            div { class: "head mono-head", "{heading}" }
            div { class: "screen-actions",
                button {
                    onclick: {
                        let command = command.clone();
                        move |_| on_apply.call(command("select_parent", String::new()))
                    },
                    "Select parent"
                }
            }
            if node.styles.is_leaf {
                label {
                    "Text"
                    textarea {
                        value: "{node.text}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("text", event.value()))
                        },
                    }
                }
            }
            div { class: "frame-grid",
                label {
                    "Font size"
                    input {
                        value: "{node.styles.font_size}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("font_size", event.value()))
                        },
                    }
                }
                label {
                    "Color"
                    div { class: "color-field",
                        input {
                            r#type: "color",
                            value: "{color}",
                            oninput: {
                                let command = command.clone();
                                move |event| on_apply.call(command("text_color", event.value()))
                            },
                        }
                        span { class: "color-code mono", "{color}" }
                    }
                }
                div { class: "field",
                    span { class: "field-label", "Align" }
                    Select {
                        value: node.styles.text_align.clone(),
                        options: plain_options(["start", "left", "center", "right", "justify"]),
                        on_change: {
                            let command = command.clone();
                            move |align: String| on_apply.call(command("text_align", align))
                        },
                    }
                }
                label {
                    "Padding"
                    input {
                        value: "{node.styles.padding}",
                        oninput: {
                            let command = command.clone();
                            move |event| on_apply.call(command("padding", event.value()))
                        },
                    }
                }
                if node.tag == "img" {
                    label {
                        "Image source: pick a file under Uploads, or type a /uploads/… path"
                        input {
                            value: "{node.styles.src}",
                            oninput: {
                                let command = command.clone();
                                move |event| on_apply.call(command("src", event.value()))
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Google font families offered for headings and body text. The server
/// loads any of them from Google Fonts.
const TEXT_FONTS: [&str; 30] = [
    "Inter",
    "Roboto",
    "Open Sans",
    "Lato",
    "Montserrat",
    "Poppins",
    "Raleway",
    "Nunito",
    "Work Sans",
    "DM Sans",
    "Manrope",
    "Outfit",
    "Sora",
    "Plus Jakarta Sans",
    "Space Grotesk",
    "IBM Plex Sans",
    "Source Sans 3",
    "Fira Sans",
    "Rubik",
    "Archivo",
    "Oswald",
    "Bebas Neue",
    "Josefin Sans",
    "Playfair Display",
    "Lora",
    "Merriweather",
    "Libre Baskerville",
    "EB Garamond",
    "Cormorant Garamond",
    "Crimson Text",
];

/// Monospace families offered for code and figures.
const MONO_FONTS: [&str; 9] = [
    "JetBrains Mono",
    "Fira Code",
    "IBM Plex Mono",
    "Source Code Pro",
    "Roboto Mono",
    "Space Mono",
    "DM Mono",
    "Inconsolata",
    "Courier Prime",
];

/// System families that need no download.
const SYSTEM_FONTS: [&str; 4] = ["system-ui", "sans-serif", "serif", "monospace"];

/// The options for one font select: the current value first when it is
/// not in the list, then `families`, then the system families.
fn font_options(current: &str, families: &[&str]) -> Vec<String> {
    let mut options: Vec<String> = Vec::with_capacity(families.len() + SYSTEM_FONTS.len() + 1);
    let is_listed = families
        .iter()
        .chain(SYSTEM_FONTS.iter())
        .any(|family| family.eq_ignore_ascii_case(current));
    if !is_listed && !current.trim().is_empty() {
        options.push(current.to_owned());
    }
    options.extend(families.iter().map(|family| (*family).to_owned()));
    options.extend(SYSTEM_FONTS.iter().map(|family| (*family).to_owned()));
    options
}

/// A font family select. A family outside the list, written by the
/// agent, stays selectable so the value never changes on open.
#[component]
fn FontField(
    label: &'static str,
    value: String,
    families: &'static [&'static str],
    on_change: EventHandler<String>,
) -> Element {
    let options = font_options(&value, families);
    // The listed spelling of the current family, so a lower-case value
    // written by the agent still shows as chosen.
    let chosen = options
        .iter()
        .find(|family| family.eq_ignore_ascii_case(&value))
        .cloned()
        .unwrap_or_else(|| value.clone());
    rsx! {
        div { class: "field",
            span { class: "field-label", "{label}" }
            Select {
                value: chosen,
                options: plain_options(options),
                on_change: move |family| on_change.call(family),
            }
        }
    }
}

/// Theme fields: the four palette colors and the three fonts. The
/// theme name stays the agent's label for the design; the user never
/// needs to edit it.
#[component]
fn ThemeForm(mut design: Signal<Design>, mut is_dirty: Signal<bool>) -> Element {
    rsx! {
        div { class: "sheet-section theme",
            div { class: "head", "Theme" }
            div { class: "color-list",
                RequiredColorField {
                    label: "Background color",
                    value: design().theme.colors.background.clone(),
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.colors.background = value);
                        is_dirty.set(true);
                    },
                }
                RequiredColorField {
                    label: "Text color",
                    value: design().theme.colors.text.clone(),
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.colors.text = value);
                        is_dirty.set(true);
                    },
                }
                RequiredColorField {
                    label: "Accent color",
                    value: design().theme.colors.accent.clone(),
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.colors.accent = value);
                        is_dirty.set(true);
                    },
                }
                RequiredColorField {
                    label: "Muted color",
                    value: design().theme.colors.muted.clone(),
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.colors.muted = value);
                        is_dirty.set(true);
                    },
                }
            }
            div { class: "font-grid",
                FontField {
                    label: "Heading font",
                    value: design().theme.fonts.heading.clone(),
                    families: &TEXT_FONTS,
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.fonts.heading = value);
                        is_dirty.set(true);
                    },
                }
                FontField {
                    label: "Body font",
                    value: design().theme.fonts.body.clone(),
                    families: &TEXT_FONTS,
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.fonts.body = value);
                        is_dirty.set(true);
                    },
                }
                FontField {
                    label: "Mono font",
                    value: design().theme.fonts.mono.clone(),
                    families: &MONO_FONTS,
                    on_change: move |value: String| {
                        design.with_mut(|design| design.theme.fonts.mono = value);
                        is_dirty.set(true);
                    },
                }
            }
        }
    }
}

/// One row of the palette list: the swatch, the colour's name, and its
/// hex code. The colour always has a value.
#[component]
fn RequiredColorField(
    label: &'static str,
    value: String,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        label { class: "color-field",
            input {
                r#type: "color",
                value: "{value}",
                oninput: move |event| on_change.call(event.value()),
            }
            span { class: "color-name", "{label}" }
            span { class: "color-code mono", "{value}" }
        }
    }
}

/// Moves the screen at `from` to position `to`, shifting the others.
fn move_screen(screens: &mut Vec<Screen>, from: usize, to: usize) {
    if from == to || from >= screens.len() || to >= screens.len() {
        return;
    }
    let screen = screens.remove(from);
    screens.insert(to, screen);
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the screen, else `Screen N`.
fn screen_label(index: usize, screen: &Screen) -> String {
    let heading = first_heading(&screen.html).map(|text| strip_tags(&text));
    let text = match heading {
        Some(text) if !text.is_empty() => text,
        _ => strip_tags(&screen.html).chars().take(40).collect(),
    };
    if text.is_empty() {
        format!("{}. Screen {}", index + 1, index + 1)
    } else {
        format!("{}. {text}", index + 1)
    }
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Screen 5` when the outline has no entry there.
fn outline_title(design: &Design, index: usize) -> String {
    match design.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Screen {}", index + 1, index + 1),
    }
}

/// The inner HTML of the first `<h1>`, `<h2>`, or `<h3>` in `html`.
fn first_heading(html: &str) -> Option<String> {
    let lowered = html.to_ascii_lowercase();
    let mut best: Option<(usize, String)> = None;
    for level in ["h1", "h2", "h3"] {
        let open = format!("<{level}");
        let Some(start) = lowered.find(&open) else {
            continue;
        };
        let Some(content_start) = lowered[start..].find('>').map(|offset| start + offset + 1)
        else {
            continue;
        };
        let close = format!("</{level}>");
        let Some(end) = lowered[content_start..]
            .find(&close)
            .map(|offset| content_start + offset)
        else {
            continue;
        };
        let candidate = html[content_start..end].to_owned();
        if best.as_ref().is_none_or(|(position, _)| start < *position) {
            best = Some((start, candidate));
        }
    }
    best.map(|(_, text)| text)
}

/// Text without HTML tags, whitespace collapsed.
fn strip_tags(html: &str) -> String {
    let mut text = String::new();
    let mut is_in_tag = false;
    for character in html.chars() {
        match character {
            '<' => is_in_tag = true,
            '>' => {
                is_in_tag = false;
                text.push(' ');
            }
            other if !is_in_tag => text.push(other),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The screen the + tile inserts: a heading and a paragraph.
fn default_screen() -> Screen {
    Screen {
        name: String::new(),
        html: "<div class='body'><h2>New screen</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 90px; height: 100%; display: flex; flex-direction: column; gap: 30px; } h2 { font-size: 54px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// `None` for blank input, so cleared fields leave the design JSON.
fn optional(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Number of set leaf fields in the design. Matches the server's
/// provenance paths: absent optional fields are not counted.
fn field_count(design: &Design) -> usize {
    // Design title, theme name, four colors, three fonts.
    let mut count = 9;
    for screen in &design.screens {
        count += 1 + usize::from(screen.css.is_some()) + usize::from(screen.notes.is_some());
    }
    count
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::TransitionEffect;

    use crate::editor::{
        MONO_FONTS, NodeStyles, SelectedNode, TEXT_FONTS, default_screen, effect_uses_motion,
        field_count, first_heading, font_options, history_label, move_screen, node_reference,
        optional, outline_title, screen_label, strip_tags,
    };

    #[test]
    fn only_moving_effects_show_direction_and_duration() {
        assert!(!effect_uses_motion(None));
        assert!(!effect_uses_motion(Some(TransitionEffect::None)));
        assert!(effect_uses_motion(Some(TransitionEffect::Fade)));
        assert!(effect_uses_motion(Some(TransitionEffect::Push)));
        assert!(effect_uses_motion(Some(TransitionEffect::Cover)));
        assert!(effect_uses_motion(Some(TransitionEffect::Zoom)));
    }

    #[test]
    fn history_labels_read_as_utc_times() {
        assert_eq!(
            history_label("2026-08-25T10:14:02Z"),
            "2026-08-25 10:14:02 UTC"
        );
        assert_eq!(history_label("odd"), "odd");
    }

    #[test]
    fn outline_titles_fall_back_to_the_screen_number() {
        let mut planned = design();
        planned.outline = vec!["Intro".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Intro");
        assert_eq!(outline_title(&planned, 1), "2. Screen 2");
        assert_eq!(outline_title(&planned, 5), "6. Screen 6");
    }

    fn design() -> design_model::Design {
        design_model::Design {
            transition: None,
            title: "T".to_owned(),
            theme: design_model::Theme {
                name: "n".to_owned(),
                colors: design_model::Palette {
                    background: "#000000".to_owned(),
                    text: "#ffffff".to_owned(),
                    accent: "#0e6e63".to_owned(),
                    muted: "#888888".to_owned(),
                },
                fonts: design_model::FontSet {
                    heading: "Inter".to_owned(),
                    body: "Inter".to_owned(),
                    mono: "Menlo".to_owned(),
                },
            },
            viewport: Default::default(),
            screens: vec![default_screen()],
            outline: Vec::new(),
        }
    }

    #[test]
    fn screen_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(screen_label(0, &default_screen()), "1. New screen");
        let plain = design_model::Screen {
            name: String::new(),
            html: "<p>Just <b>some</b> words here</p>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(screen_label(2, &plain), "3. Just some words here");
        let empty = design_model::Screen {
            name: String::new(),
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(screen_label(1, &empty), "2. Screen 2");
        assert_eq!(
            first_heading("<p>x</p><h3>Third</h3><h2>Second</h2>"),
            Some("Third".to_owned())
        );
    }

    #[test]
    fn node_references_name_screen_path_tag_and_text() {
        let node = SelectedNode {
            path: "0/1".to_owned(),
            tag: "h2".to_owned(),
            classes: "title big".to_owned(),
            text: "What Swift Design does".to_owned(),
            styles: NodeStyles::default(),
        };
        assert_eq!(
            node_reference(2, &node),
            "[screen 3, node 0/1 <h2.title>: What Swift Design does]"
        );
        let image = SelectedNode {
            path: "2".to_owned(),
            tag: "img".to_owned(),
            classes: String::new(),
            text: String::new(),
            styles: NodeStyles::default(),
        };
        assert_eq!(node_reference(0, &image), "[screen 1, node 2 <img>]");
    }

    #[test]
    fn the_default_screen_validates_inside_a_design() {
        assert_eq!(design().validate(), Vec::new());
    }

    #[test]
    fn helpers_handle_blank_input_and_counts() {
        assert_eq!(optional("  ".to_owned()), None);
        assert_eq!(optional("x".to_owned()), Some("x".to_owned()));
        assert_eq!(strip_tags("<h1>Hello <em>world</em></h1>"), "Hello world");
        // 9 design and theme fields, then html and css on the one screen.
        assert_eq!(field_count(&design()), 11);
    }

    #[test]
    fn move_screen_shifts_the_others() {
        let mut screens = vec![default_screen(), default_screen(), default_screen()];
        for (index, screen) in screens.iter_mut().enumerate() {
            screen.notes = Some(index.to_string());
        }
        move_screen(&mut screens, 0, 2);
        let order: Vec<String> = screens
            .iter()
            .map(|screen| screen.notes.clone().unwrap())
            .collect();
        assert_eq!(order, ["1", "2", "0"]);
        move_screen(&mut screens, 5, 0);
        assert_eq!(screens.len(), 3);
    }

    #[test]
    fn font_options_keep_an_unlisted_family_first() {
        let options = font_options("Playfair Display", &TEXT_FONTS);
        assert_eq!(options.len(), TEXT_FONTS.len() + 4);
        assert_eq!(options[0], "Inter");
        assert!(options.iter().any(|family| family == "system-ui"));
        let custom = font_options("Comic Neue", &MONO_FONTS);
        assert_eq!(custom[0], "Comic Neue");
        assert_eq!(custom.len(), MONO_FONTS.len() + 5);
        // Case differences do not duplicate a listed family.
        assert_eq!(font_options("inter", &TEXT_FONTS)[0], "Inter");
        assert_eq!(font_options("", &MONO_FONTS)[0], "JetBrains Mono");
    }
}
