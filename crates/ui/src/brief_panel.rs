//! The brief panel: the versioned design brief, its three distinct
//! groups (confirmed facts, assumptions, open questions), the editable
//! fields, the revision list, and the approve/generate actions.

use design_model::{
    AnsweredQuestion, ArtifactKind, BriefRevision, BriefSection, DesignBrief, Viewport,
    WorkflowState,
};
use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::CountChips;
use crate::icons;
use crate::question_card::QaRow;
use crate::select::Select;

/// True while the kind may still change: before anything is generated.
pub(crate) fn can_change_kind(state: WorkflowState) -> bool {
    matches!(
        state,
        WorkflowState::Clarifying | WorkflowState::BriefReady | WorkflowState::AwaitingApproval
    )
}

/// One canvas the user can pick, as it appears in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlatformChoice {
    /// The value written to the brief.
    pub value: &'static str,
    /// The name under the icon.
    pub label: &'static str,
    /// The canvas size, for the caption.
    pub size: &'static str,
    /// The icon markup.
    pub icon: &'static str,
}

/// The canvases the app offers. The three cover every viewport the
/// renderer has, so a pick always resolves to a real canvas.
pub(crate) fn platform_choices() -> Vec<PlatformChoice> {
    vec![
        PlatformChoice {
            value: "desktop web",
            label: "Desktop",
            size: "1440 × 900",
            icon: icons::MONITOR,
        },
        PlatformChoice {
            value: "phone",
            label: "Phone",
            size: "390 × 844",
            icon: icons::PHONE,
        },
        PlatformChoice {
            value: "tablet",
            label: "Tablet",
            size: "1024 × 768",
            icon: icons::TABLET,
        },
    ]
}

/// The canvases the run has picked, as choice values.
///
/// Free text from a model still lands on a choice, because the viewport
/// decides which one it is. Never empty: no pick means the desktop
/// canvas, which is what the renderer defaults to.
pub(crate) fn picked_platforms(platforms: &[String]) -> Vec<String> {
    let choices = platform_choices();
    let mut picked: Vec<String> = Vec::new();
    for platform in platforms {
        let viewport = Viewport::for_platform(platform);
        let Some(choice) = choices
            .iter()
            .find(|choice| Viewport::for_platform(choice.value) == viewport)
        else {
            continue;
        };
        if !picked.iter().any(|value| value == choice.value) {
            picked.push(choice.value.to_owned());
        }
    }
    if picked.is_empty() {
        picked.push("desktop web".to_owned());
    }
    picked
}

/// The platforms after the user clicked `value`.
///
/// Clicking an unpicked canvas adds it; clicking a picked one removes
/// it. The last one cannot be removed: a run needs a canvas.
pub(crate) fn toggled_platforms(picked: &[String], value: &str) -> Vec<String> {
    if picked.iter().any(|platform| platform == value) {
        if picked.len() == 1 {
            return picked.to_vec();
        }
        return picked
            .iter()
            .filter(|platform| *platform != value)
            .cloned()
            .collect();
    }
    // Keep the offered order, so the tabs never jump about.
    platform_choices()
        .into_iter()
        .map(|choice| choice.value.to_owned())
        .filter(|choice| choice == value || picked.iter().any(|platform| platform == choice))
        .collect()
}

/// The slide counts the app offers, as (brief value, label). The empty
/// value leaves the length to the agent.
pub(crate) fn slide_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [5, 8, 10, 12, 15, 20, 30] {
        options.push((count.to_string(), format!("{count} slides")));
    }
    options
}

/// Which brief actions are enabled in the current state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BriefActions {
    /// Approve the brief and generate.
    pub can_approve: bool,
    /// Generate with the recorded assumptions.
    pub can_generate_with_assumptions: bool,
    /// Edit the brief fields.
    pub can_edit: bool,
    /// Critique the design.
    pub can_critique: bool,
    /// Retry after an error.
    pub can_retry: bool,
    /// Send a chat turn.
    pub can_chat: bool,
}

