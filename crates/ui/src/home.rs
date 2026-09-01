//! The landing page: a composer on top, the session list below.
//!
//! The user describes a design and submits. Swift Design creates a
//! session, starts the briefing run, and opens the session workspace.
//! Existing sessions are listed underneath.

use design_model::ArtifactKind;
use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::{ModelChip, SendButton};
use crate::icons;
use crate::run_settings::state_label;
use crate::settings::{SettingsPanel, pause_briefly};
use crate::uploads::{AttachButton, AttachmentChips, PasteUploads};

/// The label on the template button: how many templates are chosen.
fn template_button_label(saved: usize, chosen: usize) -> String {
    match (saved, chosen) {
        (0, _) => "No templates yet: save one from the editor, or make one above".to_owned(),
        (_, 0) => "None chosen: the candidates get their own look".to_owned(),
        (_, 1) => "1 chosen".to_owned(),
        (_, count) => format!("{count} chosen"),
    }
}

/// The composer placeholder. It names every kind, because the user
/// picks the kind after they send the request, not before.
const COMPOSER_PLACEHOLDER: &str = "A landing page for my finance app, a ten-slide pitch deck, a two-page memo, or a launch carousel…";

/// What one kind is for, under its name in the picker.
fn kind_detail(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Demo => "A landing page, app screens, or a flow, on a device canvas.",
        ArtifactKind::Deck => "Slides on a 1920 by 1080 px canvas, with a presenter view.",
        ArtifactKind::Document => {
            "A report, a memo, a proposal, or a guide on A4 or Letter pages, with PDF and DOCX exports."
        }
        ArtifactKind::Social => {
            "A post or a carousel on a square, portrait, story, or landscape canvas, with PDF and PNG exports."
        }
        ArtifactKind::Print => {
            "A poster, a flyer, a menu, or a sign on an A5 to A3, Letter, or Tabloid canvas, with PDF and PNG exports."
        }
        ArtifactKind::Mailing => {
            "An email or an email sequence on a 600 px wide canvas, with PDF and PNG exports."
        }
        ArtifactKind::Campaign => {
            "A display ad or a set of A/B variants on a standard IAB unit, with PDF and PNG exports."
        }
        ArtifactKind::Artwork => {
            "A video thumbnail, channel banner, album or book cover, with PDF and PNG exports."
        }
    }
}

/// The badge class suffix for a session state.
fn state_class(state: design_model::WorkflowState) -> &'static str {
    use design_model::WorkflowState::*;
    match state {
        Generating => "generating",
        Reviewing => "reviewing",
        Stopped => "stopped",
        Error => "error",
        _ => "",
    }
}

/// A stored RFC 3339 stamp as a row shows it: the date, the time to the
/// minute, and the zone. A stamp of another shape is shown unchanged.
fn short_time(stamp: &str) -> String {
    let Some((date, time)) = stamp.split_once('T') else {
        return stamp.to_owned();
    };
    let minutes: String = time.chars().take(5).collect();
    if date.len() != 10 || minutes.len() != 5 {
        return stamp.to_owned();
    }
    format!("{date} {minutes} UTC")
}

