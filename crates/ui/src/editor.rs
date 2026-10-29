//! The design editor: a chat column, a live preview with a right-click
//! menu, thumbnails, and a properties sheet for the rare manual edit.
//!
//! The main path is the chat: the user asks for changes and the engine
//! rewrites the design. The preview iframe renders the screen in editable
//! mode: clicking a node selects it and puts a reference in the chat,
//! text is editable in place, and a right-click menu applies quick
//! actions in the iframe's DOM. After every change the iframe posts the
//! screen's HTML back, and this editor stores it as `screens[i].html` and
//! saves. Reordering remaps local screen links before saving. Field badges show who
//! wrote the title, theme, css, and notes: the agent, or the user.

use design_model::transition::MAX_TRANSITION_MS;
use design_model::{
    ArtifactKind, Design, LinkedUnit, Screen, Theme, Transition, TransitionAxis, TransitionEffect,
    Viewport, reorder_units,
};
use dioxus::document;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::canvas::{frame_width_rem, is_narrow_canvas, is_portrait_canvas};
use crate::chat::DesignChat;
use crate::icons;
use crate::select::{Select, plain_options};
use crate::settings::artifact_project;

mod history;
mod node;
mod preview;
mod support;
mod theme;
mod transition;

pub(crate) use history::HistorySection;
pub(crate) use node::{NodeInspector, node_reference, selection_paths};
pub(crate) use preview::{
    APPLY_TO_PREVIEW, NodeCommand, PREVIEW_LISTENER, PreviewFrame, PreviewMessage, PreviewMode,
    SelectedNode, SelectionEntry, pin_reference, selection_of,
};
pub(crate) use support::{
    STRIP_TILE_HEIGHT_REM, ThumbnailState, fragment_label, move_screen, navigation_target,
    optional, outline_entry, page_reference, preview_stage_class, schedule_save, strip_summary,
    thumbnail_class, toggle_pin,
};
pub(crate) use theme::ThemeForm;
pub(crate) use transition::TransitionForm;

