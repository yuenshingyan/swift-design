//! The deck editor: a chat column, a live slide preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The deck twin of `editor.rs`. It shares the preview bridge, the node
//! inspector, the theme and transition forms, and the history section
//! with the design editor; what differs is the deck type, the `/decks`
//! routes, the slide vocabulary, and two deck-only actions: the presenter
//! view and the PPTX export.

use design_model::{ArtifactKind, DECK_VIEWPORT, Deck, Slide, Theme, Transition};
use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::canvas::frame_width_rem;
use crate::chat::DesignChat;
use crate::editor::{
    APPLY_TO_PREVIEW, HistorySection, NodeCommand, NodeInspector, PREVIEW_LISTENER, PreviewMessage,
    STRIP_TILE_HEIGHT_REM, SelectedNode, SelectionEntry, ThemeForm, ThumbnailState, TransitionForm,
    fragment_label, move_screen, node_reference, optional, outline_entry, page_reference,
    pin_reference, schedule_save, selection_of, selection_paths, strip_summary, thumbnail_class,
    toggle_pin,
};
use crate::icons;
use crate::settings::artifact_project;

/// How the deck preview takes a click: as a selection, or as a reader
/// of the deck would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeckPreviewMode {
    /// A click reaches the slide as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl DeckPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so a deck opens as its reader sees it.
    pub(crate) const ALL: [DeckPreviewMode; 2] = [DeckPreviewMode::Read, DeckPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            DeckPreviewMode::Read => "Read",
            DeckPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            DeckPreviewMode::Read => "See the slide as a reader would",
            DeckPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the slide shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            DeckPreviewMode::Read => "",
            DeckPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one deck, then hands it to the editor.
#[component]
pub fn DeckEditor(deck_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = deck_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_deck(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(deck)) => rsx! {
            LoadedDeckEditor { deck_id, initial: deck.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading deck…" }
        },
    }
}

/// The editor for a loaded deck: chat on the left, preview and
/// thumbnails on the right, the properties sheet on demand.
#[component]
fn LoadedDeckEditor(deck_id: String, initial: Deck, on_back: EventHandler<()>) -> Element {
    let mut deck = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| DeckPreviewMode::Read);
    // The slides pinned for the chat with a command-click on a tile.
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
    let mut pending_slide_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = deck_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_deck_user_paths(&id).await {
                user_paths.set(paths);
            }
        }
    });
    // Until the settings answer, the PDF and PPTX buttons stay enabled:
    // the server answers 503 with the install hint when Chrome is missing.
    let settings = use_resource(|| async { api::fetch_settings().await.ok() });
    let can_export_with_chrome = settings().flatten().is_none_or(|view| view.has_chrome);

    let save = use_callback({
        let deck_id = deck_id.clone();
        move |reload_preview: bool| {
            let id = deck_id.clone();
            let snapshot = deck();
            spawn(async move {
                match api::save_deck(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_deck_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let deck_id = deck_id.clone();
        move |_: ()| {
            let id = deck_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_deck(&id).await {
                    deck.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_deck_user_paths(&id).await {
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
                    let count = deck.peek().slides.len();
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
                "swift-design-select" if message.screen < deck.peek().slides.len() => {
                    selected.set(message.screen);
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        deck.with_mut(|deck| match deck.slides.get_mut(message.screen) {
                            Some(slide) if slide.html != html => {
                                slide.html = html;
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
                    Some("start") if message.screen < deck.peek().slides.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < deck.peek().slides.len()
                        {
                            deck.with_mut(|deck| move_screen(&mut deck.slides, from, to));
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
                            "slide",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
                    Some("ask") => {
                        selected.set(message.screen);
                        chat_context.set(Some(match SelectedNode::from_message(&message) {
                            Some(node) => node_reference("slide", message.screen, &node),
                            None => format!("[slide {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = deck.with_mut(|deck| {
                            if deck.slides.len() > 1 && message.screen < deck.slides.len() {
                                deck.slides.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = deck.peek().slides.len();
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

    let slide_count = deck().slides.len();
    let outline_count = deck().outline.len();
    // A deck is always 16:9, so every tile has the same width.
    let tile_width = frame_width_rem(DECK_VIEWPORT, STRIP_TILE_HEIGHT_REM);
    let deck_ratio = DECK_VIEWPORT.aspect_ratio_css();
    let planned_count = outline_count.saturating_sub(slide_count);
    let summary = strip_summary(slide_count, planned_count);
    let total_fields = field_count(&deck());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = deck()
        .slides
        .iter()
        .enumerate()
        .map(|(index, slide)| slide_label(index, slide))
        .collect();
    let page_labels = thumbnail_labels.clone();
    let current_notes = deck()
        .slides
        .get(selected())
        .and_then(|slide| slide.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: deck_id.clone(),
                context: chat_context,
                page: page_reference("slide", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: page_labels.clone(),
                page_unit: Some("slide".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {slide_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in DeckPreviewMode::ALL {
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
                        DeckExportGroup {
                            deck_id: deck_id.clone(),
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
                                            let deck_id = deck_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let deck_id = deck_id.clone();
                                                spawn(async move {
                                                    match api::save_deck_template(&deck_id, &name).await {
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
                                    title: "Keep this deck's theme and layout style for a future design or deck",
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
                    div { class: "preview-stage",
                        iframe {
                            title: "Deck preview",
                            "data-preview": "true",
                            // Without the ratio the iframe falls back to
                            // its default height and the slide sits in a
                            // band of the deck's own background.
                            style: "aspect-ratio: {deck_ratio}",
                            src: "/decks/{deck_id}/render?version={preview_version()}{mode().render_query()}&slide={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == DeckPreviewMode::Read {
                            span { "The slide as a reader sees it · switch to Edit to select a node" }
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
                            " change slides"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Presenter notes"
                            span { class: "screen-no", "slide {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "What to say on this slide. Never shown on the slide.",
                            oninput: move |event| {
                                let index = selected();
                                deck.with_mut(|deck| {
                                    if let Some(slide) = deck.slides.get_mut(index) {
                                        slide.notes = optional(event.value());
                                    }
                                });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Slides"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned slide. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the slides the outline still plans",
                                onclick: {
                                    let deck_id = deck_id.clone();
                                    move |_| {
                                        let deck_id = deck_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&deck_id);
                                            let sent = api::continue_artifact(&session_id, &deck_id).await;
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
                                let is_deleting = pending_slide_delete() == Some(index);
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
                                            // A command-click pins the slide for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_slide_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Slide {index + 1}",
                                            tabindex: "-1",
                                            src: "/decks/{deck_id}/render?version={preview_version()}&slide={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if slide_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this slide",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_slide_delete() == Some(index) {
                                                        pending_slide_delete.set(None);
                                                        deck.with_mut(|deck| {
                                                            if deck.slides.len() > 1 && index < deck.slides.len() {
                                                                deck.slides.remove(index);
                                                            }
                                                        });
                                                        selected.set(selected().min(slide_count.saturating_sub(2)));
                                                        selected_node.set(None);
                                                        save.call(true);
                                                    } else {
                                                        pending_slide_delete.set(Some(index));
                                                    }
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the slide anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this slide anew",
                                            onclick: {
                                                let deck_id = deck_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_slide_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let deck_id = deck_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&deck_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &deck_id,
                                                                "slide",
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
                        if outline_count > slide_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in slide_count..outline_count {
                            {
                                let (number, title) = outline_entry(&deck().outline, index, "Slide");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&deck(), index)} · not written yet",
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
                            scope: artifact_project(&deck_id),
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Deck" }
                            label {
                                "Deck title"
                                input {
                                    value: "{deck().title}",
                                    oninput: move |event| {
                                        deck.with_mut(|deck| deck.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                        }
                        ThemeForm {
                            theme: deck().theme.clone(),
                            on_change: move |theme: Theme| {
                                deck.with_mut(|deck| deck.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        TransitionForm {
                            transition: deck().transition,
                            on_change: move |transition: Option<Transition>| {
                                deck.with_mut(|deck| deck.transition = transition);
                                is_dirty.set(true);
                                save.call(false);
                            },
                        }
                        HistorySection {
                            design_id: deck_id.clone(),
                            kind: ArtifactKind::Deck,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// The deck toolbar's export group: the presenter view, the HTML file,
/// and the two Chrome-backed exports, PDF and PPTX.
#[component]
fn DeckExportGroup(deck_id: String, selected: usize, can_export_with_chrome: bool) -> Element {
    rsx! {
        div { class: "export-group",
            a {
                class: "button",
                href: "/decks/{deck_id}/present?slide={selected + 1}",
                target: "_blank",
                title: "Present in a new window",
                span { dangerous_inner_html: icons::PLAY }
                "Present"
            }
            a {
                class: "button",
                href: "/decks/{deck_id}/export",
                title: "Export as one HTML file",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "HTML"
            }
            ChromeExportLink {
                href: format!("/decks/{deck_id}/export.pdf"),
                label: "PDF",
                title: "Export as a PDF",
                is_enabled: can_export_with_chrome,
            }
            ChromeExportLink {
                href: format!("/decks/{deck_id}/export.pptx"),
                label: "PPTX",
                title: "Export as a PowerPoint file",
                is_enabled: can_export_with_chrome,
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

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the slide, else `Slide N`.
fn slide_label(index: usize, slide: &Slide) -> String {
    fragment_label("Slide", index, &slide.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Slide 5` when the outline has no entry there.
fn outline_title(deck: &Deck, index: usize) -> String {
    match deck.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Slide {}", index + 1, index + 1),
    }
}

/// A sample slide for tests: a heading and a paragraph.
#[cfg(test)]
fn default_slide() -> Slide {
    Slide {
        html: "<div class='body'><h2>New slide</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 120px; height: 100%; display: flex; flex-direction: column; gap: 40px; } h2 { font-size: 72px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the deck. Matches the server's
/// provenance paths: absent optional fields are not counted.
fn field_count(deck: &Deck) -> usize {
    // Deck title, theme name, four colors, three fonts.
    let mut count = 9;
    for slide in &deck.slides {
        count += 1 + usize::from(slide.css.is_some()) + usize::from(slide.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Deck, FontSet, Palette, Slide, Theme};

    use super::{DeckPreviewMode, default_slide, field_count, outline_title, slide_label};

    #[test]
    fn a_deck_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(DeckPreviewMode::ALL[0], DeckPreviewMode::Read);
        assert_eq!(
            DeckPreviewMode::ALL.map(DeckPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(DeckPreviewMode::Read.render_query(), "");
        assert_eq!(DeckPreviewMode::Edit.render_query(), "&editable=true");
    }

    fn deck() -> Deck {
        Deck {
            title: "T".to_owned(),
            theme: Theme {
                name: "n".to_owned(),
                colors: Palette {
                    background: "#000000".to_owned(),
                    text: "#ffffff".to_owned(),
                    accent: "#0e6e63".to_owned(),
                    muted: "#888888".to_owned(),
                },
                fonts: FontSet {
                    heading: "Inter".to_owned(),
                    body: "Inter".to_owned(),
                    mono: "Menlo".to_owned(),
                },
            },
            slides: vec![
                Slide {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Open".to_owned()),
                },
                default_slide(),
            ],
            outline: Vec::new(),
            transition: None,
        }
    }

    #[test]
    fn slide_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(slide_label(0, &default_slide()), "1. New slide");
        let deck = deck();
        assert_eq!(slide_label(0, &deck.slides[0]), "1. Swift Design");
        let empty = Slide {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(slide_label(1, &empty), "2. Slide 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_slide_number() {
        let mut planned = deck();
        planned.outline = vec!["Intro".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Intro");
        assert_eq!(outline_title(&planned, 1), "2. Slide 2");
        assert_eq!(outline_title(&planned, 5), "6. Slide 6");
    }

    #[test]
    fn the_default_slide_validates_inside_a_deck() {
        let deck = deck();
        assert_eq!(deck.validate(), Vec::new());
        // 9 deck and theme fields, html and notes on the first slide, html
        // and css on the second.
        assert_eq!(field_count(&deck), 9 + 2 + 2);
    }
}