/// The landing page.
#[component]
pub fn Home(on_open_session: EventHandler<String>) -> Element {
    let mut sessions = use_signal(Vec::<api::SessionSummary>::new);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let mut is_configuring = use_signal(|| false);
    let mut is_loaded = use_signal(|| false);
    let mut request = use_signal(String::new);
    let mut is_picking_kind = use_signal(|| false);
    let mut effort = use_signal(|| "medium".to_owned());
    let mut templates = use_signal(Vec::<api::TemplateSummary>::new);
    let chosen_templates = use_signal(Vec::<String>::new);
    let mut chosen_for_defaults = chosen_templates;
    let mut has_applied_defaults = use_signal(|| false);
    let mut is_picking_templates = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut uploads = use_signal(Vec::<api::UploadSummary>::new);
    let refresh_uploads = use_callback(move |_: ()| {
        spawn(async move {
            if let Ok(listing) = api::fetch_uploads(api::DRAFT_SCOPE).await {
                uploads.set(listing);
            }
        });
    });
    let refresh_sessions = use_callback(move |_: ()| {
        spawn(async move {
            if let Ok(listing) = api::fetch_sessions().await {
                sessions.set(listing);
            }
        });
    });
    let mut has_auto_opened_setup = use_signal(|| false);
    use_effect(move || {
        if let Some(view) = settings()
            && view.current.is_none()
            && !has_auto_opened_setup()
        {
            has_auto_opened_setup.set(true);
            is_configuring.set(true);
        }
    });

    use_future(move || async move {
        let mut seen = 0u64;
        loop {
            if let Ok(fetched) = api::fetch_sessions().await {
                sessions.set(fetched);
            }
            if let Ok(fetched) = api::fetch_settings().await {
                settings.set(Some(fetched));
            }
            if let Ok(fetched) = api::fetch_templates().await {
                // A default template is picked for the first session of
                // this visit. A later pick by the user is kept as it is.
                if !has_applied_defaults() {
                    let defaults: Vec<String> = fetched
                        .iter()
                        .filter(|template| template.is_default)
                        .map(|template| template.id.clone())
                        .collect();
                    if !defaults.is_empty() && chosen_templates().is_empty() {
                        chosen_for_defaults.set(defaults);
                    }
                    has_applied_defaults.set(true);
                }
                templates.set(fetched);
            }
            refresh_uploads.call(());
            is_loaded.set(true);
            match api::wait_for_change(seen).await {
                Ok(current) => seen = current,
                Err(_) => pause_briefly().await,
            }
        }
    });

    // Sending asks for the kind. The request text is checked first, so
    // the picker never opens over an empty composer.
    let create = use_callback(move |_: ()| {
        if request().trim().is_empty() {
            error.set(Some("Describe the design first.".to_owned()));
            return;
        }
        error.set(None);
        is_picking_kind.set(true);
    });

    let create_with_kind = use_callback(move |chosen_kind: ArtifactKind| {
        let text = request().trim().to_owned();
        if text.is_empty() {
            return;
        }
        let level = effort();
        let picked = chosen_templates();
        is_picking_kind.set(false);
        spawn(async move {
            let body = api::CreateSessionRequest {
                request: &text,
                artifact_kind: chosen_kind.as_str(),
                options: api::CreateOptions {
                    effort: &level,
                    preview: true,
                    templates: &picked,
                },
            };
            match api::create_session(&body).await {
                Ok(id) => {
                    error.set(None);
                    on_open_session.call(id);
                }
                Err(message) => error.set(Some(message)),
            }
        });
    });

    let is_model_chosen = settings().and_then(|view| view.current).is_some();
    let show_setup = is_loaded() && is_configuring();
    let session_list = sessions();

    rsx! {
        main { class: "home",
            section { class: "home-hero",
                h1 { "What are we designing?" }
                p { class: "lede",
                    "Describe it once. We'll ask a few questions in the chat, then write candidates."
                }
            }
            section { class: "home-composer",
                div { class: "brief-card",
                    textarea {
                        placeholder: COMPOSER_PLACEHOLDER,
                        value: "{request()}",
                        oninput: move |event| request.set(event.value()),
                        onkeydown: move |event: Event<KeyboardData>| {
                            if event.key() == Key::Enter && !event.modifiers().shift() {
                                event.prevent_default();
                                if is_model_chosen {
                                    create.call(());
                                }
                            }
                        },
                    }
                    PasteUploads {
                        scope: api::DRAFT_SCOPE.to_owned(),
                        on_uploaded: move |_| {
                            error.set(None);
                            refresh_uploads.call(());
                        },
                        on_error: move |message| error.set(Some(message)),
                    }
                    AttachmentChips {
                        uploads: uploads(),
                        on_changed: move |_| refresh_uploads.call(()),
                        on_error: move |message| error.set(Some(message)),
                    }
                    div { class: "brief-footer",
                        div { class: "home-controls",
                            AttachButton {
                                scope: api::DRAFT_SCOPE.to_owned(),
                                on_uploaded: move |_| {
                                    error.set(None);
                                    refresh_uploads.call(());
                                },
                                on_error: move |message| error.set(Some(message)),
                            }
                            // Always shown: the picker also makes a
                            // template from a website or brand files.
                            button {
                                class: if chosen_templates().is_empty() { "template-button" } else { "template-button chosen" },
                                title: "Templates: pick a saved look, or make one from a website",
                                onclick: move |_| is_picking_templates.set(true),
                                span { dangerous_inner_html: icons::LAYOUT }
                                if !chosen_templates().is_empty() {
                                    span { class: "template-button-count", "{chosen_templates().len()}" }
                                }
                            }
                            span { class: "divider" }
                            ModelChip {
                                settings,
                                is_configuring,
                                effort: Some(effort()),
                                on_effort: move |level: String| effort.set(level),
                            }
                        }
                        SendButton {
                            label: "Create",
                            is_enabled: is_model_chosen,
                            on_send: create,
                        }
                    }
                }
                if show_setup {
                    div { class: "home-setup",
                        SettingsPanel { settings, is_configuring }
                    }
                }
                if let Some(message) = error() {
                    p { class: "error", "{message}" }
                }
            }
            if is_picking_kind() {
                KindPicker {
                    is_open: is_picking_kind,
                    on_pick: move |chosen| create_with_kind.call(chosen),
                }
            }
            if is_picking_templates() {
                TemplatePicker {
                    templates,
                    chosen: chosen_templates,
                    is_open: is_picking_templates,
                    uploads: uploads().iter().map(|upload| upload.name.clone()).collect::<Vec<String>>(),
                    on_error: move |message| error.set(Some(message)),
                }
            }
            section { class: "home-projects",
                div { class: "projects-head",
                    h2 { "Sessions" }
                    span { class: "projects-count", "{session_list.len()}" }
                }
                if session_list.is_empty() && is_loaded() {
                    p { class: "home-empty", "No sessions yet. Describe a design above to start one." }
                }
                for summary in session_list {
                    div {
                        class: "project-row session-row",
                        key: "{summary.id}",
                        tabindex: "0",
                        onclick: {
                            let id = summary.id.clone();
                            move |_| on_open_session.call(id.clone())
                        },
                        span { class: "project-title", title: "{summary.title}", "{summary.title}" }
                        span { class: "project-kind", "{summary.artifact_kind.label()}" }
                        span {
                            class: "project-time",
                            title: "Created {summary.created_at}\nUpdated {summary.updated_at}",
                            "{short_time(&summary.updated_at)}"
                        }
                        span { class: "state-badge {state_class(summary.state)}",
                            "{state_label(summary.state)}"
                        }
                        button {
                            class: "row-delete",
                            title: "Delete this session",
                            onclick: {
                                let id = summary.id.clone();
                                move |event: Event<MouseData>| {
                                    event.stop_propagation();
                                    let id = id.clone();
                                    spawn(async move {
                                        // A refused delete used to be
                                        // dropped here, so the button
                                        // read as broken.
                                        match api::delete_session(&id).await {
                                            Ok(()) => {
                                                error.set(None);
                                                refresh_sessions.call(());
                                            }
                                            Err(message) => error.set(Some(message)),
                                        }
                                    });
                                }
                            },
                            "×"
                        }
                    }
                }
            }
        }
    }
}