use support::{DesignExportGroup, field_count, outline_title, screen_label, view_href};
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
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    // The screens pinned for the chat with a command-click on a tile.
    let mut pinned = use_signal(Vec::<usize>::new);
    let mut messages = use_signal(Vec::<String>::new);
    let mut preview_version = use_signal(|| 0u32);
    let mut user_paths = use_signal(Vec::<String>::new);
    let mut is_dirty = use_signal(|| false);
    // Autosave: the newest change wins the delay.
    let save_generation = use_signal(|| 0u64);
    let mut show_properties = use_signal(|| false);
    // The toolbar's `…` menu, and the template name being typed while
    // its save prompt is open.
    let mut is_menu_open = use_signal(|| false);
    let mut mode = use_signal(|| PreviewMode::Play);
    let mut template_name = use_signal(|| Option::<String>::None);
    // The node reference the chat prepends to the next message.
    let mut chat_context = use_signal(|| Option::<String>::None);
    // Thumbnail being dragged, and a screen awaiting delete confirmation.
    let mut dragged = use_signal(|| Option::<usize>::None);
    let mut pending_screen_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);
    let settings = use_resource(|| async { api::fetch_settings().await.ok() });

    let authors_id = design_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_user_paths(&id).await {
                user_paths.set(paths);
            }
        }
    });
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
                    let next = navigation_target(current, count, message.step, message.target);
                    if next != current {
                        selected.set(next);
                        selected_node.set(None);
                    }
                }
                "swift-design-select" if message.screen < design.peek().screens.len() => {
                    selected.set(message.screen);
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
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
                            design.with_mut(|design| {
                                move_screen(&mut design.screens, &mut design.outline, from, to)
                            });
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
                    Some("pin") => {
                        selected.set(message.screen);
                        chat_context.set(Some(pin_reference(
                            "screen",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference("screen", message.screen, &node),
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

    // Sends an inspector command into the preview iframe, once per
    // selected node: a change made on the primary node lands on every
    // node in the selection.
    let apply = use_callback(move |command: NodeCommand| {
        for path in selection_paths(&selection.peek(), &command.path) {
            let channel = document::eval(APPLY_TO_PREVIEW);
            let _ = channel.send(NodeCommand {
                path,
                ..command.clone()
            });
        }
    });

    let screen_count = design().screens.len();
    let outline_count = design().outline.len();
    let can_export_with_chrome = settings().flatten().is_none_or(|view| view.has_chrome);
    // The render page fits the screen to its frame, so a frame with the
    // canvas ratio shows the screen edge to edge. Every thumbnail is
    // the same height, and as wide as the ratio says.
    let viewport = design().viewport;
    let ratio = viewport.aspect_ratio_css();
    let stage_class = preview_stage_class(viewport);
    let is_portrait = is_portrait_canvas(viewport);
    let tile_width = frame_width_rem(viewport, STRIP_TILE_HEIGHT_REM);
    let planned_count = outline_count.saturating_sub(screen_count);
    let summary = strip_summary(screen_count, planned_count);
    let total_fields = field_count(&design());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = design()
        .screens
        .iter()
        .enumerate()
        .map(|(index, screen)| screen_label(index, screen))
        .collect();
    let page_labels = thumbnail_labels.clone();
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
                page: page_reference("screen", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: page_labels.clone(),
                page_unit: Some("screen".to_owned()),
                on_drop_page: move |_| pinned.write().clear(),
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
                    div {
                        class: "canvas-tabs preview-modes",
                        role: "group",
                        aria_label: "Demo preview controls",
                        for candidate in PreviewMode::ALL {
                            button {
                                key: "{candidate.label()}",
                                aria_pressed: "{mode() == candidate}",
                                class: if mode() == candidate { "canvas-tab open" } else { "canvas-tab" },
                                title: "{candidate.title()}",
                                onclick: move |_| {
                                    mode.set(candidate);
                                    selected_node.set(None);
                                },
                                span { class: "tab-name", "{candidate.label()}" }
                            }
                        }
                        if is_dirty() {
                            button {
                                class: "canvas-tab",
                                disabled: true,
                                title: "Wait for changes to save before viewing the demo",
                                "View"
                            }
                        } else {
                            a {
                                class: "canvas-tab",
                                href: view_href(&design_id),
                                target: "_blank",
                                rel: "noopener noreferrer",
                                title: "View the full demo in a new tab",
                                "View"
                            }
                        }
                    }
                    span { class: "badge", "agent {agent_count}" }
                    span { class: "badge you", "you {user_count}" }
                    // Every change saves by itself, shortly after it. The
                    // cue shows only while a save is due.
                    if is_dirty() {
                        span { class: "save-state pending", "saving…" }
                    }
                    div { class: "actions",
                        button { onclick: move |_| show_properties.set(!show_properties()),
                            "Properties"
                        }
                        span { class: "divider" }
                        DesignExportGroup {
                            design_id: design_id.clone(),
                            selected: selected(),
                            can_export_with_chrome,
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
                    div { class: stage_class,
                        PreviewFrame {
                            title: "Design preview",
                            ratio: "{ratio}",
                            src: "/designs/{design_id}/render?version={preview_version()}{mode().render_query()}&screen={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == PreviewMode::Play {
                            span {
                                "Click buttons, links, and menus as a user would · a link to another screen opens it"
                            }
                        } else {
                            span {
                                "Click a node to select it and edit its text · ⌘-click adds more · right-click and Pin to chat references it · ⌘-click a tile to pin pages"
                            }
                            span { class: "dot", "·" }
                            span { "right-click for quick edits" }
                        }
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
                            "Design notes"
                            span { class: "screen-no", "screen {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "Intent, states, and handoff remarks for this screen. Never shown on the screen.",
                            oninput: move |event| {
                                let index = selected();
                                design
                                    .with_mut(|design| {
                                        if let Some(screen) = design.screens.get_mut(index) {
                                            screen.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Screens"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned screen. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the screens the outline still plans",
                                onclick: {
                                    let design_id = design_id.clone();
                                    move |_| {
                                        let design_id = design_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&design_id);
                                            let sent = api::continue_artifact(&session_id, &design_id).await;
                                            if let Err(message) = sent {
                                                messages.write().push(message);
                                            }
                                        });
                                    }
                                },
                                span { dangerous_inner_html: icons::PLAY }
                                span { "Write the {planned_count} planned" }
                            }
                        }
                    }
                    div { class: "thumbnails",
                        for (index, label) in thumbnail_labels.into_iter().enumerate() {
                            {
                                let is_deleting = pending_screen_delete() == Some(index);
                                let is_redoing = pending_redo() == Some(index);
                                let class = thumbnail_class(ThumbnailState {
                                    is_current: index == selected(),
                                    is_dragging: dragged() == Some(index),
                                    is_portrait,
                                    is_deleting,
                                    is_pinned: pinned().contains(&index),
                                });
                                rsx! {
                                    div {
                                        key: "{index}",
                                        class: "{class}",
                                        title: "{label} · drag to reorder · ⌘-click to pin for the chat",
                                        style: "--tile-width: {tile_width}rem",
                                        "data-index": "{index}",
                                        onclick: move |event: MouseEvent| {
                                            // A command-click pins the screen for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_screen_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Screen {index + 1}",
                                            tabindex: "-1",
                                            src: "/designs/{design_id}/render?version={preview_version()}&screen={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if screen_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this screen",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_screen_delete() == Some(index) {
                                                        pending_screen_delete.set(None);
                                                        design
                                                            .with_mut(|design| {
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
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the screen anew: the model
                                        // sees its name and notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this screen anew",
                                            onclick: {
                                                let design_id = design_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_screen_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let design_id = design_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&design_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &design_id,
                                                                "screen",
                                                                index + 1,
                                                            )
                                                            .await;
                                                        if let Err(message) = sent {
                                                            messages.write().push(message);
                                                        }
                                                    });
                                                }
                                            },
                                            span { dangerous_inner_html: icons::REDO }
                                            if is_redoing {
                                                span { class: "redo-text", "redo?" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if outline_count > screen_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in screen_count..outline_count {
                            {
                                let (number, title) = outline_entry(&design().outline, index, "Screen");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: if is_portrait { "thumbnail outline portrait" } else { "thumbnail outline" },
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&design(), index)} · not written yet",
                                        span { class: "outline-kicker",
                                            "{number}"
                                            span { class: "planned-text", " planned" }
                                        }
                                        span { class: "outline-title", "{title}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if show_properties() {
                aside { class: "properties-sheet",
                    div { class: "sheet-head",
                        span { class: "kicker", "Properties" }
                        span { class: "spacer" }
                        // The same cue as the toolbar.
                        if is_dirty() {
                            span { class: "save-state pending", "saving…" }
                        }
                        button { onclick: move |_| show_properties.set(false), "Close" }
                    }
                    div { class: "sheet-body",
                        NodeInspector {
                            screen: selected(),
                            node: selected_node(),
                            on_apply: move |command: NodeCommand| apply.call(command),
                            scope: artifact_project(&design_id),
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
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                        }
                        ThemeForm {
                            theme: design().theme.clone(),
                            on_change: move |theme: Theme| {
                                design.with_mut(|design| design.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        TransitionForm {
                            default_cut: true,
                            transition: design().transition,
                            on_change: move |transition: Option<Transition>| {
                                design.with_mut(|design| design.transition = transition);
                                is_dirty.set(true);
                                save.call(false);
                            },
                        }
                        HistorySection {
                            design_id: design_id.clone(),
                            kind: ArtifactKind::Demo,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}