/// The actions for `state`. `can_proceed` is the question set's flag.
pub(crate) fn brief_actions_for(state: WorkflowState, can_proceed: bool) -> BriefActions {
    use WorkflowState::*;
    match state {
        Intake => BriefActions::default(),
        Clarifying => BriefActions {
            can_generate_with_assumptions: can_proceed,
            can_chat: true,
            ..BriefActions::default()
        },
        BriefReady | AwaitingApproval => BriefActions {
            can_approve: true,
            can_generate_with_assumptions: true,
            can_edit: true,
            can_chat: true,
            ..BriefActions::default()
        },
        Generating => BriefActions {
            can_edit: true,
            ..BriefActions::default()
        },
        Reviewing => BriefActions {
            can_edit: true,
            can_critique: true,
            can_chat: true,
            ..BriefActions::default()
        },
        Error => BriefActions {
            can_retry: true,
            can_chat: true,
            ..BriefActions::default()
        },
    }
}

/// The three brief groups, cleaned of blanks and duplicates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BriefGroups {
    /// Facts the user confirmed.
    pub facts: Vec<String>,
    /// Choices the app or the agent made.
    pub assumptions: Vec<String>,
    /// Questions still open.
    pub open: Vec<String>,
}

/// The three groups of `brief`, trimmed, de-duplicated, order kept.
///
/// The server drops the lines that repeat an answer or a brief field
/// before it serves a brief, so this only tidies whitespace and exact
/// repeats.
pub(crate) fn facts_assumptions_open(brief: &DesignBrief) -> BriefGroups {
    BriefGroups {
        facts: clean(&brief.confirmed_facts),
        assumptions: clean(&brief.assumptions),
        open: clean(&brief.open_questions),
    }
}

/// Trims, drops blanks, and de-duplicates a list, keeping order.
fn clean(items: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if !trimmed.is_empty() && !seen.iter().any(|kept| kept == trimmed) {
            seen.push(trimmed.to_owned());
        }
    }
    seen
}

/// Splits textarea text into a list, one item per non-blank line. Used
/// by the brief editor.
#[allow(dead_code)]
pub(crate) fn lines_to_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Joins a list back into textarea text, one item per line.
#[allow(dead_code)]
pub(crate) fn list_to_lines(items: &[String]) -> String {
    items.join("\n")
}

/// The label for a revision row: `r3 · user edit · 14:02`.
pub(crate) fn revision_label(entry: &BriefRevision) -> String {
    let time = entry.at.get(11..16).unwrap_or("");
    format!(
        "r{} · {} · {time}",
        entry.revision,
        entry.source.as_str().replace('_', " ")
    )
}

/// The readable name of a workflow state.
pub(crate) fn state_label(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Intake => "Intake",
        WorkflowState::Clarifying => "Clarifying",
        WorkflowState::BriefReady => "Brief ready",
        WorkflowState::AwaitingApproval => "Awaiting approval",
        WorkflowState::Generating => "Generating",
        WorkflowState::Reviewing => "Reviewing",
        WorkflowState::Error => "Error",
    }
}

/// True when the server accepts a user brief write in `state`: while
/// the brief is being settled, and while a result is under review.
pub(crate) fn is_restore_allowed(state: WorkflowState) -> bool {
    matches!(
        state,
        WorkflowState::Clarifying
            | WorkflowState::BriefReady
            | WorkflowState::AwaitingApproval
            | WorkflowState::Reviewing
    )
}

/// True when the viewed revision can be written back: it is not the
/// current one, and the state accepts a user edit.
pub(crate) fn is_restore_offered(state: WorkflowState, viewed: u32, current: u32) -> bool {
    viewed != current && is_restore_allowed(state)
}

/// The class of one revision row: the current revision, the one on
/// view, or a plain row.
pub(crate) fn revision_row_class(entry: u32, current: u32, viewed: Option<u32>) -> &'static str {
    if entry == current {
        "history-row current"
    } else if viewed == Some(entry) {
        "history-row selected"
    } else {
        "history-row"
    }
}

/// True when the brief panel offers to let the agent decide the open
/// items: only once a brief can be approved, and only while there is
/// something open. While the questions are still open, the question
/// card carries the skip instead.
pub(crate) fn is_decide_open_offered(actions: BriefActions, open_count: usize) -> bool {
    actions.can_approve && actions.can_generate_with_assumptions && open_count > 0
}

/// True when the brief panel starts collapsed: once a run starts, the
/// candidates are the thing to look at, and the brief is a reference.
pub(crate) fn is_brief_collapsed_by_default(state: WorkflowState) -> bool {
    matches!(state, WorkflowState::Generating | WorkflowState::Reviewing)
}

