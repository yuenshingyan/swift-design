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
                    selection.set(selection_of(&message));
                    selected_node.set(SelectedNode::from_message(&message));
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
                            campaign.with_mut(|campaign| {
                                move_screen(&mut campaign.ads, &mut campaign.outline, from, to)
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
                            "ad",
                            message.screen,
                            &selection_of(&message),
                        )));
                    }
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
                        PreviewFrame {
                            title: "Campaign preview",
                            ratio: "{ad_ratio}",
                            src: "/campaigns/{campaign_id}/render?version={preview_version()}{mode().render_query()}&ad={selected() + 1}",
                        }
                    }
                    p { class: "preview-hint",
                        if mode() == CampaignPreviewMode::Read {
                            span { "The ad as a reader sees it · switch to Edit to select a node" }
                        } else {
                            span {
                                "Click a node to select it and edit its text · ⌘-click adds more · right-click and Pin to chat references it · ⌘-click a tile to pin ads"
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
                            scope: artifact_project(&campaign_id),
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
