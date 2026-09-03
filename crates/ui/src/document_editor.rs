//! The document editor: a chat column, a live page preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The document twin of `deck_editor.rs`. It shares the preview bridge,
//! the node inspector, the theme form, and the history section with
//! the design editor; what differs is the document type, the
//! `/documents` routes, the page vocabulary, the paper control, and
//! the exports: PDF and DOCX beside the HTML file.

use design_model::{ArtifactKind, Document, Page, Paper, Theme};
use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::canvas::frame_width_rem;
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

/// How the document preview takes a click: as a selection, or as a
/// reader of the document would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocumentPreviewMode {
    /// A click reaches the page as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl DocumentPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so a document opens as its reader sees it.
    pub(crate) const ALL: [DocumentPreviewMode; 2] =
        [DocumentPreviewMode::Read, DocumentPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            DocumentPreviewMode::Read => "Read",
            DocumentPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            DocumentPreviewMode::Read => "See the page as a reader would",
            DocumentPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the page shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            DocumentPreviewMode::Read => "",
            DocumentPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one document, then hands it to the editor.
#[component]
pub fn DocumentEditor(document_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = document_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_document(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(document)) => rsx! {
            LoadedDocumentEditor { document_id, initial: document.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading document…" }
        },
    }
}

/// The editor for a loaded document: chat on the left, preview and
/// thumbnails on the right, the properties sheet on demand.
#[component]
fn LoadedDocumentEditor(
    document_id: String,
    initial: Document,
    on_back: EventHandler<()>,
) -> Element {
    let mut document = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| DocumentPreviewMode::Read);
    // The pages pinned for the chat with a command-click on a tile.
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
    let mut pending_page_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = document_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_document_user_paths(&id).await {
                user_paths.set(paths);
            }
        }
    });
    // Until the settings answer, the PDF button stays enabled: the
    // server answers 503 with the install hint when Chrome is missing.
    let settings = use_resource(|| async { api::fetch_settings().await.ok() });
    let can_export_with_chrome = settings().flatten().is_none_or(|view| view.has_chrome);

    let save = use_callback({
        let document_id = document_id.clone();
        move |reload_preview: bool| {
            let id = document_id.clone();
            let snapshot = document();
            spawn(async move {
                match api::save_document(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_document_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let document_id = document_id.clone();
        move |_: ()| {
            let id = document_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_document(&id).await {
                    document.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_document_user_paths(&id).await {
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
                    let count = document.peek().pages.len();
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
                "swift-design-select" if message.screen < document.peek().pages.len() => {
                    selected.set(message.screen);
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed = document.with_mut(|document| {
                        match document.pages.get_mut(message.screen) {
                            Some(page) if page.html != html => {
                                page.html = html;
                                true
                            }
                            _ => false,
                        }
                    });
                    if is_changed {
                        is_dirty.set(true);
                    }
                    if message.save && is_dirty() {
                        save.call(false);
                    }
                }
                "swift-design-drag" => match message.action.as_deref() {
                    Some("start") if message.screen < document.peek().pages.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < document.peek().pages.len()
                        {
                            document
                                .with_mut(|document| move_screen(&mut document.pages, from, to));
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
                            "page",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference("page", message.screen, &node),
                            None => format!("[page {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = document.with_mut(|document| {
                            if document.pages.len() > 1 && message.screen < document.pages.len() {
                                document.pages.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = document.peek().pages.len();
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

    let page_count = document().pages.len();
    let outline_count = document().outline.len();
    // Every page shares the paper, so every tile has the same width.
    let viewport = document().viewport();
    let tile_width = frame_width_rem(viewport, STRIP_TILE_HEIGHT_REM);
    let page_ratio = viewport.aspect_ratio_css();
    let planned_count = outline_count.saturating_sub(page_count);
    let summary = strip_summary(page_count, planned_count);
    let total_fields = field_count(&document());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = document()
        .pages
        .iter()
        .enumerate()
        .map(|(index, page)| page_label(index, page))
        .collect();
    let page_labels = thumbnail_labels.clone();
    let current_notes = document()
        .pages
        .get(selected())
        .and_then(|page| page.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: document_id.clone(),
                context: chat_context,
                page: page_reference("page", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: page_labels.clone(),
                page_unit: Some("page".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {page_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in DocumentPreviewMode::ALL {
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
                        DocumentExportGroup {
                            document_id: document_id.clone(),
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
                                            let document_id = document_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let document_id = document_id.clone();
                                                spawn(async move {
                                                    match api::save_document_template(&document_id, &name).await {
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
                                    title: "Keep this document's theme and layout style for a future design, deck, or document",
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
                    // A page is taller than wide, so the stage limits it
                    // by height like a phone canvas, without the bezel.
                    div { class: "preview-stage narrow",
                        PreviewFrame {
                            title: "Document preview",
                            ratio: "{page_ratio}",
                            src: "/documents/{document_id}/render?version={preview_version()}{mode().render_query()}&page={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == DocumentPreviewMode::Read {
                            span { "The page as a reader sees it · switch to Edit to select a node" }
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
                            " change pages"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Author notes"
                            span { class: "screen-no", "page {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "Sources, intent, or handoff remarks. Never shown on the page.",
                            oninput: move |event| {
                                let index = selected();
                                document
                                    .with_mut(|document| {
                                        if let Some(page) = document.pages.get_mut(index) {
                                            page.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Pages"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned page. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the pages the outline still plans",
                                onclick: {
                                    let document_id = document_id.clone();
                                    move |_| {
                                        let document_id = document_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&document_id);
                                            let sent = api::continue_artifact(&session_id, &document_id).await;
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
                                let is_deleting = pending_page_delete() == Some(index);
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
                                            // A command-click pins the page for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_page_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Page {index + 1}",
                                            tabindex: "-1",
                                            src: "/documents/{document_id}/render?version={preview_version()}&page={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if page_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this page",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_page_delete() != Some(index) {
                                                        pending_page_delete.set(Some(index));
                                                        return;
                                                    }
                                                    pending_page_delete.set(None);
                                                    document.with_mut(|document| remove_page(document, index));
                                                    selected.set(selected().min(page_count.saturating_sub(2)));
                                                    selected_node.set(None);
                                                    save.call(true);
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the page anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this page anew",
                                            onclick: {
                                                let document_id = document_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_page_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let document_id = document_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&document_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &document_id,
                                                                "page",
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
                        if outline_count > page_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in page_count..outline_count {
                            {
                                let (number, title) = outline_entry(&document().outline, index, "Page");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&document(), index)} · not written yet",
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
                            scope: artifact_project(&document_id),
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Document" }
                            label {
                                "Document title"
                                input {
                                    value: "{document().title}",
                                    oninput: move |event| {
                                        document.with_mut(|document| document.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                            label {
                                "Paper"
                                select {
                                    value: "{document().paper.as_str()}",
                                    onchange: move |event| {
                                        let Some(paper) = Paper::from_name(&event.value()) else {
                                            return;
                                        };
                                        document.with_mut(|document| document.paper = paper);
                                        is_dirty.set(true);
                                        // The canvas changes, so the preview
                                        // reloads on save.
                                        schedule_save(save_generation, save, true);
                                    },
                                    for paper in Paper::ALL {
                                        option {
                                            key: "{paper.as_str()}",
                                            value: "{paper.as_str()}",
                                            selected: document().paper == paper,
                                            "{paper_option_label(paper)}"
                                        }
                                    }
                                }
                            }
                        }
                        ThemeForm {
                            theme: document().theme.clone(),
                            on_change: move |theme: Theme| {
                                document.with_mut(|document| document.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        HistorySection {
                            design_id: document_id.clone(),
                            kind: ArtifactKind::Document,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// The document toolbar's export group: the HTML file, the
/// Chrome-backed PDF, and the DOCX file.
#[component]
fn DocumentExportGroup(document_id: String, can_export_with_chrome: bool) -> Element {
    rsx! {
        div { class: "export-group",
            a {
                class: "button",
                href: "/documents/{document_id}/export",
                title: "Export as one HTML file",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "HTML"
            }
            ChromeExportLink {
                href: format!("/documents/{document_id}/export.pdf"),
                label: "PDF",
                title: "Export as a PDF, one sheet per page",
                is_enabled: can_export_with_chrome,
            }
            a {
                class: "button",
                href: "/documents/{document_id}/export.docx",
                title: "Export as a Word file",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "DOCX"
            }
        }
    }
}

/// One export link that needs Chrome on the server: a link when Chrome
/// is there, a disabled cell with the install hint otherwise.
#[component]
fn ChromeExportLink(
    href: String,
    label: &'static str,
    title: &'static str,
    is_enabled: bool,
) -> Element {
    if is_enabled {
        return rsx! {
            a { class: "button", href: "{href}", title: "{title}",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "{label}"
            }
        };
    }
    rsx! {
        span {
            class: "export-cell",
            title: "Install Chrome or Chromium on the server machine, or set SWIFT_DESIGN_CHROME",
            a { class: "button", "aria-disabled": "true",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "{label}"
            }
        }
    }
}

/// Removes page `index`, unless it is the last page: a document keeps
/// at least one page.
fn remove_page(document: &mut Document, index: usize) {
    if document.pages.len() > 1 && index < document.pages.len() {
        document.pages.remove(index);
    }
}

/// The paper option text: the name and the px canvas.
fn paper_option_label(paper: Paper) -> String {
    let viewport = paper.viewport();
    format!(
        "{} · {} × {}",
        paper.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the page, else `Page N`.
fn page_label(index: usize, page: &Page) -> String {
    fragment_label("Page", index, &page.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Page 5` when the outline has no entry there.
fn outline_title(document: &Document, index: usize) -> String {
    match document.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Page {}", index + 1, index + 1),
    }
}

/// A sample page for tests: a heading and a paragraph.
#[cfg(test)]
fn default_page() -> Page {
    Page {
        html: "<div class='body'><h2>New page</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 28px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the document. Matches the server's
/// provenance paths: absent optional fields are not counted.
fn field_count(document: &Document) -> usize {
    // Document title, paper, theme name, four colors, three fonts.
    let mut count = 10;
    for page in &document.pages {
        count += 1 + usize::from(page.css.is_some()) + usize::from(page.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Document, FontSet, Page, Palette, Paper, Theme};

    use super::{
        DocumentPreviewMode, default_page, field_count, outline_title, page_label,
        paper_option_label,
    };

    #[test]
    fn a_document_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(DocumentPreviewMode::ALL[0], DocumentPreviewMode::Read);
        assert_eq!(
            DocumentPreviewMode::ALL.map(DocumentPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(DocumentPreviewMode::Read.render_query(), "");
        assert_eq!(DocumentPreviewMode::Edit.render_query(), "&editable=true");
    }

    fn document() -> Document {
        Document {
            title: "T".to_owned(),
            theme: Theme {
                name: "n".to_owned(),
                colors: Palette {
                    background: "#ffffff".to_owned(),
                    text: "#1a1d21".to_owned(),
                    accent: "#0e6e63".to_owned(),
                    muted: "#888888".to_owned(),
                },
                fonts: FontSet {
                    heading: "Inter".to_owned(),
                    body: "Inter".to_owned(),
                    mono: "Menlo".to_owned(),
                },
            },
            paper: Paper::A4,
            pages: vec![
                Page {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Open".to_owned()),
                },
                default_page(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn page_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(page_label(0, &default_page()), "1. New page");
        let document = document();
        assert_eq!(page_label(0, &document.pages[0]), "1. Swift Design");
        let empty = Page {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(page_label(1, &empty), "2. Page 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_page_number() {
        let mut planned = document();
        planned.outline = vec!["Intro".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Intro");
        assert_eq!(outline_title(&planned, 1), "2. Page 2");
        assert_eq!(outline_title(&planned, 5), "6. Page 6");
    }

    #[test]
    fn the_default_page_validates_inside_a_document() {
        let document = document();
        assert_eq!(document.validate(), Vec::new());
        // 10 document and theme fields, html and notes on the first
        // page, html and css on the second.
        assert_eq!(field_count(&document), 10 + 2 + 2);
    }

    #[test]
    fn paper_options_name_the_paper_and_its_canvas() {
        assert_eq!(paper_option_label(Paper::A4), "A4 · 794 × 1123");
        assert_eq!(paper_option_label(Paper::Letter), "Letter · 816 × 1056");
    }
}