/// The brief panel for one session.
#[component]
pub(crate) fn BriefPanel(
    session_id: String,
    state: WorkflowState,
    brief: Option<DesignBrief>,
    revisions: Vec<BriefRevision>,
    answers: Vec<AnsweredQuestion>,
    options: api::SessionOptions,
    actions: BriefActions,
    on_error: EventHandler<String>,
) -> Element {
    let mut is_expanded = use_signal(|| false);
    // The user's own collapse choice. A state change clears it, so a
    // click on `Go` collapses the panel even after the user opened it.
    let mut collapse_choice = use_signal(|| None::<bool>);
    let mut seen_state = use_signal(|| state);
    if seen_state() != state {
        seen_state.set(state);
        collapse_choice.set(None);
    }
    let is_collapsed = collapse_choice().unwrap_or(is_brief_collapsed_by_default(state));
    // An old revision on view, fetched on a click on its row. The
    // current revision is always the panel's own `brief`.
    let mut viewed = use_signal(|| None::<(u32, DesignBrief)>);
    let view_revision = {
        let id = session_id.clone();
        use_callback(move |number: u32| {
            let id = id.clone();
            spawn(async move {
                match api::fetch_brief_revision(&id, number).await {
                    Ok(old) => viewed.set(Some((number, old))),
                    Err(error) => on_error.call(error),
                }
            });
        })
    };
    let restore_revision = {
        let id = session_id.clone();
        use_callback(move |number: u32| {
            let id = id.clone();
            spawn(async move {
                match api::restore_brief_revision(&id, number).await {
                    Ok(()) => viewed.set(None),
                    Err(error) => on_error.call(error),
                }
            });
        })
    };
    let Some(brief) = brief else {
        return rsx! {
            section { class: "brief-panel",
                div { class: "brief-head",
                    span { class: "kicker", "Brief" }
                }
                AnswersView { entries: answers }
                p { class: "lede", "The brief appears here once the questions are answered." }
            }
        };
    };
    let groups = facts_assumptions_open(&brief);
    let is_full_brief_offered = has_full_brief(&brief, &answers) || !revisions.is_empty();
    let revision = brief.revision;
    let open_count = groups.open.len();
    let viewed_value = viewed();
    let viewed_number = viewed_value.as_ref().map(|(number, _)| *number);
    let rows: Vec<(u32, &'static str, String, String)> = revisions
        .iter()
        .map(|entry| {
            (
                entry.revision,
                revision_row_class(entry.revision, revision, viewed_number),
                revision_label(entry),
                entry.summary.clone(),
            )
        })
        .collect();
    let approve = {
        let id = session_id.clone();
        move |_| {
            let id = id.clone();
            let on_error = on_error;
            spawn(async move {
                if let Err(error) = api::approve_brief(&id).await {
                    on_error.call(error);
                }
            });
        }
    };
    let generate = {
        let id = session_id.clone();
        move |_| {
            let id = id.clone();
            let on_error = on_error;
            spawn(async move {
                if let Err(error) = api::generate_with_assumptions(&id).await {
                    on_error.call(error);
                }
            });
        }
    };
    rsx! {
        section { class: if is_collapsed { "brief-panel collapsed" } else { "brief-panel" },
            button {
                class: "brief-head",
                onclick: move |_| collapse_choice.set(Some(!is_collapsed)),
                span { class: "chevron",
                    if is_collapsed {
                        "▸"
                    } else {
                        "▾"
                    }
                }
                span { class: "kicker", "Brief" }
                span { class: "kind-badge", "{brief.artifact_kind.label()}" }
                span { class: "rev", "rev {revision}" }
            }
            if !is_collapsed {
                AnswersView { entries: answers.clone() }
                BriefGroupsView { groups: groups.clone() }
                if is_full_brief_offered {
                    button {
                        class: "brief-toggle",
                        onclick: move |_| is_expanded.set(!is_expanded()),
                        if is_expanded() {
                            "Hide the full brief"
                        } else {
                            "Show the full brief"
                        }
                    }
                }
                if is_expanded() {
                    if let Some((number, old)) = viewed_value.clone() {
                        div { class: "revision-view",
                            span { class: "kicker", "Revision {number}" }
                            if is_restore_offered(state, number, revision) {
                                button {
                                    class: "secondary",
                                    onclick: move |_| restore_revision(number),
                                    "Restore this revision"
                                }
                            }
                            button {
                                class: "secondary",
                                onclick: move |_| viewed.set(None),
                                "Back to current"
                            }
                        }
                        BriefFields { brief: old, answers: answers.clone() }
                    } else {
                        BriefFields { brief: brief.clone(), answers: answers.clone() }
                    }
                    if !revisions.is_empty() {
                        div { class: "revision-list",
                            span { class: "kicker", "Revisions" }
                            for (number, row_class, label, summary) in rows.iter().cloned() {
                                button {
                                    key: "{number}",
                                    class: "{row_class}",
                                    onclick: move |_| {
                                        if number == revision {
                                            viewed.set(None);
                                        } else {
                                            view_revision(number);
                                        }
                                    },
                                    span { class: "revision-name", "{label}" }
                                    span { class: "revision-summary", "{summary}" }
                                }
                            }
                        }
                    }
                }
                RunSettings {
                    session_id: session_id.clone(),
                    state,
                    brief: brief.clone(),
                    options,
                    on_error,
                }
                div { class: "brief-actions",
                    button {
                        class: "primary",
                        disabled: !actions.can_approve,
                        onclick: approve,
                        "Go"
                    }
                    if is_decide_open_offered(actions, open_count) {
                        button { class: "secondary", onclick: generate, "Decide automatically" }
                    }
                }
            }
        }
    }
}

/// The three distinct brief groups.
#[component]
fn BriefGroupsView(groups: BriefGroups) -> Element {
    rsx! {
        div { class: "brief-groups",
            BriefGroup {
                class: "facts",
                title: "Confirmed by you",
                items: groups.facts,
            }
            BriefGroup {
                class: "assumptions",
                title: "Assumed",
                items: groups.assumptions,
            }
            BriefGroup { class: "open", title: "Still open", items: groups.open }
        }
    }
}

/// One brief group block.
#[component]
fn BriefGroup(class: String, title: String, items: Vec<String>) -> Element {
    // A heading over the word `none` tells the reader nothing. An empty
    // group is not drawn at all.
    if items.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "brief-group {class}",
            span { class: "brief-group-title", "{title}" }
            ul { class: "brief-list",
                for item in items {
                    li { "{item}" }
                }
            }
        }
    }
}

