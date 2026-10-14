//! The print editor: a chat column, a live sheet preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The print twin of `social_editor.rs`. It shares the preview
//! bridge, the node inspector, the theme form, and the history section
//! with the design editor; what differs is the print type, the
//! `/prints` routes, the sheet vocabulary, the size and orientation
//! controls, and the exports: PDF and a PNG zip beside the HTML file.

use design_model::{ArtifactKind, Orientation, Print, PrintSize, Sheet, Theme};
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

/// How the print preview takes a click: as a selection, or as a
/// reader of the printed piece would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrintPreviewMode {
    /// A click reaches the sheet as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl PrintPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so a print opens as its reader sees it.
    pub(crate) const ALL: [PrintPreviewMode; 2] = [PrintPreviewMode::Read, PrintPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            PrintPreviewMode::Read => "Read",
            PrintPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            PrintPreviewMode::Read => "See the sheet as a reader would",
            PrintPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the sheet shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            PrintPreviewMode::Read => "",
            PrintPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one print, then hands it to the editor.
#[component]
pub fn PrintEditor(print_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = print_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_print(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(print)) => rsx! {
            LoadedPrintEditor { print_id, initial: print.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading print…" }
        },
    }
}

/// The editor for a loaded print: chat on the left, preview and
/// thumbnails on the right, the properties sheet on demand.
#[component]
fn LoadedPrintEditor(print_id: String, initial: Print, on_back: EventHandler<()>) -> Element {
    let mut print = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| PrintPreviewMode::Read);
    // The sheets pinned for the chat with a command-click on a tile.
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
    let mut pending_sheet_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = print_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_print_user_paths(&id).await {
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
        let print_id = print_id.clone();
        move |reload_preview: bool| {
            let id = print_id.clone();
            let snapshot = print();
            spawn(async move {
                match api::save_print(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_print_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let print_id = print_id.clone();
        move |_: ()| {
            let id = print_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_print(&id).await {
                    print.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_print_user_paths(&id).await {
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
                    let count = print.peek().sheets.len();
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
                "swift-design-select" if message.screen < print.peek().sheets.len() => {
                    selected.set(message.screen);
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        print.with_mut(|print| match print.sheets.get_mut(message.screen) {
                            Some(sheet) if sheet.html != html => {
                                sheet.html = html;
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
                    Some("start") if message.screen < print.peek().sheets.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < print.peek().sheets.len()
                        {
                            print.with_mut(|print| {
                                move_screen(&mut print.sheets, &mut print.outline, from, to)
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
                            "sheet",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference("sheet", message.screen, &node),
                            None => format!("[sheet {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = print.with_mut(|print| {
                            if print.sheets.len() > 1 && message.screen < print.sheets.len() {
                                print.sheets.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = print.peek().sheets.len();
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

    let sheet_count = print().sheets.len();
    let outline_count = print().outline.len();
    // Every sheet shares the size and orientation, so every tile has
    // the same width.
    let viewport = print().viewport();
    let tile_width = frame_width_rem(viewport, STRIP_TILE_HEIGHT_REM);
    let sheet_ratio = viewport.aspect_ratio_css();
    let stage_class = preview_stage_class(viewport);
    let planned_count = outline_count.saturating_sub(sheet_count);
    let summary = strip_summary(sheet_count, planned_count);
    let total_fields = field_count(&print());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = print()
        .sheets
        .iter()
        .enumerate()
        .map(|(index, sheet)| sheet_label(index, sheet))
        .collect();
    let sheet_labels = thumbnail_labels.clone();
    let current_notes = print()
        .sheets
        .get(selected())
        .and_then(|sheet| sheet.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: print_id.clone(),
                context: chat_context,
                page: page_reference("sheet", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: sheet_labels.clone(),
                page_unit: Some("sheet".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {sheet_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in PrintPreviewMode::ALL {
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
                        PrintExportGroup {
                            print_id: print_id.clone(),
                            selected: selected(),
                            sheet_count,
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
                                            let print_id = print_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let print_id = print_id.clone();
                                                spawn(async move {
                                                    match api::save_print_template(&print_id, &name).await {
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
                                    title: "Keep this print's theme and layout style for a future design, deck, document, or social",
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
                    // Every print size, portrait or landscape, is
                    // narrower than the 16:10 stage, so a sheet is
                    // limited by height like a phone canvas, without
                    // the bezel.
                    div { class: "{stage_class}",
                        PreviewFrame {
                            title: "Print preview",
                            ratio: "{sheet_ratio}",
                            src: "/prints/{print_id}/render?version={preview_version()}{mode().render_query()}&sheet={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == PrintPreviewMode::Read {
                            span { "The sheet as a reader sees it · switch to Edit to select a node" }
                        } else {
                            span {
                                "Click a node to select it and edit its text · ⌘-click adds more · right-click and Pin to chat references it · ⌘-click a tile to pin sheets"
                            }
                            span { class: "dot", "·" }
                            span { "right-click for quick edits" }
                        }
                        span { class: "dot", "·" }
                        span {
                            kbd { "←" }
                            " "
                            kbd { "→" }
                            " change sheets"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Author notes"
                            span { class: "screen-no", "sheet {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "Print instructions such as paper stock or bleed, intent, or handoff remarks. Never shown on the sheet.",
                            oninput: move |event| {
                                let index = selected();
                                print
                                    .with_mut(|print| {
                                        if let Some(sheet) = print.sheets.get_mut(index) {
                                            sheet.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Sheets"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned sheet. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the sheets the outline still plans",
                                onclick: {
                                    let print_id = print_id.clone();
                                    move |_| {
                                        let print_id = print_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&print_id);
                                            let sent = api::continue_artifact(&session_id, &print_id).await;
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
                                let is_deleting = pending_sheet_delete() == Some(index);
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
                                            // A command-click pins the sheet for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_sheet_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Sheet {index + 1}",
                                            tabindex: "-1",
                                            src: "/prints/{print_id}/render?version={preview_version()}&sheet={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if sheet_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this sheet",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_sheet_delete() != Some(index) {
                                                        pending_sheet_delete.set(Some(index));
                                                        return;
                                                    }
                                                    pending_sheet_delete.set(None);
                                                    print.with_mut(|print| remove_sheet(print, index));
                                                    selected.set(selected().min(sheet_count.saturating_sub(2)));
                                                    selected_node.set(None);
                                                    save.call(true);
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the sheet anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this sheet anew",
                                            onclick: {
                                                let print_id = print_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_sheet_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let print_id = print_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&print_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &print_id,
                                                                "sheet",
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
                        if outline_count > sheet_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in sheet_count..outline_count {
                            {
                                let (number, title) = outline_entry(&print().outline, index, "Sheet");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&print(), index)} · not written yet",
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
                            scope: artifact_project(&print_id),
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Print" }
                            label {
                                "Print title"
                                input {
                                    value: "{print().title}",
                                    oninput: move |event| {
                                        print.with_mut(|print| print.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                            // The size and the orientation share one row:
                            // together they are the canvas.
                            div { class: "theme-grid",
                                label {
                                    "Size"
                                    select {
                                        value: "{print().size.as_str()}",
                                        onchange: move |event| {
                                            let Some(size) = PrintSize::from_name(&event.value()) else {
                                                return;
                                            };
                                            print.with_mut(|print| print.size = size);
                                            is_dirty.set(true);
                                            // The canvas changes, so the preview
                                            // reloads on save.
                                            schedule_save(save_generation, save, true);
                                        },
                                        for size in PrintSize::ALL {
                                            option {
                                                key: "{size.as_str()}",
                                                value: "{size.as_str()}",
                                                selected: print().size == size,
                                                "{size_option_label(size)}"
                                            }
                                        }
                                    }
                                }
                                label {
                                    "Orientation"
                                    select {
                                        value: "{print().orientation.as_str()}",
                                        onchange: move |event| {
                                            let Some(orientation) = Orientation::from_name(&event.value()) else {
                                                return;
                                            };
                                            print.with_mut(|print| print.orientation = orientation);
                                            is_dirty.set(true);
                                            // The canvas changes, so the preview
                                            // reloads on save.
                                            schedule_save(save_generation, save, true);
                                        },
                                        for orientation in Orientation::ALL {
                                            option {
                                                key: "{orientation.as_str()}",
                                                value: "{orientation.as_str()}",
                                                selected: print().orientation == orientation,
                                                "{orientation.label()}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        ThemeForm {
                            theme: print().theme.clone(),
                            on_change: move |theme: Theme| {
                                print.with_mut(|print| print.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        HistorySection {
                            design_id: print_id.clone(),
                            kind: ArtifactKind::Print,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}
