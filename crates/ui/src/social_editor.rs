//! The social editor: a chat column, a live frame preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The social twin of `document_editor.rs`. It shares the preview
//! bridge, the node inspector, the theme form, and the history section
//! with the design editor; what differs is the social type, the
//! `/socials` routes, the frame vocabulary, the format control, and
//! the exports: PDF and a PNG zip beside the HTML file.

use design_model::{ArtifactKind, Format, Frame, Social, Theme};
use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::canvas::{frame_width_rem, is_narrow_canvas};
use crate::chat::DesignChat;
use crate::editor::{
    APPLY_TO_PREVIEW, HistorySection, NodeCommand, NodeInspector, PREVIEW_LISTENER, PreviewFrame,
    PreviewMessage, STRIP_TILE_HEIGHT_REM, SelectedNode, SelectionEntry, ThemeForm, ThumbnailState,
    fragment_label, move_screen, node_reference, optional, outline_entry, page_reference,
    pin_reference, schedule_save, selection_of, selection_paths, strip_summary, thumbnail_class,
    toggle_pin,
};
use crate::icons;
use crate::settings::artifact_project;

mod support;

use support::*;

/// How the social preview takes a click: as a selection, or as a
/// reader of the feed would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SocialPreviewMode {
    /// A click reaches the frame as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl SocialPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so a social opens as its reader sees it.
    pub(crate) const ALL: [SocialPreviewMode; 2] =
        [SocialPreviewMode::Read, SocialPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SocialPreviewMode::Read => "Read",
            SocialPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            SocialPreviewMode::Read => "See the frame as a reader would",
            SocialPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the frame shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            SocialPreviewMode::Read => "",
            SocialPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one social, then hands it to the editor.
#[component]
pub fn SocialEditor(social_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = social_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_social(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(social)) => rsx! {
            LoadedSocialEditor { social_id, initial: social.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading social…" }
        },
    }
}

/// The editor for a loaded social: chat on the left, preview and
/// thumbnails on the right, the properties sheet on demand.
#[component]
fn LoadedSocialEditor(social_id: String, initial: Social, on_back: EventHandler<()>) -> Element {
    let mut social = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| SocialPreviewMode::Read);
    // The frames pinned for the chat with a command-click on a tile.
    let mut pinned = use_signal(Vec::<usize>::new);
    let mut messages = use_signal(Vec::<String>::new);
    let mut preview_version = use_signal(|| 0u32);
    let mut user_paths = use_signal(Vec::<String>::new);
    let mut is_dirty = use_signal(|| false);
    // Autosave: the newest change wins the delay.
    let save_generation = use_signal(|| 0u64);
    let mut show_properties = use_signal(|| false);
    let mut is_menu_open = use_signal(|| false);
    let mut template_name = use_signal(|| Option::<String>::None);
    let mut chat_context = use_signal(|| Option::<String>::None);
    let mut dragged = use_signal(|| Option::<usize>::None);
    let mut pending_frame_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = social_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_social_user_paths(&id).await {
                user_paths.set(paths);
            }
        }
    });
    // Until the settings answer, the PDF and PNG buttons stay enabled:
    // the server answers 503 with the install hint when Chrome is
    // missing.
    let settings = use_resource(|| async { api::fetch_settings().await.ok() });
    let can_export_with_chrome = settings().flatten().is_none_or(|view| view.has_chrome);

    let save = use_callback({
        let social_id = social_id.clone();
        move |reload_preview: bool| {
            let id = social_id.clone();
            let snapshot = social();
            spawn(async move {
                match api::save_social(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_social_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let social_id = social_id.clone();
        move |_: ()| {
            let id = social_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_social(&id).await {
                    social.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_social_user_paths(&id).await {
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
                    let count = social.peek().frames.len();
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
                "swift-design-select" if message.screen < social.peek().frames.len() => {
                    selected.set(message.screen);
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        social.with_mut(|social| match social.frames.get_mut(message.screen) {
                            Some(frame) if frame.html != html => {
                                frame.html = html;
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
                    Some("start") if message.screen < social.peek().frames.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < social.peek().frames.len()
                        {
                            social.with_mut(|social| {
                                move_screen(&mut social.frames, &mut social.outline, from, to)
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
                            "frame",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference("frame", message.screen, &node),
                            None => format!("[frame {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = social.with_mut(|social| {
                            if social.frames.len() > 1 && message.screen < social.frames.len() {
                                social.frames.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = social.peek().frames.len();
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

    // One command per selected node: see the design editor.
    let apply = use_callback(move |command: NodeCommand| {
        for path in selection_paths(&selection.peek(), &command.path) {
            let channel = document::eval(APPLY_TO_PREVIEW);
            let _ = channel.send(NodeCommand {
                path,
                ..command.clone()
            });
        }
    });

    let frame_count = social().frames.len();
    let outline_count = social().outline.len();
    // Every frame shares the format, so every tile has the same width.
    let viewport = social().viewport();
    let tile_width = frame_width_rem(viewport, STRIP_TILE_HEIGHT_REM);
    let frame_ratio = viewport.aspect_ratio_css();
    let stage_class = preview_stage_class(viewport);
    let planned_count = outline_count.saturating_sub(frame_count);
    let summary = strip_summary(frame_count, planned_count);
    let total_fields = field_count(&social());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = social()
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| frame_label(index, frame))
        .collect();
    let frame_labels = thumbnail_labels.clone();
    let current_notes = social()
        .frames
        .get(selected())
        .and_then(|frame| frame.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: social_id.clone(),
                context: chat_context,
                page: page_reference("frame", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: frame_labels.clone(),
                page_unit: Some("frame".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {frame_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in SocialPreviewMode::ALL {
                            button {
                                key: "{candidate.label()}",
                                role: "tab",
                                class: if mode() == candidate { "canvas-tab open" } else { "canvas-tab" },
                                title: "{candidate.title()}",
                                onclick: move |_| {
                                    mode.set(candidate);
                                    selected_node.set(None);
                                },
                                span { class: "tab-name", "{candidate.label()}" }
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
                        SocialExportGroup {
                            social_id: social_id.clone(),
                            selected: selected(),
                            frame_count,
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
                                            let social_id = social_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let social_id = social_id.clone();
                                                spawn(async move {
                                                    match api::save_social_template(&social_id, &name).await {
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
                                    title: "Keep this social's theme and layout style for a future design, deck, document, or social",
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
                    // A square, portrait, or story frame is limited by
                    // height like a phone canvas, without the bezel. A
                    // landscape frame is wide, so it fills the width.
                    div { class: "{stage_class}",
                        PreviewFrame {
                            title: "Social preview",
                            ratio: "{frame_ratio}",
                            src: "/socials/{social_id}/render?version={preview_version()}{mode().render_query()}&frame={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == SocialPreviewMode::Read {
                            span { "The frame as a reader sees it · switch to Edit to select a node" }
                        } else {
                            span {
                                "Click a node to select it and edit its text · ⌘-click adds more · right-click and Pin to chat references it · ⌘-click a tile to pin frames"
                            }
                            span { class: "dot", "·" }
                            span { "right-click for quick edits" }
                        }
                        span { class: "dot", "·" }
                        span {
                            kbd { "←" }
                            " "
                            kbd { "→" }
                            " change frames"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Author notes"
                            span { class: "screen-no", "frame {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "The caption to post with the frame, intent, or handoff remarks. Never shown on the frame.",
                            oninput: move |event| {
                                let index = selected();
                                social
                                    .with_mut(|social| {
                                        if let Some(frame) = social.frames.get_mut(index) {
                                            frame.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Frames"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned frame. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the frames the outline still plans",
                                onclick: {
                                    let social_id = social_id.clone();
                                    move |_| {
                                        let social_id = social_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&social_id);
                                            let sent = api::continue_artifact(&session_id, &social_id).await;
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
                                let is_deleting = pending_frame_delete() == Some(index);
                                let is_redoing = pending_redo() == Some(index);
                                let class = thumbnail_class(ThumbnailState {
                                    is_current: index == selected(),
                                    is_dragging: dragged() == Some(index),
                                    is_portrait: false,
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
                                            // A command-click pins the frame for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_frame_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Frame {index + 1}",
                                            tabindex: "-1",
                                            src: "/socials/{social_id}/render?version={preview_version()}&frame={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if frame_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this frame",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_frame_delete() != Some(index) {
                                                        pending_frame_delete.set(Some(index));
                                                        return;
                                                    }
                                                    pending_frame_delete.set(None);
                                                    social.with_mut(|social| remove_frame(social, index));
                                                    selected.set(selected().min(frame_count.saturating_sub(2)));
                                                    selected_node.set(None);
                                                    save.call(true);
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the frame anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this frame anew",
                                            onclick: {
                                                let social_id = social_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_frame_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let social_id = social_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&social_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &social_id,
                                                                "frame",
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
                        if outline_count > frame_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in frame_count..outline_count {
                            {
                                let (number, title) = outline_entry(&social().outline, index, "Frame");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&social(), index)} · not written yet",
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
                            scope: artifact_project(&social_id),
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Social" }
                            label {
                                "Social title"
                                input {
                                    value: "{social().title}",
                                    oninput: move |event| {
                                        social.with_mut(|social| social.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                            label {
                                "Format"
                                select {
                                    value: "{social().format.as_str()}",
                                    onchange: move |event| {
                                        let Some(format) = Format::from_name(&event.value()) else {
                                            return;
                                        };
                                        social.with_mut(|social| social.format = format);
                                        is_dirty.set(true);
                                        // The canvas changes, so the preview
                                        // reloads on save.
                                        schedule_save(save_generation, save, true);
                                    },
                                    for format in Format::ALL {
                                        option {
                                            key: "{format.as_str()}",
                                            value: "{format.as_str()}",
                                            selected: social().format == format,
                                            "{format_option_label(format)}"
                                        }
                                    }
                                }
                            }
                        }
                        ThemeForm {
                            theme: social().theme.clone(),
                            on_change: move |theme: Theme| {
                                social.with_mut(|social| social.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        HistorySection {
                            design_id: social_id.clone(),
                            kind: ArtifactKind::Social,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}