/// The answers the user gave, as the brief records them.
///
/// The brief keeps these apart from the confirmed facts, so the panel
/// can show a question as a question. Empty until the user answers.
#[component]
fn AnswersView(entries: Vec<AnsweredQuestion>) -> Element {
    if entries.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "brief-answers",
            span { class: "brief-group-title", "Your answers" }
            for (index, entry) in entries.iter().enumerate() {
                QaRow {
                    key: "{index}",
                    question: entry.question.clone(),
                    answer: entry.answer.clone(),
                    is_assumed: entry.is_assumed,
                }
            }
        }
    }
}

/// The settings the app asks for instead of the agent: how many
/// variations to write, which kind to build, and the canvas.
///
/// Each has a closed set of answers, so a control settles it in one
/// click. The agent never spends a clarification turn on them.
#[component]
fn RunSettings(
    session_id: String,
    state: WorkflowState,
    brief: DesignBrief,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    // The server refuses an options change once a run starts, and a
    // critique run edits one artifact, so the settings show only while
    // the brief is still the thing being decided.
    if !can_change_kind(state) {
        return rsx! {};
    }
    let count = options.variation_count();
    let save_options = {
        let id = session_id.clone();
        let options = options.clone();
        move |variations: usize| {
            let id = id.clone();
            let mut next = options.clone();
            next.variations = Some(variations);
            spawn(async move {
                if let Err(message) = api::save_session_options(&id, &next).await {
                    on_error.call(message);
                }
            });
        }
    };
    rsx! {
        div { class: "run-settings",
            span { class: "brief-group-title", "Run settings" }
            CountChips {
                label: "Variations",
                value: count,
                limit: api::VARIATION_LIMIT,
                on_change: save_options,
            }
            CanvasPicker {
                session_id: session_id.clone(),
                kind: brief.artifact_kind,
                options: options.clone(),
                on_error,
            }
        }
    }
}

