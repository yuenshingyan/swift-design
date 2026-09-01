//! The campaign editor: a chat column, a live ad preview with a
//! right-click menu, thumbnails, and a properties sheet.
//!
//! The campaign twin of `mailing_editor.rs`. It shares the preview
//! bridge, the node inspector, the theme form, and the history section
//! with the design editor; what differs is the campaign type, the
//! `/campaigns` routes, the ad vocabulary, the size control, and the
//! exports: PDF and a PNG zip beside the HTML file.

use design_model::{Ad, AdSize, ArtifactKind, Campaign, Theme};
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

/// How the campaign preview takes a click: as a selection, or as a
/// reader in the inbox would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CampaignPreviewMode {
    /// A click reaches the ad as it would for a reader.
    Read,
    /// A click selects a node, a double-click edits its text.
    Edit,
}

impl CampaignPreviewMode {
    /// Both modes, in tab order. Read comes first: it is the default,
    /// so a campaign opens as its reader sees it.
    pub(crate) const ALL: [CampaignPreviewMode; 2] =
        [CampaignPreviewMode::Read, CampaignPreviewMode::Edit];

    /// The tab label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            CampaignPreviewMode::Read => "Read",
            CampaignPreviewMode::Edit => "Edit",
        }
    }

    /// The tab tooltip.
    pub(crate) fn title(self) -> &'static str {
        match self {
            CampaignPreviewMode::Read => "See the ad as a reader would",
            CampaignPreviewMode::Edit => "Click a node to select it",
        }
    }

    /// The query that asks the render for the editing script. Read mode
    /// asks for nothing, so the ad shows with no selection outlines.
    pub(crate) fn render_query(self) -> &'static str {
        match self {
            CampaignPreviewMode::Read => "",
            CampaignPreviewMode::Edit => "&editable=true",
        }
    }
}

/// Loads one campaign, then hands it to the editor.
#[component]
pub fn CampaignEditor(campaign_id: String, on_back: EventHandler<()>) -> Element {
    let id_for_fetch = campaign_id.clone();
    let loaded = use_resource(move || {
        let id = id_for_fetch.clone();
        async move { api::fetch_campaign(&id).await }
    });
    let current = loaded.read();
    match &*current {
        Some(Ok(campaign)) => rsx! {
            LoadedCampaignEditor { campaign_id, initial: campaign.clone(), on_back }
        },
        Some(Err(message)) => rsx! {
            p { class: "error", "{message}" }
        },
        None => rsx! {
            p { "Loading campaign…" }
        },
    }
}

