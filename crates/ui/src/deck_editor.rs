//! The deck editor: a chat column, a live slide preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The deck twin of `editor.rs`. It shares the preview bridge, the node
//! inspector, the theme and transition forms, and the history section
//! with the design editor; what differs is the deck type, the `/decks`
//! routes, the slide vocabulary, and two deck-only actions: the presenter
//! view and the PPTX export.

use design_model::{ArtifactKind, Deck, Slide, Theme, Transition};
use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::chat::DesignChat;
use crate::editor::{
    APPLY_TO_PREVIEW, HistorySection, NodeCommand, NodeInspector, PREVIEW_LISTENER, PreviewMessage,
    SelectedNode, ThemeForm, TransitionForm, fragment_label, move_screen, node_reference, optional,
};
use crate::icons;

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
    let mut messages = use_signal(Vec::<String>::new);
    let mut preview_version = use_signal(|| 0u32);
    let mut user_paths = use_signal(Vec::<String>::new);
    let mut is_dirty = use_signal(|| false);
    let mut show_properties = use_signal(|| false);
    let mut is_menu_open = use_signal(|| false);
    let mut template_name = use_signal(|| Option::<String>::None);
    let mut chat_context = use_signal(|| Option::<String>::None);
    let mut dragged = use_signal(|| Option::<usize>::None);
    let mut pending_slide_delete = use_signal(|| Option::<usize>::None);

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
                    let node = SelectedNode::from_message(&message);
                    if let Some(node) = &node {
                        chat_context.set(Some(node_reference("slide", message.screen, node)));
                    }
                    selected_node.set(node);
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

    let apply = use_callback(move |command: NodeCommand| {
        let channel = document::eval(APPLY_TO_PREVIEW);
        let _ = channel.send(command);
    });

    let slide_count = deck().slides.len();
    let outline_count = deck().outline.len();
    let total_fields = field_count(&deck());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = deck()
        .slides
        .iter()
        .enumerate()
        .map(|(index, slide)| slide_label(index, slide))
        .collect();
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
                    iframe {
                        title: "Deck preview",
                        "data-preview": "true",
                        src: "/decks/{deck_id}/render?version={preview_version()}&editable=true&slide={selected() + 1}",
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
                                    pending_slide_delete.set(None);
                                },
                                iframe {
                                    title: "Slide {index + 1}",
                                    tabindex: "-1",
                                    src: "/decks/{deck_id}/render?version={preview_version()}&slide={index + 1}",
                                }
                                span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                if slide_count > 1 {
                                    button {
                                        class: if pending_slide_delete() == Some(index) { "thumbnail-delete confirm" } else { "thumbnail-delete" },
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
                                        if pending_slide_delete() == Some(index) {
                                            "Delete?"
                                        } else {
                                            "×"
                                        }
                                    }
                                }
                            }
                        }
                        for index in slide_count..outline_count {
                            div {
                                key: "outline-{index}",
                                class: "thumbnail outline",
                                title: "{outline_title(&deck(), index)} · not written yet",
                                span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                span { class: "outline-label", "outline" }
                            }
                        }
                        button {
                            class: "thumbnail add",
                            title: "Add a slide",
                            onclick: move |_| {
                                deck.with_mut(|deck| deck.slides.push(default_slide()));
                                selected.set(slide_count);
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
                            div { class: "head", "Deck" }
                            label {
                                "Deck title"
                                input {
                                    value: "{deck().title}",
                                    oninput: move |event| {
                                        deck.with_mut(|deck| deck.title = event.value());
                                        is_dirty.set(true);
                                    },
                                }
                            }
                        }
                        ThemeForm {
                            theme: deck().theme.clone(),
                            on_change: move |theme: Theme| {
                                deck.with_mut(|deck| deck.theme = theme);
                                is_dirty.set(true);
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

/// The slide the + tile inserts: a heading and a paragraph.
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

    use super::{default_slide, field_count, outline_title, slide_label};

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