/// The canvas control: which devices a demo is drawn for, or how many
/// slides a deck has. Both write a user brief revision.
///
/// A demo run writes one design per canvas, so this is a multiple pick.
#[component]
pub(crate) fn CanvasPicker(
    session_id: String,
    kind: ArtifactKind,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    if kind == ArtifactKind::Deck {
        return rsx! {
            SlideCountSelect { session_id, options, on_error }
        };
    }
    let picked = picked_platforms(&options.platforms);
    // A callback, not a closure: every button calls it, and each button
    // owns its own copy of the current picks.
    let save = use_callback(move |platforms: Vec<String>| {
        let mut next = options.clone();
        next.platforms = platforms;
        if next == options {
            return;
        }
        let id = session_id.clone();
        spawn(async move {
            if let Err(message) = api::save_session_options(&id, &next).await {
                on_error.call(message);
            }
        });
    });
    rsx! {
        div { class: "canvas-picker",
            span { class: "brief-field-label", "Canvas" }
            div { class: "device-choices",
                for choice in platform_choices() {
                    {
                        let is_picked = picked.iter().any(|value| value == choice.value);
                        let picked_now = picked.clone();
                        rsx! {
                            button {
                                key: "{choice.value}",
                                class: if is_picked { "device-choice picked" } else { "device-choice" },
                                title: "{choice.label} · {choice.size}",
                                "aria-pressed": "{is_picked}",
                                onclick: move |_| save.call(toggled_platforms(&picked_now, choice.value)),
                                span { class: "device-glyph", dangerous_inner_html: choice.icon }
                                span { class: "device-name", "{choice.label}" }
                                span { class: "device-size", "{choice.size}" }
                            }
                        }
                    }
                }
            }
            p { class: "device-note",
                if picked.len() > 1 {
                    "One design per canvas, in tabs."
                } else {
                    "Pick more than one to get a design for each."
                }
            }
        }
    }
}

/// How many slides a deck has. A closed set, so the app asks it.
#[component]
fn SlideCountSelect(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let current = options
        .slide_count
        .map(|count| count.to_string())
        .unwrap_or_default();
    rsx! {
        div { class: "brief-field kind-field",
            span { class: "brief-field-label", "Slides" }
            Select {
                value: current,
                options: slide_count_options(),
                on_change: move |value: String| {
                    let mut next = options.clone();
                    next.slide_count = value.parse::<u32>().ok();
                    if next == options {
                        return;
                    }
                    let id = session_id.clone();
                    spawn(async move {
                        if let Err(message) = api::save_session_options(&id, &next).await {
                            on_error.call(message);
                        }
                    });
                },
            }
        }
    }
}

/// The brief fields worth reading, as (label, value) rows.
///
/// A field an answer already states is left out: the answers sit above
/// it in the same panel, so printing `Audience: developers` under
/// `Q Who is this for?  A Developers` says nothing new.
pub(crate) fn extra_fields(
    brief: &DesignBrief,
    answers: &[AnsweredQuestion],
) -> Vec<(&'static str, String)> {
    [
        ("Target artifact", &brief.target_artifact),
        ("Audience", &brief.audience),
        ("User problem", &brief.user_problem),
        ("Primary job", &brief.primary_job),
        ("Success criterion", &brief.success_criterion),
        ("Visual direction", &brief.visual_direction),
    ]
    .into_iter()
    .filter(|(_, value)| !value.trim().is_empty())
    .filter(|(_, value)| {
        !answers
            .iter()
            .any(|entry| design_model::text::mostly_repeats(&entry.answer, value))
    })
    .map(|(label, value)| (label, value.clone()))
    .collect()
}

/// The brief lists worth reading, as (label, items) blocks. These carry
/// what the agent worked out from the conversation, which is what a
/// generation run reads.
pub(crate) fn extra_lists(brief: &DesignBrief) -> Vec<(&'static str, Vec<String>)> {
    // The required sections and the information architecture are the
    // same list twice: `App shell` and `Desktop app shell with sidebar`
    // name one screen. The sections carry the content, so they win.
    let architecture: Vec<String> = clean(&brief.information_architecture)
        .into_iter()
        .filter(|line| {
            !brief
                .required_sections
                .iter()
                .any(|section| design_model::text::repeats(line, &section.name))
        })
        .collect();
    [
        ("Information architecture", &architecture),
        ("Brand assets", &brief.brand_assets),
        (
            "Accessibility constraints",
            &brief.accessibility_constraints,
        ),
        ("Technical constraints", &brief.technical_constraints),
        ("Generation instructions", &brief.generation_instructions),
    ]
    .into_iter()
    .map(|(label, items)| (label, clean(items)))
    .filter(|(_, items)| !items.is_empty())
    .collect()
}