/// The kind picker: the modal that opens when the user sends a request.
///
/// The kind is fixed for the life of a session, so it is asked once,
/// here, and never guessed. Escape and the backdrop close the modal and
/// leave the request in the composer, so a mis-press costs nothing.
#[component]
fn KindPicker(is_open: Signal<bool>, on_pick: EventHandler<ArtifactKind>) -> Element {
    let mut is_open = is_open;
    let close = move |_| is_open.set(false);
    rsx! {
        div { class: "modal-backdrop blurring", onclick: close }
        div {
            class: "modal kind-modal",
            role: "dialog",
            aria_modal: true,
            aria_label: "Choose what to build",
            autofocus: true,
            tabindex: "-1",
            onkeydown: move |event: Event<KeyboardData>| {
                if event.key() == Key::Escape {
                    is_open.set(false);
                }
            },
            div { class: "modal-head",
                span { class: "kicker", "What are we building?" }
                button { class: "icon-button", title: "Close", onclick: close, "×" }
            }
            div { class: "modal-body",
                div { class: "kind-choices",
                    for choice in ArtifactKind::ALL {
                        button {
                            key: "{choice.as_str()}",
                            class: "kind-choice",
                            onclick: move |_| on_pick.call(choice),
                            span { class: "kind-choice-name", "{choice.label()}" }
                            span { class: "kind-choice-detail", "{kind_detail(choice)}" }
                        }
                    }
                }
                p { class: "kind-note",
                    "This is fixed for the session. Start a new one to build the other kind."
                }
            }
        }
    }
}

