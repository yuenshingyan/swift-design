//! The artwork editor: a chat column, a live cover preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The artwork twin of `mailing_editor.rs`. It shares the preview
//! bridge, the node inspector, the theme form, and the history section
//! with the design editor; what differs is the artwork type, the
//! `/artworks` routes, the cover vocabulary, the size control, and the
//! exports: PDF and a PNG zip beside the HTML file.

use design_model::{ArtifactKind, Artwork, Cover, CoverSize, Theme};
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

/// How the artwork preview takes a click: as a selection, or as a
/// reader in the inbox would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtworkPreviewMode {
    /// A click reaches the cover as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl ArtworkPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so an artwork opens as its reader sees it.
    pub(crate) const ALL: [ArtworkPreviewMode; 2] =
        [ArtworkPreviewMode::Read, ArtworkPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ArtworkPreviewMode::Read => "Read",
            ArtworkPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            ArtworkPreviewMode::Read => "See the cover as a reader would",
            ArtworkPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the cover shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            ArtworkPreviewMode::Read => "",
            ArtworkPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one artwork, then hands it to the editor.
#[component]
pub fn ArtworkEditor(artwork_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = artwork_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_artwork(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(artwork)) => rsx! {
            LoadedArtworkEditor { artwork_id, initial: artwork.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading artwork…" }
        },
    }
}

/// The editor for a loaded artwork: chat on the left, preview and
/// thumbnails on the right, the properties cover on demand.
#[component]
fn LoadedArtworkEditor(artwork_id: String, initial: Artwork, on_back: EventHandler<()>) -> Element {
    let mut artwork = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| ArtworkPreviewMode::Read);
    // The covers pinned for the chat with a command-click on a tile.
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
    let mut pending_cover_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = artwork_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_artwork_user_paths(&id).await {
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
        let artwork_id = artwork_id.clone();
        move |reload_preview: bool| {
            let id = artwork_id.clone();
            let snapshot = artwork();
            spawn(async move {
                match api::save_artwork(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_artwork_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let artwork_id = artwork_id.clone();
        move |_: ()| {
            let id = artwork_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_artwork(&id).await {
                    artwork.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_artwork_user_paths(&id).await {
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
                    let count = artwork.peek().covers.len();
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
                "swift-design-select" if message.screen < artwork.peek().covers.len() => {
                    selected.set(message.screen);
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        artwork.with_mut(|artwork| match artwork.covers.get_mut(message.screen) {
                            Some(cover) if cover.html != html => {
                                cover.html = html;
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
                    Some("start") if message.screen < artwork.peek().covers.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < artwork.peek().covers.len()
                        {
                            artwork.with_mut(|artwork| {
                                move_screen(&mut artwork.covers, &mut artwork.outline, from, to)
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
                            "cover",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference("cover", message.screen, &node),
                            None => format!("[cover {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = artwork.with_mut(|artwork| {
                            if artwork.covers.len() > 1 && message.screen < artwork.covers.len() {
                                artwork.covers.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = artwork.peek().covers.len();
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

    let cover_count = artwork().covers.len();
    let outline_count = artwork().outline.len();
    // Every cover shares the size, so every tile has the same width.
    let viewport = artwork().viewport();
    let tile_width = strip_tile_width(viewport);
    let cover_ratio = viewport.aspect_ratio_css();
    let stage_class = preview_stage_class(viewport);
    let planned_count = outline_count.saturating_sub(cover_count);
    let summary = strip_summary(cover_count, planned_count);
    let total_fields = field_count(&artwork());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = artwork()
        .covers
        .iter()
        .enumerate()
        .map(|(index, cover)| cover_label(index, cover))
        .collect();
    let cover_labels = thumbnail_labels.clone();
    let current_notes = artwork()
        .covers
        .get(selected())
        .and_then(|cover| cover.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: artwork_id.clone(),
                context: chat_context,
                page: page_reference("cover", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: cover_labels.clone(),
                page_unit: Some("cover".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {cover_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in ArtworkPreviewMode::ALL {
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
                        ArtworkExportGroup {
                            artwork_id: artwork_id.clone(),
                            selected: selected(),
                            cover_count,
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
                                            let artwork_id = artwork_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let artwork_id = artwork_id.clone();
                                                spawn(async move {
                                                    match api::save_artwork_template(&artwork_id, &name).await {
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
                                    title: "Keep this artwork's theme and layout style for a future design, deck, document, or social",
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
                    // A tall cover such as the book is limited by
                    // height like a phone canvas, without the bezel; a
                    // wide header fills the width instead.
                    div { class: "{stage_class}",
                        PreviewFrame {
                            title: "Artwork preview",
                            ratio: "{cover_ratio}",
                            src: "/artworks/{artwork_id}/render?version={preview_version()}{mode().render_query()}&cover={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == ArtworkPreviewMode::Read {
                            span { "The cover as a reader sees it · switch to Edit to select a node" }
                        } else {
                            span {
                                "Click a node to select it and edit its text · ⌘-click adds more · right-click and Pin to chat references it · ⌘-click a tile to pin covers"
                            }
                            span { class: "dot", "·" }
                            span { "right-click for quick edits" }
                        }
                        span { class: "dot", "·" }
                        span {
                            kbd { "←" }
                            " "
                            kbd { "→" }
                            " change covers"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Author notes"
                            span { class: "screen-no", "cover {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "The title context and the alt text, as a Title: line and an Alt: line, plus intent or handoff remarks. Never shown on the cover.",
                            oninput: move |event| {
                                let index = selected();
                                artwork
                                    .with_mut(|artwork| {
                                        if let Some(cover) = artwork.covers.get_mut(index) {
                                            cover.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Covers"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned cover. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the covers the outline still plans",
                                onclick: {
                                    let artwork_id = artwork_id.clone();
                                    move |_| {
                                        let artwork_id = artwork_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&artwork_id);
                                            let sent = api::continue_artifact(&session_id, &artwork_id).await;
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
                                let is_deleting = pending_cover_delete() == Some(index);
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
                                            // A command-click pins the cover for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_cover_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Cover {index + 1}",
                                            tabindex: "-1",
                                            src: "/artworks/{artwork_id}/render?version={preview_version()}&cover={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if cover_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this cover",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_cover_delete() != Some(index) {
                                                        pending_cover_delete.set(Some(index));
                                                        return;
                                                    }
                                                    pending_cover_delete.set(None);
                                                    artwork.with_mut(|artwork| remove_cover(artwork, index));
                                                    selected.set(selected().min(cover_count.saturating_sub(2)));
                                                    selected_node.set(None);
                                                    save.call(true);
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the cover anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this cover anew",
                                            onclick: {
                                                let artwork_id = artwork_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_cover_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let artwork_id = artwork_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&artwork_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &artwork_id,
                                                                "cover",
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
                        if outline_count > cover_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in cover_count..outline_count {
                            {
                                let (number, title) = outline_entry(&artwork().outline, index, "Cover");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&artwork(), index)} · not written yet",
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
                            scope: artifact_project(&artwork_id),
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Artwork" }
                            label {
                                "Artwork title"
                                input {
                                    value: "{artwork().title}",
                                    oninput: move |event| {
                                        artwork.with_mut(|artwork| artwork.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                            label {
                                "Size"
                                select {
                                    value: "{artwork().size.as_str()}",
                                    onchange: move |event| {
                                        let Some(size) = CoverSize::from_name(&event.value()) else {
                                            return;
                                        };
                                        artwork.with_mut(|artwork| artwork.size = size);
                                        is_dirty.set(true);
                                        // The canvas changes, so the preview
                                        // reloads on save.
                                        schedule_save(save_generation, save, true);
                                    },
                                    for size in CoverSize::ALL {
                                        option {
                                            key: "{size.as_str()}",
                                            value: "{size.as_str()}",
                                            selected: artwork().size == size,
                                            "{size_option_label(size)}"
                                        }
                                    }
                                }
                            }
                        }
                        ThemeForm {
                            theme: artwork().theme.clone(),
                            on_change: move |theme: Theme| {
                                artwork.with_mut(|artwork| artwork.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        HistorySection {
                            design_id: artwork_id.clone(),
                            kind: ArtifactKind::Artwork,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}