/// True when the full brief holds anything the panel does not already
/// show. Without this the toggle would open an empty box.
pub(crate) fn has_full_brief(brief: &DesignBrief, answers: &[AnsweredQuestion]) -> bool {
    !extra_fields(brief, answers).is_empty()
        || !extra_lists(brief).is_empty()
        || !brief.required_sections.is_empty()
}

/// The read-only brief fields. Editing is done from the chat for now.
#[component]
fn BriefFields(brief: DesignBrief, answers: Vec<AnsweredQuestion>) -> Element {
    let fields = extra_fields(&brief, &answers);
    let lists = extra_lists(&brief);
    rsx! {
        div { class: "brief-fields",
            for (label, value) in fields {
                BriefField { key: "{label}", label, value }
            }
            if !brief.required_sections.is_empty() {
                div { class: "brief-field",
                    span { class: "brief-field-label", "Required sections" }
                    for section in brief.required_sections.iter() {
                        SectionRow { section: section.clone() }
                    }
                }
            }
            for (label, items) in lists {
                div { key: "{label}", class: "brief-field",
                    span { class: "brief-field-label", "{label}" }
                    ul { class: "brief-list",
                        for item in items {
                            li { "{item}" }
                        }
                    }
                }
            }
        }
    }
}

/// One labelled brief field. Blank values are hidden.
#[component]
fn BriefField(label: String, value: String) -> Element {
    if value.trim().is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "brief-field",
            span { class: "brief-field-label", "{label}" }
            span { class: "brief-field-value", "{value}" }
        }
    }
}