/// The template picker modal.
#[component]
fn TemplatePicker(
    templates: Signal<Vec<api::TemplateSummary>>,
    chosen: Signal<Vec<String>>,
    is_open: Signal<bool>,
    /// The names of the files attached to the composer, as brand
    /// material for a new template.
    #[props(default)]
    uploads: Vec<String>,
    on_error: EventHandler<String>,
) -> Element {
    let mut chosen = chosen;
    let mut is_open = is_open;
    let close = move |_| is_open.set(false);
    let saved = templates();
    let count = chosen().len();
    // The extraction form: a name, a website, or the attached files.
    let mut extract_name = use_signal(String::new);
    let mut extract_url = use_signal(String::new);
    let mut is_extracting = use_signal(|| false);
    let upload_count = uploads.len();
    let has_url = !extract_url().trim().is_empty();
    let can_extract =
        !extract_name().trim().is_empty() && (has_url || upload_count > 0) && !is_extracting();
    let extract = use_callback(move |_: ()| {
        let name = extract_name().trim().to_owned();
        let url = extract_url().trim().to_owned();
        let files = uploads.clone();
        is_extracting.set(true);
        spawn(async move {
            let source_url = (!url.is_empty()).then_some(url.as_str());
            let files = if source_url.is_some() {
                Vec::new()
            } else {
                files
            };
            let made = api::extract_template(&name, source_url, &files, api::DRAFT_SCOPE).await;
            is_extracting.set(false);
            match made {
                Ok(summary) => {
                    extract_name.set(String::new());
                    extract_url.set(String::new());
                    let mut ids = chosen();
                    if !ids.contains(&summary.id) {
                        ids.push(summary.id);
                    }
                    chosen.set(ids);
                }
                Err(message) => on_error.call(message),
            }
        });
    });
    rsx! {
        div { class: "modal-backdrop", onclick: close }
        div {
            class: "modal",
            role: "dialog",
            aria_modal: true,
            aria_label: "Choose templates",
            div { class: "modal-head",
                span { class: "kicker", "Templates" }
                button { class: "icon-button", title: "Close", onclick: close, "×" }
            }
            div { class: "modal-body",
                div { class: "extract-form",
                    span { class: "extract-title", "New template from a brand" }
                    input {
                        class: "extract-name",
                        placeholder: "Template name",
                        value: "{extract_name()}",
                        oninput: move |event| extract_name.set(event.value()),
                    }
                    input {
                        class: "extract-url",
                        placeholder: "https://example.com",
                        value: "{extract_url()}",
                        oninput: move |event| extract_url.set(event.value()),
                    }
                    button {
                        class: "extract-button",
                        disabled: !can_extract,
                        title: "The model reads the website, or the files attached to the composer, and writes a theme",
                        onclick: move |_| extract.call(()),
                        if is_extracting() {
                            "Reading…"
                        } else if has_url {
                            "Extract from the website"
                        } else {
                            "{extract_label(upload_count)}"
                        }
                    }
                }
                div { class: "template-grid",
                    for saved_template in saved.iter().cloned() {
                        {
                            let id = saved_template.id.clone();
                            let is_chosen = chosen().contains(&id);
                            rsx! {
                                div {
                                    key: "{saved_template.id}",
                                    class: if is_chosen { "template-card chosen" } else { "template-card" },
                                    button {
                                        class: "template-card-hit",
                                        onclick: {
                                            let id = id.clone();
                                            move |_| {
                                                let mut ids = chosen();
                                                match ids.iter().position(|chosen_id| chosen_id == &id) {
                                                    Some(index) => {
                                                        ids.remove(index);
                                                    }
                                                    None => ids.push(id.clone()),
                                                }
                                                chosen.set(ids);
                                            }
                                        },
                                        div { class: "template-thumb",
                                            iframe {
                                                title: "{saved_template.name}",
                                                src: "/templates/{saved_template.id}/render?screen=1",
                                            }
                                        }
                                        div { class: "template-meta",
                                            span { class: "template-card-name", "{saved_template.name}" }
                                            span { class: "template-card-detail",
                                                "{saved_template.theme} · {saved_template.screen_count} screens"
                                            }
                                        }
                                    }
                                    button {
                                        class: if saved_template.is_default { "template-card-default on" } else { "template-card-default" },
                                        title: if saved_template.is_default { "New sessions start with this template. Click to stop that." } else { "Start new sessions with this template" },
                                        onclick: {
                                            let id = id.clone();
                                            let is_default = saved_template.is_default;
                                            move |_| {
                                                let id = id.clone();
                                                spawn(async move {
                                                    let set = api::set_default_template(&id, !is_default).await;
                                                    if let Err(message) = set {
                                                        on_error.call(message);
                                                    }
                                                });
                                            }
                                        },
                                        if saved_template.is_default {
                                            "★ default"
                                        } else {
                                            "☆"
                                        }
                                    }
                                    button {
                                        class: "template-card-delete",
                                        title: "Delete this template",
                                        onclick: {
                                            let id = id.clone();
                                            move |_| {
                                                let id = id.clone();
                                                spawn(async move {
                                                    if let Err(message) = api::delete_template(&id).await {
                                                        on_error.call(message);
                                                    }
                                                });
                                            }
                                        },
                                        "×"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "modal-foot",
                span { class: "step-count", "{template_button_label(saved.len(), count)}" }
                button { class: "primary", onclick: close, "Done" }
                if count > 0 {
                    button { onclick: move |_| chosen.set(Vec::new()), "Clear" }
                }
            }
        }
    }
}

/// The extract button's label when no website is typed: the attached
/// files are the material.
fn extract_label(upload_count: usize) -> String {
    match upload_count {
        0 => "Type a website, or attach brand files".to_owned(),
        1 => "Extract from the attached file".to_owned(),
        count => format!("Extract from the {count} attached files"),
    }
}

#[cfg(test)]
mod tests {
    use design_model::ArtifactKind;

    use crate::home::{
        COMPOSER_PLACEHOLDER, extract_label, kind_detail, short_time, template_button_label,
    };

    #[test]
    fn the_extract_button_names_its_material() {
        assert_eq!(extract_label(0), "Type a website, or attach brand files");
        assert_eq!(extract_label(1), "Extract from the attached file");
        assert_eq!(extract_label(3), "Extract from the 3 attached files");
    }

    #[test]
    fn a_row_time_keeps_the_date_the_minute_and_the_zone() {
        assert_eq!(short_time("2026-08-28T15:27:55Z"), "2026-08-28 15:27 UTC");
        // A session written before the app stored the times has none.
        assert_eq!(short_time(""), "");
        assert_eq!(short_time("odd"), "odd");
    }

    #[test]
    fn the_template_button_counts_what_is_chosen() {
        assert!(template_button_label(0, 0).starts_with("No templates yet"));
        assert!(template_button_label(2, 0).starts_with("None chosen"));
        assert_eq!(template_button_label(2, 1), "1 chosen");
        assert_eq!(template_button_label(5, 4), "4 chosen");
    }

    #[test]
    fn the_placeholder_names_every_kind() {
        // The composer no longer knows the kind: the picker asks after
        // the request is sent, so the hint must fit any answer.
        assert!(COMPOSER_PLACEHOLDER.contains("landing page"));
        assert!(COMPOSER_PLACEHOLDER.contains("deck"));
        assert!(COMPOSER_PLACEHOLDER.contains("memo"));
        assert!(COMPOSER_PLACEHOLDER.contains("carousel"));
    }

    #[test]
    fn every_kind_has_a_detail_line_in_the_picker() {
        assert!(kind_detail(ArtifactKind::Demo).contains("device canvas"));
        assert!(kind_detail(ArtifactKind::Deck).contains("1920 by 1080"));
        assert!(kind_detail(ArtifactKind::Document).contains("A4 or Letter"));
        assert!(kind_detail(ArtifactKind::Social).contains("carousel"));
        for choice in ArtifactKind::ALL {
            assert!(!kind_detail(choice).is_empty());
        }
    }
}