/// The editor for a loaded campaign: chat on the left, preview and
/// thumbnails on the right, the properties ad on demand.
#[component]
fn LoadedCampaignEditor(
    campaign_id: String,
    initial: Campaign,
    on_back: EventHandler<()>,
) -> Element {
    let mut campaign = use_signal(|| initial.clone());
    let mut selected = use_signal(|| 0usize);
    let mut selected_node = use_signal(|| Option::<SelectedNode>::None);
    let mut selection = use_signal(Vec::<SelectionEntry>::new);
    let mut mode = use_signal(|| CampaignPreviewMode::Read);
    // The ads pinned for the chat with a command-click on a tile.
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
    let mut pending_ad_delete = use_signal(|| Option::<usize>::None);
    // The tile whose Redo waits for its second click.
    let mut pending_redo = use_signal(|| Option::<usize>::None);

    let authors_id = campaign_id.clone();
    use_future(move || {
        let id = authors_id.clone();
        async move {
            if let Ok(paths) = api::fetch_campaign_user_paths(&id).await {
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
        let campaign_id = campaign_id.clone();
        move |reload_preview: bool| {
            let id = campaign_id.clone();
            let snapshot = campaign();
            spawn(async move {
                match api::save_campaign(&id, &snapshot).await {
                    Ok(()) => {
                        messages.set(Vec::new());
                        is_dirty.set(false);
                        if reload_preview {
                            preview_version += 1;
                        }
                        if let Ok(paths) = api::fetch_campaign_user_paths(&id).await {
                            user_paths.set(paths);
                        }
                    }
                    Err(details) => messages.set(details),
                }
            });
        }
    });

    let reload = use_callback({
        let campaign_id = campaign_id.clone();
        move |_: ()| {
            let id = campaign_id.clone();
            spawn(async move {
                if let Ok(fetched) = api::fetch_campaign(&id).await {
                    campaign.set(fetched);
                    is_dirty.set(false);
                    preview_version += 1;
                    selected_node.set(None);
                }
                if let Ok(paths) = api::fetch_campaign_user_paths(&id).await {
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
                    let count = campaign.peek().ads.len();
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
                "swift-design-select" if message.screen < campaign.peek().ads.len() => {
                    selected.set(message.screen);
                    let node = SelectedNode::from_message(&message);
                    let entries = selection_of(&message);
                    if node.is_some() {
                        chat_context.set(Some(selection_reference("ad", message.screen, &entries)));
                    }
                    selection.set(entries);
                    selected_node.set(node);
                }
                "swift-design-html" => {
                    let Some(html) = message.html else {
                        continue;
                    };
                    let is_changed =
                        campaign.with_mut(|campaign| match campaign.ads.get_mut(message.screen) {
                            Some(ad) if ad.html != html => {
                                ad.html = html;
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
                    Some("start") if message.screen < campaign.peek().ads.len() => {
                        dragged.set(Some(message.screen));
                    }
                    Some("over") => {
                        let to = message.screen;
                        if let Some(from) = dragged()
                            && from != to
                            && to < campaign.peek().ads.len()
                        {
                            campaign.with_mut(|campaign| move_screen(&mut campaign.ads, from, to));
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
                            Some(node) => node_reference("ad", message.screen, &node),
                            None => format!("[ad {}]", message.screen + 1),
                        }));
                    }
                    Some("properties") => {
                        selected.set(message.screen);
                        selected_node.set(SelectedNode::from_message(&message));
                        show_properties.set(true);
                    }
                    Some("delete-screen") => {
                        let removed = campaign.with_mut(|campaign| {
                            if campaign.ads.len() > 1 && message.screen < campaign.ads.len() {
                                campaign.ads.remove(message.screen);
                                true
                            } else {
                                false
                            }
                        });
                        if removed {
                            let count = campaign.peek().ads.len();
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

    let ad_count = campaign().ads.len();
    let outline_count = campaign().outline.len();
    // Every ad shares the size, so every tile has the same width.
    let viewport = campaign().viewport();
    let tile_width = strip_tile_width(viewport);
    let ad_ratio = viewport.aspect_ratio_css();
    let stage_class = preview_stage_class(viewport);
    let planned_count = outline_count.saturating_sub(ad_count);
    let summary = strip_summary(ad_count, planned_count);
    let total_fields = field_count(&campaign());
    let user_count = user_paths().len().min(total_fields);
    let agent_count = total_fields - user_count;
    let thumbnail_labels: Vec<String> = campaign()
        .ads
        .iter()
        .enumerate()
        .map(|(index, ad)| ad_label(index, ad))
        .collect();
    let ad_labels = thumbnail_labels.clone();
    let current_notes = campaign()
        .ads
        .get(selected())
        .and_then(|ad| ad.notes.clone())
        .unwrap_or_default();
    rsx! {
        main { class: "editor",
            DesignChat {
                design_id: campaign_id.clone(),
                context: chat_context,
                page: page_reference("ad", &pinned()),
                is_pinned: !pinned().is_empty(),
                on_pin_page: move |index: usize| {
                    if !pinned().contains(&index) {
                        pinned.write().push(index);
                    }
                },
                pages: ad_labels.clone(),
                page_unit: Some("ad".to_owned()),
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
                    span { class: "preview-heading", "{selected() + 1} / {ad_count}" }
                    div { class: "canvas-tabs preview-modes", role: "tablist",
                        for candidate in CampaignPreviewMode::ALL {
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
                        CampaignExportGroup {
                            campaign_id: campaign_id.clone(),
                            selected: selected(),
                            ad_count,
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
                                            let campaign_id = campaign_id.clone();
                                            move |_| {
                                                let Some(name) = template_name() else {
                                                    return;
                                                };
                                                let name = name.trim().to_owned();
                                                if name.is_empty() {
                                                    return;
                                                }
                                                let campaign_id = campaign_id.clone();
                                                spawn(async move {
                                                    match api::save_campaign_template(&campaign_id, &name).await {
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
                                    title: "Keep this campaign's theme and layout style for a future design, deck, document, or social",
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
                    // A tall ad unit is limited by height like a
                    // phone canvas, without the bezel; a leaderboard
                    // or a mobile banner fills the width instead.
                    div { class: "{stage_class}",
                        iframe {
                            title: "Campaign preview",
                            "data-preview": "true",
                            style: "aspect-ratio: {ad_ratio}",
                            src: "/campaigns/{campaign_id}/render?version={preview_version()}{mode().render_query()}&ad={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == CampaignPreviewMode::Read {
                            span { "The ad as a reader sees it · switch to Edit to select a node" }
                        } else {
                            span {
                                "Click a node to reference it in the chat and edit its text · ⌘-click adds more · ⌘-click a tile to pin ads"
                            }
                            span { class: "dot", "·" }
                            span { "right-click for quick edits" }
                        }
                        span { class: "dot", "·" }
                        span {
                            kbd { "←" }
                            " "
                            kbd { "→" }
                            " change ads"
                        }
                    }
                    label { class: "notes-box",
                        span { class: "notes-heading",
                            "Author notes"
                            span { class: "screen-no", "ad {selected() + 1}" }
                        }
                        textarea {
                            value: "{current_notes}",
                            placeholder: "The click-through URL and the alt text, as a Link: line and an Alt: line, plus intent or handoff remarks. Never shown on the ad.",
                            oninput: move |event| {
                                let index = selected();
                                campaign
                                    .with_mut(|campaign| {
                                        if let Some(ad) = campaign.ads.get_mut(index) {
                                            ad.notes = optional(event.value());
                                        }
                                    });
                                is_dirty.set(true);
                                schedule_save(save_generation, save, false);
                            },
                        }
                    }
                    div { class: "strip-head",
                        "Ads"
                        span { class: "strip-counts", "{summary}" }
                        // One control writes every planned ad. A
                        // button on each planned tile did the same thing
                        // and read as if it wrote that one alone.
                        if planned_count > 0 {
                            button {
                                class: "strip-write",
                                title: "Write the ads the outline still plans",
                                onclick: {
                                    let campaign_id = campaign_id.clone();
                                    move |_| {
                                        let campaign_id = campaign_id.clone();
                                        spawn(async move {
                                            let session_id = artifact_project(&campaign_id);
                                            let sent = api::continue_artifact(&session_id, &campaign_id).await;
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
                                let is_deleting = pending_ad_delete() == Some(index);
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
                                            // A command-click pins the ad for
                                            // the chat; a plain click opens it.
                                            if event.modifiers().meta() || event.modifiers().ctrl() {
                                                toggle_pin(&mut pinned.write(), index);
                                                return;
                                            }
                                            selected.set(index);
                                            selected_node.set(None);
                                            pending_ad_delete.set(None);
                                            pending_redo.set(None);
                                        },
                                        iframe {
                                            title: "Ad {index + 1}",
                                            tabindex: "-1",
                                            src: "/campaigns/{campaign_id}/render?version={preview_version()}&ad={index + 1}",
                                        }
                                        span { class: "thumbnail-number", {format!("{:02}", index + 1)} }
                                        if ad_count > 1 {
                                            button {
                                                class: if is_deleting { "thumbnail-delete confirm" } else { "thumbnail-delete" },
                                                title: "Delete this ad",
                                                onclick: move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    if pending_ad_delete() != Some(index) {
                                                        pending_ad_delete.set(Some(index));
                                                        return;
                                                    }
                                                    pending_ad_delete.set(None);
                                                    campaign.with_mut(|campaign| remove_ad(campaign, index));
                                                    selected.set(selected().min(ad_count.saturating_sub(2)));
                                                    selected_node.set(None);
                                                    save.call(true);
                                                },
                                                "×"
                                                if is_deleting {
                                                    span { class: "delete-text", "delete?" }
                                                }
                                            }
                                        }
                                        // A redo writes the ad anew: the model
                                        // sees its notes, not its markup.
                                        button {
                                            class: if is_redoing { "thumbnail-redo confirm" } else { "thumbnail-redo" },
                                            title: "Write this ad anew",
                                            onclick: {
                                                let campaign_id = campaign_id.clone();
                                                move |event: Event<MouseData>| {
                                                    event.stop_propagation();
                                                    pending_ad_delete.set(None);
                                                    if pending_redo() != Some(index) {
                                                        pending_redo.set(Some(index));
                                                        return;
                                                    }
                                                    pending_redo.set(None);
                                                    if is_dirty() {
                                                        save.call(true);
                                                    }
                                                    let campaign_id = campaign_id.clone();
                                                    spawn(async move {
                                                        let session_id = artifact_project(&campaign_id);
                                                        let sent = api::regenerate_unit(
                                                                &session_id,
                                                                &campaign_id,
                                                                "ad",
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
                        if outline_count > ad_count {
                            span { class: "strip-divider" }
                        }
                        // A planned tile is an outline entry nobody has
                        // written. The strip head writes them.
                        for index in ad_count..outline_count {
                            {
                                let (number, title) = outline_entry(&campaign().outline, index, "Ad");
                                rsx! {
                                    div {
                                        key: "outline-{index}",
                                        class: "thumbnail outline",
                                        style: "--tile-width: {tile_width}rem",
                                        title: "{outline_title(&campaign(), index)} · not written yet",
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
                            div { class: "head", "Campaign" }
                            label {
                                "Campaign title"
                                input {
                                    value: "{campaign().title}",
                                    oninput: move |event| {
                                        campaign.with_mut(|campaign| campaign.title = event.value());
                                        is_dirty.set(true);
                                        schedule_save(save_generation, save, false);
                                    },
                                }
                            }
                            label {
                                "Size"
                                select {
                                    value: "{campaign().size.as_str()}",
                                    onchange: move |event| {
                                        let Some(size) = AdSize::from_name(&event.value()) else {
                                            return;
                                        };
                                        campaign.with_mut(|campaign| campaign.size = size);
                                        is_dirty.set(true);
                                        // The canvas changes, so the preview
                                        // reloads on save.
                                        schedule_save(save_generation, save, true);
                                    },
                                    for size in AdSize::ALL {
                                        option {
                                            key: "{size.as_str()}",
                                            value: "{size.as_str()}",
                                            selected: campaign().size == size,
                                            "{size_option_label(size)}"
                                        }
                                    }
                                }
                            }
                        }
                        ThemeForm {
                            theme: campaign().theme.clone(),
                            on_change: move |theme: Theme| {
                                campaign.with_mut(|campaign| campaign.theme = theme);
                                is_dirty.set(true);
                                schedule_save(save_generation, save, true);
                            },
                        }
                        HistorySection {
                            design_id: campaign_id.clone(),
                            kind: ArtifactKind::Campaign,
                            on_restored: move |_| reload.call(()),
                        }
                    }
                }
            }
        }
    }
}

/// The href of the HTML or PDF export: the whole campaign, or one
/// zero-based ad through the export route's `?ad=N` query.
fn export_href(campaign_id: &str, extension: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!(
            "/campaigns/{campaign_id}/export{extension}?ad={}",
            index + 1
        ),
        None => format!("/campaigns/{campaign_id}/export{extension}"),
    }
}

/// The href of the PNG export: one ad's image when scoped, the zip
/// of every ad otherwise.
fn png_export_href(campaign_id: &str, only: Option<usize>) -> String {
    match only {
        Some(index) => format!("/campaigns/{campaign_id}/ads/{}.png", index + 1),
        None => format!("/campaigns/{campaign_id}/export.zip"),
    }
}

/// The download name of a scoped export file, matching the zip entry.
fn ad_download_name(campaign_id: &str, index: usize, extension: &str) -> String {
    format!("{campaign_id}-ad-{}.{extension}", index + 1)
}

/// The campaign toolbar's export group: a scope toggle when the
/// campaign has more than one ad, then the HTML file, the
/// Chrome-backed PDF, and the PNG export. The scope picks the ad on
/// screen or every ad; scoped, the PNG link downloads that ad's image.
#[component]
fn CampaignExportGroup(
    campaign_id: String,
    selected: usize,
    ad_count: usize,
    can_export_with_chrome: bool,
) -> Element {
    let mut is_scoped = use_signal(|| false);
    let only = (is_scoped() && ad_count > 1).then_some(selected);
    let number = selected + 1;
    let html_title = if only.is_some() {
        "Export the ad on screen as one HTML file"
    } else {
        "Export as one HTML file"
    };
    let pdf_title = if only.is_some() {
        "Export the ad on screen as a one-page PDF"
    } else {
        "Export as a PDF, one page per ad"
    };
    let png_title = if only.is_some() {
        "Download the ad on screen as a PNG"
    } else {
        "Export as a zip of one PNG per ad"
    };
    rsx! {
        div { class: "export-group",
            if ad_count > 1 {
                div { class: "canvas-tabs export-scope", role: "tablist",
                    button {
                        role: "tab",
                        class: if only.is_none() { "canvas-tab open" } else { "canvas-tab" },
                        title: "Export every ad",
                        onclick: move |_| is_scoped.set(false),
                        "All ads"
                    }
                    button {
                        role: "tab",
                        class: if only.is_some() { "canvas-tab open" } else { "canvas-tab" },
                        title: "Export only the ad on screen",
                        onclick: move |_| is_scoped.set(true),
                        "Ad {number}"
                    }
                }
            }
            a {
                class: "button",
                href: export_href(&campaign_id, "", only),
                title: "{html_title}",
                span { dangerous_inner_html: icons::DOWNLOAD }
                "HTML"
            }
            ChromeExportLink {
                href: export_href(&campaign_id, ".pdf", only),
                label: "PDF",
                title: pdf_title,
                is_enabled: can_export_with_chrome,
            }
            ChromeExportLink {
                href: png_export_href(&campaign_id, only),
                label: "PNG",
                title: png_title,
                is_enabled: can_export_with_chrome,
                download: only.map(|index| ad_download_name(&campaign_id, index, "png")),
            }
        }
    }
}

/// One export link that needs Chrome on the server: a link when Chrome
/// is there, a disabled cell with the install hint otherwise. With
/// `download`, the link saves under that name instead of navigating.
#[component]
fn ChromeExportLink(
    href: String,
    label: &'static str,
    title: &'static str,
    is_enabled: bool,
    #[props(default)] download: Option<String>,
) -> Element {
    if is_enabled {
        return rsx! {
            a {
                class: "button",
                href: "{href}",
                title: "{title}",
                download,
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

/// The stage class for an ad of `viewport`: a narrow canvas is
/// limited by height, a wide one fills the width. No size gets the
/// phone bezel.
fn preview_stage_class(viewport: design_model::Viewport) -> &'static str {
    if is_narrow_canvas(viewport) {
        return "preview-stage narrow";
    }
    "preview-stage"
}

/// The strip tile width for an ad of `viewport`, in rem. The tile is
/// `STRIP_TILE_HEIGHT_REM` tall at the canvas ratio, but a wide unit
/// such as the 728 by 90 leaderboard would grow past the strip, so the
/// width is capped and the tile shrinks below the shared height.
fn strip_tile_width(viewport: design_model::Viewport) -> String {
    let ratio = f64::from(viewport.width) / f64::from(viewport.height);
    let height = STRIP_TILE_HEIGHT_REM.min(TILE_WIDTH_CAP_REM / ratio);
    frame_width_rem(viewport, height)
}

/// Widest a strip tile may be, in rem.
const TILE_WIDTH_CAP_REM: f64 = 18.0;

/// Removes ad `index`, unless it is the last ad: a campaign keeps
/// at least one ad.
fn remove_ad(campaign: &mut Campaign, index: usize) {
    if campaign.ads.len() > 1 && index < campaign.ads.len() {
        campaign.ads.remove(index);
    }
}

/// The size option text: the name and the px canvas.
fn size_option_label(size: AdSize) -> String {
    let viewport = size.viewport();
    format!(
        "{} · {} × {}",
        size.label(),
        viewport.width,
        viewport.height
    )
}

/// Short label for a thumbnail: position, then the first heading text,
/// else the first words of the ad, else `Ad N`.
fn ad_label(index: usize, ad: &Ad) -> String {
    fragment_label("Ad", index, &ad.html)
}

/// The planned title of outline entry `index`, as the thumbnail tooltip:
/// `5. Title`, or `5. Ad 5` when the outline has no entry there.
fn outline_title(campaign: &Campaign, index: usize) -> String {
    match campaign.outline.get(index) {
        Some(title) if !title.trim().is_empty() => format!("{}. {}", index + 1, title.trim()),
        _ => format!("{}. Ad {}", index + 1, index + 1),
    }
}

/// A sample ad for tests: a heading and a paragraph.
#[cfg(test)]
fn default_ad() -> Ad {
    Ad {
        html: "<div class='body'><h2>New ad</h2><p>Text</p></div>".to_owned(),
        css: Some(
            ".body { padding: 72px; height: 100%; display: flex; flex-direction: column; gap: 16px; } h2 { font-size: 48px; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// Number of set leaf fields in the campaign. Matches the server's
/// provenance paths: absent optional fields are not counted.
fn field_count(campaign: &Campaign) -> usize {
    // Campaign title, size, theme name, four colors, three fonts.
    let mut count = 10;
    for ad in &campaign.ads {
        count += 1 + usize::from(ad.css.is_some()) + usize::from(ad.notes.is_some());
    }
    count
}

#[cfg(test)]
mod tests {
    use design_model::{Ad, AdSize, Campaign, FontSet, Palette, Theme};

    use super::{
        CampaignPreviewMode, ad_label, default_ad, field_count, outline_title, preview_stage_class,
        remove_ad, size_option_label, strip_tile_width,
    };

    #[test]
    fn a_scoped_export_names_the_ad_in_href_and_filename() {
        assert_eq!(
            super::export_href("launch", "", None),
            "/campaigns/launch/export"
        );
        assert_eq!(
            super::export_href("launch", ".pdf", Some(1)),
            "/campaigns/launch/export.pdf?ad=2"
        );
        assert_eq!(
            super::png_export_href("launch", None),
            "/campaigns/launch/export.zip"
        );
        assert_eq!(
            super::png_export_href("launch", Some(1)),
            "/campaigns/launch/ads/2.png"
        );
        assert_eq!(
            super::ad_download_name("launch", 1, "png"),
            "launch-ad-2.png"
        );
    }

    #[test]
    fn a_campaign_opens_in_read_mode_and_edit_loads_the_editing_script() {
        assert_eq!(CampaignPreviewMode::ALL[0], CampaignPreviewMode::Read);
        assert_eq!(
            CampaignPreviewMode::ALL.map(CampaignPreviewMode::label),
            ["Read", "Edit"]
        );
        assert_eq!(CampaignPreviewMode::Read.render_query(), "");
        assert_eq!(CampaignPreviewMode::Edit.render_query(), "&editable=true");
        assert!(CampaignPreviewMode::Read.title().contains("reader"));
        assert!(CampaignPreviewMode::Edit.title().contains("select"));
    }

    fn campaign() -> Campaign {
        Campaign {
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
            size: AdSize::MediumRectangle,
            ads: vec![
                Ad {
                    html: "<h1>Swift Design</h1>".to_owned(),
                    css: None,
                    notes: Some("Link: https://swift.design.".to_owned()),
                },
                default_ad(),
            ],
            outline: Vec::new(),
        }
    }

    #[test]
    fn ad_labels_use_the_heading_then_the_text_then_a_number() {
        assert_eq!(ad_label(0, &default_ad()), "1. New ad");
        let campaign = campaign();
        assert_eq!(ad_label(0, &campaign.ads[0]), "1. Swift Design");
        let empty = Ad {
            html: "<div></div>".to_owned(),
            css: None,
            notes: None,
        };
        assert_eq!(ad_label(1, &empty), "2. Ad 2");
    }

    #[test]
    fn outline_titles_fall_back_to_the_ad_number() {
        let mut planned = campaign();
        planned.outline = vec!["Front".to_owned(), "  ".to_owned()];
        assert_eq!(outline_title(&planned, 0), "1. Front");
        assert_eq!(outline_title(&planned, 1), "2. Ad 2");
        assert_eq!(outline_title(&planned, 5), "6. Ad 6");
    }

    #[test]
    fn the_default_ad_validates_inside_a_campaign() {
        let campaign = campaign();
        assert_eq!(campaign.validate(), Vec::new());
        // 10 campaign and theme fields, html and notes on the first
        // ad, html and css on the second.
        assert_eq!(field_count(&campaign), 10 + 2 + 2);
    }

    #[test]
    fn a_campaign_keeps_its_last_ad() {
        let mut campaign = campaign();
        remove_ad(&mut campaign, 5);
        assert_eq!(campaign.ads.len(), 2);
        remove_ad(&mut campaign, 0);
        assert_eq!(campaign.ads.len(), 1);
        assert_eq!(campaign.ads[0].html, default_ad().html);
        remove_ad(&mut campaign, 0);
        assert_eq!(campaign.ads.len(), 1);
    }

    #[test]
    fn size_options_name_the_size_and_its_canvas() {
        assert_eq!(
            size_option_label(AdSize::MediumRectangle),
            "Medium rectangle · 300 × 250"
        );
        assert_eq!(
            size_option_label(AdSize::Leaderboard),
            "Leaderboard · 728 × 90"
        );
        assert_eq!(
            size_option_label(AdSize::MobileBanner),
            "Mobile banner · 320 × 100"
        );
    }

    #[test]
    fn tall_units_are_limited_by_height_and_wide_units_by_width() {
        assert_eq!(
            preview_stage_class(AdSize::MediumRectangle.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(AdSize::HalfPage.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(AdSize::Skyscraper.viewport()),
            "preview-stage narrow"
        );
        assert_eq!(
            preview_stage_class(AdSize::Leaderboard.viewport()),
            "preview-stage"
        );
        assert_eq!(
            preview_stage_class(AdSize::MobileBanner.viewport()),
            "preview-stage"
        );
    }

    #[test]
    fn wide_units_get_a_capped_strip_tile() {
        // A medium rectangle keeps the shared tile height.
        assert_eq!(strip_tile_width(AdSize::MediumRectangle.viewport()), "6.60");
        // A leaderboard at the shared height would be 44.5 rem wide;
        // the cap holds it to 18 rem.
        assert_eq!(strip_tile_width(AdSize::Leaderboard.viewport()), "18.00");
        assert_eq!(strip_tile_width(AdSize::MobileBanner.viewport()), "17.60");
    }
}