/// One required-section row.
#[component]
fn SectionRow(section: BriefSection) -> Element {
    rsx! {
        div { class: "section-row",
            span { class: "section-name", "{section.name}" }
            span { class: "section-content", "{section.content}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use design_model::RevisionSource;

    #[test]
    fn approve_is_enabled_only_when_the_brief_awaits_approval() {
        assert!(brief_actions_for(WorkflowState::AwaitingApproval, false).can_approve);
        assert!(brief_actions_for(WorkflowState::BriefReady, false).can_approve);
        assert!(!brief_actions_for(WorkflowState::Clarifying, false).can_approve);
        assert!(!brief_actions_for(WorkflowState::Reviewing, false).can_approve);
    }

    #[test]
    fn generate_with_assumptions_follows_the_set_flag_while_clarifying() {
        assert!(brief_actions_for(WorkflowState::Clarifying, true).can_generate_with_assumptions);
        assert!(!brief_actions_for(WorkflowState::Clarifying, false).can_generate_with_assumptions);
        assert!(brief_actions_for(WorkflowState::BriefReady, false).can_generate_with_assumptions);
    }

    #[test]
    fn deciding_open_items_is_offered_only_with_open_items_and_an_approvable_brief() {
        let approvable = brief_actions_for(WorkflowState::AwaitingApproval, false);
        assert!(is_decide_open_offered(approvable, 2));
        assert!(!is_decide_open_offered(approvable, 0));
        let clarifying = brief_actions_for(WorkflowState::Clarifying, true);
        assert!(!is_decide_open_offered(clarifying, 2));
    }

    #[test]
    fn the_brief_collapses_once_a_run_starts() {
        assert!(is_brief_collapsed_by_default(WorkflowState::Generating));
        assert!(is_brief_collapsed_by_default(WorkflowState::Reviewing));
        assert!(!is_brief_collapsed_by_default(
            WorkflowState::AwaitingApproval
        ));
        assert!(!is_brief_collapsed_by_default(WorkflowState::Clarifying));
    }

    #[test]
    fn critique_is_enabled_only_while_reviewing() {
        assert!(brief_actions_for(WorkflowState::Reviewing, false).can_critique);
        assert!(!brief_actions_for(WorkflowState::Generating, false).can_critique);
    }

    #[test]
    fn retry_is_enabled_only_in_error() {
        assert!(brief_actions_for(WorkflowState::Error, false).can_retry);
        assert!(!brief_actions_for(WorkflowState::Reviewing, false).can_retry);
    }

    #[test]
    fn editing_stays_enabled_during_generation() {
        assert!(brief_actions_for(WorkflowState::Generating, false).can_edit);
        assert!(!brief_actions_for(WorkflowState::Intake, false).can_edit);
    }

    #[test]
    fn chat_is_off_during_intake_and_generation() {
        assert!(!brief_actions_for(WorkflowState::Intake, false).can_chat);
        assert!(!brief_actions_for(WorkflowState::Generating, false).can_chat);
        assert!(brief_actions_for(WorkflowState::Reviewing, false).can_chat);
    }

    #[test]
    fn groups_keep_facts_assumptions_and_open_questions_apart() {
        let brief = DesignBrief {
            confirmed_facts: vec!["  Web  ".to_owned(), "Web".to_owned(), String::new()],
            assumptions: vec!["Investors".to_owned()],
            open_questions: vec!["Colors?".to_owned()],
            ..DesignBrief::default()
        };
        let groups = facts_assumptions_open(&brief);
        assert_eq!(groups.facts, vec!["Web"]);
        assert_eq!(groups.assumptions, vec!["Investors"]);
        assert_eq!(groups.open, vec!["Colors?"]);
    }

    #[test]
    fn the_run_settings_close_once_generation_starts() {
        assert!(can_change_kind(WorkflowState::Clarifying));
        assert!(can_change_kind(WorkflowState::AwaitingApproval));
        assert!(!can_change_kind(WorkflowState::Generating));
        assert!(!can_change_kind(WorkflowState::Reviewing));
    }

    fn answered(question: &str, answer: &str) -> AnsweredQuestion {
        AnsweredQuestion {
            question: question.to_owned(),
            answer: answer.to_owned(),
            is_assumed: false,
        }
    }

    #[test]
    fn a_field_an_answer_already_states_is_not_shown_again() {
        let brief = DesignBrief {
            audience: "developers".to_owned(),
            primary_job: "organize project or coding work".to_owned(),
            user_problem: "tasks are scattered across repos".to_owned(),
            ..DesignBrief::default()
        };
        let answers = vec![
            answered("Who is this for?", "Developers"),
            answered("Main goal?", "Organize project or coding work"),
        ];
        let fields = extra_fields(&brief, &answers);
        assert_eq!(
            fields,
            vec![(
                "User problem",
                "tasks are scattered across repos".to_owned()
            )]
        );
        // With no answers, every filled field is worth showing.
        assert_eq!(extra_fields(&brief, &[]).len(), 3);
    }

    #[test]
    fn the_lists_carry_what_the_agent_worked_out() {
        let brief = DesignBrief {
            information_architecture: vec!["List".to_owned(), "  ".to_owned()],
            technical_constraints: vec!["No external fonts".to_owned()],
            ..DesignBrief::default()
        };
        let lists = extra_lists(&brief);
        assert_eq!(lists.len(), 2);
        assert_eq!(
            lists[0],
            ("Information architecture", vec!["List".to_owned()])
        );
        assert_eq!(lists[1].0, "Technical constraints");
        assert!(extra_lists(&DesignBrief::default()).is_empty());
    }

    #[test]
    fn architecture_lines_a_required_section_already_names_are_dropped() {
        let section = |name: &str| design_model::BriefSection {
            name: name.to_owned(),
            content: "x".to_owned(),
        };
        let brief = DesignBrief {
            required_sections: vec![section("App shell"), section("Task list")],
            information_architecture: vec![
                "Desktop app shell with sidebar navigation".to_owned(),
                "Task list view with filters and search".to_owned(),
                "Completed and active task state examples".to_owned(),
            ],
            ..DesignBrief::default()
        };
        let lists = extra_lists(&brief);
        assert_eq!(
            lists,
            vec![(
                "Information architecture",
                vec!["Completed and active task state examples".to_owned()]
            )]
        );
    }

    #[test]
    fn the_full_brief_is_offered_only_when_it_holds_something() {
        let empty = DesignBrief::default();
        assert!(!has_full_brief(&empty, &[]));
        let covered = DesignBrief {
            audience: "developers".to_owned(),
            ..DesignBrief::default()
        };
        assert!(!has_full_brief(&covered, &[answered("Who?", "Developers")]));
        assert!(has_full_brief(&covered, &[]));
    }

    #[test]
    fn the_groups_drop_blanks_and_exact_repeats() {
        let brief = DesignBrief {
            confirmed_facts: vec![
                "The audience is developers.".to_owned(),
                "  The audience is developers.  ".to_owned(),
                String::new(),
            ],
            ..DesignBrief::default()
        };
        let groups = facts_assumptions_open(&brief);
        assert_eq!(groups.facts, vec!["The audience is developers."]);
    }

    #[test]
    fn every_pick_maps_to_one_canvas() {
        // Free text from the model still lands on a choice.
        assert_eq!(
            picked_platforms(&["Desktop web app".to_owned()]),
            vec!["desktop web"]
        );
        assert_eq!(picked_platforms(&["iOS app".to_owned()]), vec!["phone"]);
        assert_eq!(picked_platforms(&["iPad".to_owned()]), vec!["tablet"]);
        // No pick is the desktop canvas, which is what the renderer
        // falls back to.
        assert_eq!(picked_platforms(&[]), vec!["desktop web"]);
        // The same canvas twice is one pick.
        assert_eq!(
            picked_platforms(&["phone".to_owned(), "iPhone".to_owned()]),
            vec!["phone"]
        );
        assert_eq!(platform_choices().len(), 3);
    }

    #[test]
    fn a_pick_toggles_and_the_last_one_stays() {
        let picked = vec!["desktop web".to_owned()];
        // Adding keeps the offered order, so the tabs never jump about.
        assert_eq!(
            toggled_platforms(&picked, "phone"),
            vec!["desktop web", "phone"]
        );
        let two = vec!["desktop web".to_owned(), "phone".to_owned()];
        assert_eq!(toggled_platforms(&two, "desktop web"), vec!["phone"]);
        // A run needs a canvas, so the last pick cannot be removed.
        assert_eq!(toggled_platforms(&picked, "desktop web"), picked);
        assert_eq!(
            toggled_platforms(&two, "tablet"),
            vec!["desktop web", "phone", "tablet"]
        );
    }

    #[test]
    fn the_first_slide_count_option_leaves_the_length_to_the_agent() {
        let options = slide_count_options();
        assert_eq!(options[0].0, "");
        assert_eq!(options[0].1, "The agent decides");
        assert!(options.iter().any(|(value, _)| value == "12"));
        // Every other value parses back to a slide count.
        for (value, _) in options.iter().skip(1) {
            assert!(value.parse::<u32>().is_ok());
        }
    }

    #[test]
    fn lines_round_trip_to_lists() {
        assert_eq!(lines_to_list("a\n\n b \n"), vec!["a", "b"]);
        assert_eq!(list_to_lines(&["a".to_owned(), "b".to_owned()]), "a\nb");
    }

    #[test]
    fn revision_labels_name_the_number_and_source() {
        let entry = BriefRevision {
            revision: 3,
            source: RevisionSource::UserEdit,
            summary: "x".to_owned(),
            at: "2026-08-26T14:02:00Z".to_owned(),
        };
        assert_eq!(revision_label(&entry), "r3 · user edit · 14:02");
    }

    #[test]
    fn restore_is_offered_only_for_an_older_revision_in_an_editable_state() {
        assert!(is_restore_offered(WorkflowState::AwaitingApproval, 1, 3));
        assert!(is_restore_offered(WorkflowState::Reviewing, 2, 3));
        assert!(!is_restore_offered(WorkflowState::AwaitingApproval, 3, 3));
        assert!(!is_restore_offered(WorkflowState::Generating, 1, 3));
        assert!(!is_restore_offered(WorkflowState::Error, 1, 3));
    }

    #[test]
    fn revision_rows_mark_the_current_and_the_viewed_one() {
        assert_eq!(revision_row_class(3, 3, Some(1)), "history-row current");
        assert_eq!(revision_row_class(1, 3, Some(1)), "history-row selected");
        assert_eq!(revision_row_class(2, 3, Some(1)), "history-row");
        assert_eq!(revision_row_class(2, 3, None), "history-row");
    }

    #[test]
    fn state_labels_are_readable() {
        assert_eq!(
            state_label(WorkflowState::AwaitingApproval),
            "Awaiting approval"
        );
        assert_eq!(state_label(WorkflowState::Error), "Error");
    }
}
