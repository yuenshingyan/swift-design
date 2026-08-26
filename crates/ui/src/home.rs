//! The landing page: a prompt box on top, the project list below.
//!
//! The user describes a design and submits. Swift Design names a project
//! from the prompt, saves the brief, starts the agent, and opens the
//! project studio. Existing projects are listed underneath, like a
//! design tool's home screen.

use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::ModelChip;
use crate::icons;
use crate::studio::{SettingsPanel, pause_briefly, project_groups};
use crate::uploads::{AttachButton, AttachmentChips};

/// The label on the template button: how many templates are chosen.
fn template_button_label(chosen: usize) -> String {
    match chosen {
        0 => "none".to_owned(),
        1 => "1 chosen".to_owned(),
        count => format!("{count} chosen"),
    }
}

/// A kebab-case project name from the first words of a prompt.
fn project_slug(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty() && !word.eq_ignore_ascii_case("candidate"))
        .take(4)
        .map(str::to_ascii_lowercase)
        .collect();
    if words.is_empty() {
        "design".to_owned()
    } else {
        words.join("-")
    }
}

/// `base`, or `base-2`, `base-3`, … when the name is taken.
fn unique_project_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|name| name == base) {
        return base.to_owned();
    }
    let mut counter = 2usize;
    loop {
        let candidate = format!("{base}-{counter}");
        if !taken.iter().any(|name| name == &candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// A readable name from a project slug: `swift-design-pitch` becomes
/// `Swift Design Pitch`.
fn project_display_name(slug: &str) -> String {
    slug.split('-')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The project row's count text: the chosen design, the candidates, or
/// both.
fn project_summary(members: &[api::DesignSummary]) -> String {
    let candidate_count = members
        .iter()
        .filter(|design| design.id.contains("-candidate-"))
        .count();
    let chosen = members
        .iter()
        .find(|design| !design.id.contains("-candidate-"));
    match (chosen, candidate_count) {
        (Some(design), 0) => format!("{} screens", design.screen_count),
        (Some(design), count) => format!("{} screens · {count} candidates", design.screen_count),
        (None, count) => format!("{count} candidates"),
    }
}

/// The landing page.
#[component]
pub fn Home(on_open_project: EventHandler<String>) -> Element {
    let mut designs = use_signal(Vec::<api::DesignSummary>::new);
    let mut settings = use_signal(|| Option::<api::SettingsView>::None);
    let mut is_configuring = use_signal(|| false);
    let mut is_loaded = use_signal(|| false);
    let mut prompt = use_signal(String::new);
    let effort = use_signal(|| "medium".to_owned());
    let mut templates = use_signal(Vec::<api::TemplateSummary>::new);
    let chosen_templates = use_signal(Vec::<String>::new);
    let mut is_picking_templates = use_signal(|| false);
    let mut error = use_signal(|| Option::<String>::None);
    let mut pending_delete = use_signal(|| Option::<String>::None);
    // The source files the next run reads. Uploads are shared by every
    // project, so the list is the same one the studio shows.
    let mut uploads = use_signal(Vec::<api::UploadSummary>::new);
    let refresh_uploads = use_callback(move |_: ()| {
        spawn(async move {
            match api::fetch_uploads().await {
                Ok(listing) => uploads.set(listing),
                Err(message) => error.set(Some(message)),
            }
        });
    });
    // Open the setup panel once when no model is chosen yet. The user
    // can close it and come back through the model chip.
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
    // The project being renamed and the draft name.
    let mut renaming = use_signal(|| Option::<(String, String)>::None);

    let commit_rename = use_callback(move |_: ()| {
        let Some((old, draft)) = renaming() else {
            return;
        };
        let new_name = draft.trim().to_owned();
        if new_name.is_empty() || new_name == old {
            renaming.set(None);
            return;
        }
        spawn(async move {
            match api::rename_project(&old, &new_name).await {
                Ok(_) => {
                    renaming.set(None);
                    error.set(None);
                }
                Err(message) => error.set(Some(message)),
            }
        });
    });

    // The live loop: refresh, then wait for the next change.
    use_future(move || async move {
        let mut seen = 0u64;
        loop {
            if let Ok(fetched) = api::fetch_design_list().await {
                designs.set(fetched);
            }
            if let Ok(fetched) = api::fetch_settings().await {
                settings.set(Some(fetched));
            }
            if let Ok(fetched) = api::fetch_templates().await {
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

    let create = move |_| {
        let text = prompt().trim().to_owned();
        if text.is_empty() {
            error.set(Some("Describe the design first.".to_owned()));
            return;
        }
        let taken: Vec<String> = project_groups(&designs())
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let project = unique_project_name(&project_slug(&text), &taken);
        let level = effort();
        let picked = chosen_templates();
        spawn(async move {
            let request = api::BriefRequest {
                prompt: &text,
                project: Some(&project),
                effort: &level,
                preview: true,
                templates: &picked,
            };
            match api::save_brief(&request).await {
                Ok(()) => {
                    error.set(None);
                    // Launch the run; one that is already active is fine.
                    if let Err(message) = api::start_agent_run().await
                        && !message.contains("already active")
                    {
                        error.set(Some(message));
                    }
                    on_open_project.call(project.clone());
                }
                Err(message) => error.set(Some(message)),
            }
        });
    };

    let projects = project_groups(&designs());
    let current_settings = settings().and_then(|view| view.current);
    let is_model_chosen = current_settings.is_some();
    let show_setup = is_loaded() && is_configuring();

    rsx! {
        main { class: "home",
            section { class: "home-hero",
                h1 { "What should the design cover?" }
                p { class: "lede", "Describe it once. The agent writes candidates you can pick from." }
            }
            section { class: "home-composer",
                div { class: "brief-card",
                    textarea {
                        placeholder: "Subject, audience, tone, and what to include…",
                        value: "{prompt()}",
                        oninput: move |event| prompt.set(event.value()),
                    }
                    AttachmentChips {
                        uploads: uploads(),
                        on_changed: move |_| refresh_uploads.call(()),
                        on_error: move |message| error.set(Some(message)),
                    }
                    div { class: "brief-footer",
                        div { class: "home-controls",
                            AttachButton {
                                on_uploaded: move |_| {
                                    error.set(None);
                                    refresh_uploads.call(());
                                },
                                on_error: move |message| error.set(Some(message)),
                            }
                            if !templates().is_empty() {
                                button {
                                    class: if chosen_templates().is_empty() { "template-button" } else { "template-button chosen" },
                                    title: "Templates: {template_button_label(chosen_templates().len())}. A template gives the design its theme and layout style; the content is still written for your prompt. Pick several and the candidates take one look each.",
                                    "aria-label": "Templates: {template_button_label(chosen_templates().len())}",
                                    onclick: move |_| is_picking_templates.set(true),
                                    span { dangerous_inner_html: icons::LAYOUT }
                                    if !chosen_templates().is_empty() {
                                        span { class: "template-button-count",
                                            "{chosen_templates().len()}"
                                        }
                                    }
                                }
                            }
                            span { class: "divider" }
                            ModelChip {
                                settings,
                                is_configuring,
                                effort: Some(effort),
                            }
                        }
                        button {
                            class: "primary",
                            disabled: !is_model_chosen,
                            onclick: create,
                            "Create →"
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
            if is_picking_templates() {
                TemplatePicker {
                    templates,
                    chosen: chosen_templates,
                    is_open: is_picking_templates,
                    on_error: move |message| error.set(Some(message)),
                }
            }
            section { class: "home-projects",
                div { class: "projects-head",
                    h2 { "Projects" }
                    span { class: "projects-count", "{projects.len()}" }
                }
                if projects.is_empty() && is_loaded() {
                    p { class: "home-empty", "No projects yet. Describe a design above to start one." }
                }
                for (index, (name, members)) in projects.into_iter().enumerate() {
                    div { class: "project-item", key: "{name}",
                        if index > 0 {
                            div { class: "project-rule" }
                        }
                        div {
                            class: "project-row",
                            tabindex: "0",
                            onclick: {
                                let name = name.clone();
                                move |_| {
                                    pending_delete.set(None);
                                    on_open_project.call(name.clone())
                                }
                            },
                            onkeydown: {
                                let name = name.clone();
                                move |event: Event<KeyboardData>| {
                                    if event.key() == Key::Enter && renaming().is_none() {
                                        pending_delete.set(None);
                                        on_open_project.call(name.clone())
                                    }
                                }
                            },
                            if let Some((target, draft)) = renaming().filter(|(target, _)| *target == name) {
                                input {
                                    class: "rename-input mono",
                                    value: "{draft}",
                                    autofocus: true,
                                    onclick: move |event: Event<MouseData>| event.stop_propagation(),
                                    oninput: {
                                        let target = target.clone();
                                        move |event| renaming.set(Some((target.clone(), event.value())))
                                    },
                                    onkeydown: move |event: Event<KeyboardData>| {
                                        if event.key() == Key::Enter {
                                            event.prevent_default();
                                            commit_rename.call(());
                                        } else if event.key() == Key::Escape {
                                            renaming.set(None);
                                        }
                                    },
                                }
                                button {
                                    class: "icon-button",
                                    title: "Save the name",
                                    onclick: move |event: Event<MouseData>| {
                                        event.stop_propagation();
                                        commit_rename.call(());
                                    },
                                    span { dangerous_inner_html: icons::CHECK }
                                }
                                button {
                                    class: "icon-button",
                                    title: "Cancel",
                                    onclick: move |event: Event<MouseData>| {
                                        event.stop_propagation();
                                        renaming.set(None);
                                    },
                                    "×"
                                }
                            } else {
                                span { class: "project-title", "{project_display_name(&name)}" }
                                span { class: "project-count mono", "{project_summary(&members)}" }
                                button {
                                    class: "row-rename",
                                    title: "Rename project",
                                    onclick: {
                                        let name = name.clone();
                                        move |event: Event<MouseData>| {
                                            event.stop_propagation();
                                            pending_delete.set(None);
                                            renaming.set(Some((name.clone(), name.clone())));
                                        }
                                    },
                                    span { dangerous_inner_html: icons::PENCIL }
                                }
                            }
                            button {
                                class: if pending_delete().as_deref() == Some(name.as_str()) { "row-delete confirm" } else { "row-delete" },
                                title: "Delete this project and its designs",
                                onclick: {
                                    let name = name.clone();
                                    let ids: Vec<String> = members.iter().map(|design| design.id.clone()).collect();
                                    move |event: Event<MouseData>| {
                                        event.stop_propagation();
                                        if pending_delete().as_deref() == Some(name.as_str()) {
                                            pending_delete.set(None);
                                            let ids = ids.clone();
                                            spawn(async move {
                                                for id in ids {
                                                    let _ = api::delete_design(&id).await;
                                                }
                                            });
                                        } else {
                                            pending_delete.set(Some(name.clone()));
                                        }
                                    }
                                },
                                if pending_delete().as_deref() == Some(name.as_str()) {
                                    "Delete?"
                                } else {
                                    "×"
                                }
                            }
                            span {
                                class: "project-chevron",
                                dangerous_inner_html: icons::CHEVRON_RIGHT,
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The template picker: every saved template as a thumbnail, each with
/// a checkbox. A click toggles the template. Several may be chosen, and
/// the candidates take one look each.
#[component]
fn TemplatePicker(
    templates: Signal<Vec<api::TemplateSummary>>,
    chosen: Signal<Vec<String>>,
    is_open: Signal<bool>,
    on_error: EventHandler<String>,
) -> Element {
    let mut chosen = chosen;
    let mut is_open = is_open;
    let mut pending_delete = use_signal(|| Option::<String>::None);
    let close = move |_| {
        pending_delete.set(None);
        is_open.set(false);
    };
    let saved = templates();
    let count = chosen().len();

    rsx! {
        div { class: "modal-backdrop", onclick: close }
        div {
            class: "modal",
            role: "dialog",
            aria_modal: true,
            aria_label: "Choose templates",
            div { class: "modal-head",
                span { class: "kicker", "Templates" }
                span { class: "modal-note",
                    if count == 0 {
                        "each candidate gets a new look"
                    } else if count == 1 {
                        "every candidate uses this look"
                    } else {
                        "candidates take one look each, in order"
                    }
                }
                button { class: "icon-button", title: "Close", onclick: close, "×" }
            }
            div { class: "modal-body",
                div { class: "template-grid",
                    for saved_template in saved.iter().cloned() {
                        {
                            let id = saved_template.id.clone();
                            let is_chosen = chosen().contains(&id);
                            let is_confirming = pending_delete().as_deref() == Some(id.as_str());
                            rsx! {
                                div {
                                    key: "{saved_template.id}",
                                    class: if is_chosen { "template-card chosen" } else { "template-card" },
                                    button {
                                        class: "template-card-hit",
                                        role: "checkbox",
                                        aria_checked: is_chosen,
                                        title: "{saved_template.name}",
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
                                    span { class: "template-check", aria_hidden: true,
                                        if is_chosen {
                                            "✓"
                                        }
                                    }
                                    button {
                                        class: if is_confirming { "template-card-delete confirm" } else { "template-card-delete" },
                                        title: "Delete this template",
                                        onclick: {
                                            let id = id.clone();
                                            move |_| {
                                                if !is_confirming {
                                                    pending_delete.set(Some(id.clone()));
                                                    return;
                                                }
                                                pending_delete.set(None);
                                                let id = id.clone();
                                                spawn(async move {
                                                    match api::delete_template(&id).await {
                                                        Ok(()) => {
                                                            let mut ids = chosen();
                                                            ids.retain(|chosen_id| chosen_id != &id);
                                                            chosen.set(ids);
                                                        }
                                                        Err(message) => on_error.call(message),
                                                    }
                                                });
                                            }
                                        },
                                        if is_confirming {
                                            "Delete?"
                                        } else {
                                            "×"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            div { class: "modal-foot",
                span { class: "step-count", "{template_button_label(count)}" }
                button { class: "primary", onclick: close, "Done" }
                if count > 0 {
                    button {
                        onclick: move |_| {
                            chosen.set(Vec::new());
                            pending_delete.set(None);
                        },
                        "Clear"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::api::DesignSummary;
    use crate::home::{
        project_display_name, project_slug, project_summary, template_button_label,
        unique_project_name,
    };

    #[test]
    fn the_template_button_counts_what_is_chosen() {
        assert_eq!(template_button_label(0), "none");
        assert_eq!(template_button_label(1), "1 chosen");
        assert_eq!(template_button_label(4), "4 chosen");
    }

    fn summary(id: &str) -> DesignSummary {
        DesignSummary {
            pending_count: 0,
            id: id.to_owned(),
            title: "T".to_owned(),
            theme: "slate".to_owned(),
            screen_count: 3,
            outline_count: 0,
        }
    }

    #[test]
    fn slugs_use_the_first_words_of_the_prompt() {
        assert_eq!(
            project_slug("Make a pitch design for a new product"),
            "make-a-pitch-design"
        );
        assert_eq!(project_slug("Q3 Review!"), "q3-review");
        assert_eq!(project_slug("   "), "design");
    }

    #[test]
    fn slugs_never_contain_the_candidate_marker() {
        assert!(!project_slug("candidate review design").contains("-candidate-"));
    }

    #[test]
    fn display_names_title_case_the_slug() {
        assert_eq!(
            project_display_name("swift-design-pitch"),
            "Swift Design Pitch"
        );
        assert_eq!(project_display_name("q3-review-2"), "Q3 Review 2");
    }

    #[test]
    fn taken_names_get_a_numeric_suffix() {
        let taken = vec!["pitch".to_owned(), "pitch-2".to_owned()];
        assert_eq!(unique_project_name("pitch", &taken), "pitch-3");
        assert_eq!(unique_project_name("talk", &taken), "talk");
    }

    #[test]
    fn summaries_count_candidates_or_show_the_chosen_design() {
        let candidates = [summary("talk-candidate-1"), summary("talk-candidate-2")];
        assert_eq!(project_summary(&candidates), "2 candidates");
        let chosen = [summary("talk")];
        assert_eq!(project_summary(&chosen), "3 screens");
        let both = [summary("talk"), summary("talk-candidate-1")];
        assert_eq!(project_summary(&both), "3 screens · 1 candidates");
    }
}
