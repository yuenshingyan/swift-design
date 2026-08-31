//! The mailing editor: a chat column, a live email preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The mailing twin of `print_editor.rs`. It shares the preview
//! bridge, the node inspector, the theme form, and the history section
//! with the design editor; what differs is the mailing type, the
//! `/mailings` routes, the email vocabulary, the format control, and
//! the exports: PDF and a PNG zip beside the HTML file.

use design_model::{ArtifactKind, Email, EmailFormat, Mailing, Theme};
use dioxus::document;
use dioxus::prelude::*;

use crate::api;
use crate::canvas::{frame_width_rem, is_narrow_canvas};
use crate::chat::DesignChat;
use crate::editor::{
    APPLY_TO_PREVIEW, HistorySection, NodeCommand, NodeInspector, PREVIEW_LISTENER, PreviewMessage,
    STRIP_TILE_HEIGHT_REM, SelectedNode, SelectionEntry, ThemeForm, ThumbnailState, fragment_label,
    move_screen, node_reference, optional, outline_entry, page_reference, schedule_save,
    selection_of, selection_paths, selection_reference, strip_summary, thumbnail_class, toggle_pin,
};
use crate::icons;
use crate::settings::artifact_project;

/// How the mailing preview takes a click: as a selection, or as a
/// reader in the inbox would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MailingPreviewMode {
    /// A click reaches the email as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl MailingPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so a mailing opens as its reader sees it.
    pub(crate) const ALL: [MailingPreviewMode; 2] =
        [MailingPreviewMode::Read, MailingPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            MailingPreviewMode::Read => "Read",
            MailingPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            MailingPreviewMode::Read => "See the email as a reader would",
            MailingPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the email shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            MailingPreviewMode::Read => "",
            MailingPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one mailing, then hands it to the editor.
#[component]
pub fn MailingEditor(mailing_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = mailing_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_mailing(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(mailing)) => rsx! {
            LoadedMailingEditor { mailing_id, initial: mailing.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading mailing…" }
        },
    }
}

/// The editor for a loaded mailing: chat on the left, preview and
/// thumbnails on the right, the properties email on demand.
#[component]
fn LoadedMailingEditor(mailing_id: String, initial: Mailing, on_back: EventHandler<()>) -> Element {
    let mut mailing = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| MailingPreviewMode::Read);
    // The emails pinned for the chat with a command-click on a tile.
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
    let mut pending_email_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = mailing_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_mailing_user_paths(&id).await {
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
        let mailing_id = mailing_id.clone();
        move |reload_preview: bool| {
            let id = mailing_id.clone();
            let snapshot = mailing();
            spawn(async move {
                match api::save_mailing(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_mailing_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let mailing_id = mailing_id.clone();
        move |_: ()| {
            let id = mailing_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_mailing(&id).await {
                    mailing.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_mailing_user_paths(&id).await {
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
                    let count = mailing.peek().emails.len();
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
                "swift-design-select" if message.screen < mailing.peek().emails.len() => {
                    selected.set(message.screen);
                    let node = SelectedNode::from_message(&message);
                    let entries = selection_of(&message);
                    if node.is_some() {
                        chat_context.set(Some(selection_reference(
                            "email",
                            message.screen,
                            &entries,
                        )));
                    }
                    selection.set(entries);
                    selected_node.set(node);
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        mailing.with_mut(|mailing| match mailing.emails.get_mut(message.screen) {
                            Some(email) if email.html != html => {
                                email.html = html;
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
                    Some("start") if message.screen < mailing.peek().emails.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < mailing.peek().emails.len()
                        {
                            mailing.with_mut(|mailing| move_screen(&mut mailing.emails, from, to));
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
                            Some(node) => node_reference("email", message.screen, &node),
                            None => format!("[email {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = mailing.with_mut(|mailing| {
                            if mailing.emails.len() > 1 && message.screen < mailing.emails.len() {
                                mailing.emails.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = mailing.peek().emails.len();
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

    let email_count = mailing().emails.len();
    let outline_count = mailing().outline.len();
    // Every email shares the format, so every tile has the same
    // width.
    let viewport = mailing().viewport();
    let tile_width = frame_width_rem(viewport, STRIP_TILE_HEIGHT_REM);
    let email_ratio = viewport.aspect_ratio_css();
    let stage_class = preview_stage_class(viewport);
    let planned_count = outline_count.saturating_sub(email_count);
    let summary = strip_summary(email_count, planned_count);
    let total_fields = field_count(&mailing());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = mailing()
        .emails
        .iter()
        .enumerate()
        .map(|(index, email)| email_label(index, email))
        .collect();
    let email_labels = thumbnail_labels.clone();
    let current_notes = mailing()
        .emails
        .get(selected())
        .and_then(|email| email.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: mailing_id.clone(),
                context: chat_context,
                page: page_reference("email", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: email_labels.clone(),
                page_unit: Some("email".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {email_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in MailingPreviewMode::ALL {
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
                        MailingExportGroup {
                            mailing_id: mailing_id.clone(),
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
                                            let mailing_id = mailing_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let mailing_id = mailing_id.clone();
                                                spawn(async move {
                                                    match api::save_mailing_template(&mailing_id, &name).await {
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
                                    title: "Keep this mailing's theme and layout style for a future design, deck, document, or social",
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
                    // Every mailing format is narrower than the
                    // 16:10 stage, so an email is limited by height
                    // like a phone canvas, without the bezel.
                    div { class: "{stage_class}",
                        iframe {
                            title: "Mailing preview",
                            "data-preview": "true",
                            style: "aspect-ratio: {email_ratio}",
                            src: "/mailings/{mailing_id}/render?version={preview_version()}{mode().render_query()}&email={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == MailingPreviewMode::Read {
                            span { "The email as a reader sees it · switch to Edit to select a node" }
                        } else {
                            span {
                                "Click a node to reference it in the chat and edit its text · ⌘-click adds more · ⌘-click a tile to pin emails"
                            }
                            span { class: "dot", "·" }
                            span { "right-click for quick edits" }
                        }
                        span { class: "dot", "·" }
                        span {
                            kbd { "←" }
                            " "
                            kbd { "→" }
                            " change emails"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Author notes"
                            span { class: "screen-no", "email {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "The subject line and the preheader, as a Subject: line and a Preheader: line, plus intent or handoff remarks. Never shown on the email.",
                            oninput: move |event| {
                                let index = selected();
                                mailing
                                    .with_mut(|mailing| {
                                        if let Some(email) = mailing.emails.get_mut(index) {
                                            email.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Emails"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned email. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the emails the outline still plans",
                                onclick: {
                                    let mailing_id = mailing_id.clone();
                                    move |_| {
                                        let mailing_id = mailing_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&mailing_id);
                                            let sent = api::continue_artifact(&session_id, &mailing_id).await;
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
                                let is_deleting = pending_email_delete() == Some(index);
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
                                            // A command-click pins the email for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_email_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Email {index + 1}",
                                            tabindex: "-1",
                                            src: "/mailings/{mailing_id}/render?version={preview_version()}&email={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if email_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this email",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_email_delete() != Some(index) {
                                                        pending_email_delete.set(Some(index));
                                                        return;
                                                    }
                                                    pending_email_delete.set(None);
                                                    mailing.with_mut(|mailing| remove_email(mailing, index));
                                                    selected.set(selected().min(email_count.saturating_sub(2)));
                                                    selected_node.set(None);
                                                    save.call(true);
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the email anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this email anew",
                                            onclick: {
                                                let mailing_id = mailing_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_email_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let mailing_id = mailing_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&mailing_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &mailing_id,
                                                                "email",
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
                        if outline_count > email_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in email_count..outline_count {
                            {
                                let (number, title) = outline_entry(&mailing().outline, index, "Email");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&mailing(), index)} · not written yet",
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
                        }
                        div { class: "sheet-section",
                            div { class: "head", "Mailing" }
                            label {
                                "Mailing title"
                                input {
                                    value: "{mailing().title}",
                                    oninput: move |event| {
                                        mailing.with_mut(|mailing| mailing.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                            label {
                                "Format"
                                select {
                                    value: "{mailing().format.as_str()}",
                                    onchange: move |event| {
                                        let Some(format) = EmailFormat::from_name(&event.value()) else {
                                            return;
                                        };
                                        mailing.with_mut(|mailing| mailing.format = format);
                                        is_dirty.set(true);
                                        // The canvas changes, so the preview
                                        // reloads on save.
                                        schedule_save(save_generation, save, true);
                                    },
                                    for format in EmailFormat::ALL {
                                        option {
                                            key: "{format.as_str()}",
                                            value: "{format.as_str()}",
                                            selected: mailing().format == format,
                                            "{format_option_label(format)}"
                                        }
                                    }
                                }
                            }
                        }
                        ThemeForm {
                            theme: mailing().theme.clone(),
                            on_change: move |theme: Theme| {
                                mailing.with_mut(|mailing| mailing.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        HistorySection {
                            design_id: mailing_id.clone(),
                            kind: ArtifactKind::Mailing,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// The mailing toolbar's export group: the HTML file, the
/// Chrome-backed PDF of one page per email, and the Chrome-backed zip
/// of one PNG per email.
#[component]
fn MailingExportGroup(mailing_id: String, can_export_with_chrome: bool) -> Element {
    rsx! {
        div { class: "export-group",
            a {
                class: "button",
                href: "/mailings/{mailing_id}/export",
                title: "Export as one HTML file",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "HTML"
            }
            ChromeExportLink {
                href: format!("/mailings/{mailing_id}/export.pdf"),
                label: "PDF",
                title: "Export as a PDF, one page per email",
                is_enabled: can_export_with_chrome,
            }
            ChromeExportLink {
                href: format!("/mailings/{mailing_id}/export.zip"),
                label: "PNG",
                title: "Export as a zip of one PNG per email",
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

/// The stage class for an email of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No format gets the
/// phone bezel.
fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// Removes email `index`, unless it is the last email: a mailing keeps
/// at least one email.
fn remove_email(mailing: &mut Mailing, index: usize) {
    if mailing.emails.len() > 1 && index < mailing.emails.len() {
        mailing.emails.remove(index);
    }
}

/// The format option text: the name and the px canvas.
fn format_option_label(format: EmailFormat) -> String {
    let viewport = format.viewport();
    format!(
        "{} · {} × {}",
        format.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the email, else `Email N`.
fn email_label(index: usize, email: &Email) -> String {
    fragment_label("Email", index, &email.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Email 5` when the outline has no entry there.
fn outline_title(mailing: &Mailing, index: usize) -> String {
    match mailing.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Email {}", index + 1, index + 1),
    }
}

/// A sample email for tests: a heading and a paragraph.
#[cfg(test)]
fn default_email() -> Email {
    Email {
        html: "<div class='body'><h2>New email</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the mailing. Matches the server's
/// provenance paths: absent optional fields are not counted.
fn field_count(mailing: &Mailing) -> usize {
    // Mailing title, format, theme name, four colors, three fonts.
    let mut count = 10;
    for email in &mailing.emails {
        count += 1 + usize::from(email.css.is_some()) + usize::from(email.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Email, EmailFormat, FontSet, Mailing, Palette, Theme};

    use super::{
        MailingPreviewMode, default_email, email_label, field_count, format_option_label,
        outline_title, preview_stage_class, remove_email,
    };

    #[test]
    fn a_mailing_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(MailingPreviewMode::ALL[0], MailingPreviewMode::Read);
        assert_eq!(
            MailingPreviewMode::ALL.map(MailingPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(MailingPreviewMode::Read.render_query(), "");
        assert_eq!(MailingPreviewMode::Edit.render_query(), "&editable=true");
        assert!(MailingPreviewMode::Read.title().contains("reader"));
        assert!(MailingPreviewMode::Edit.title().contains("select"));
    }

    fn mailing() -> Mailing {
        Mailing {
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
            format: EmailFormat::Standard,
            emails: vec![
                Email {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Subject: Swift Design.".to_owned()),
                },
                default_email(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn email_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(email_label(0, &default_email()), "1. New email");
        let mailing = mailing();
        assert_eq!(email_label(0, &mailing.emails[0]), "1. Swift Design");
        let empty = Email {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(email_label(1, &empty), "2. Email 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_email_number() {
        let mut planned = mailing();
        planned.outline = vec!["Front".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Front");
        assert_eq!(outline_title(&planned, 1), "2. Email 2");
        assert_eq!(outline_title(&planned, 5), "6. Email 6");
    }

    #[test]
    fn the_default_email_validates_inside_a_mailing() {
        let mailing = mailing();
        assert_eq!(mailing.validate(), Vec::new());
        // 10 mailing and theme fields, html and notes on the first
        // email, html and css on the second.
        assert_eq!(field_count(&mailing), 10 + 2 + 2);
    }

    #[test]
    fn a_mailing_keeps_its_last_email() {
        let mut mailing = mailing();
        remove_email(&mut mailing, 5);
        assert_eq!(mailing.emails.len(), 2);
        remove_email(&mut mailing, 0);
        assert_eq!(mailing.emails.len(), 1);
        assert_eq!(mailing.emails[0].html, default_email().html);
        remove_email(&mut mailing, 0);
        assert_eq!(mailing.emails.len(), 1);
    }

    #[test]
    fn format_options_name_the_format_and_its_canvas() {
        assert_eq!(format_option_label(EmailFormat::Short), "Short · 600 × 800");
        assert_eq!(
            format_option_label(EmailFormat::Standard),
            "Standard · 600 × 1200"
        );
        assert_eq!(format_option_label(EmailFormat::Long), "Long · 600 × 1800");
    }

    #[test]
    fn every_email_is_limited_by_height() {
        // Every format is 600 px wide and taller than wide, so every
        // mailing stage is narrow.
        for format in EmailFormat::ALL {
            assert_eq!(
                preview_stage_class(format.viewport()),
                "preview-stage narrow",
                "{}",
                format.label()
            );
        }
    }
}
