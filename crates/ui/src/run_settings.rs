//! The run settings the app asks for instead of the agent: the canvas
//! for a demo, the scenario, the length, the candidate count, and the
//! variety for a deck, and the number of variations. Each has a closed
//! set of answers, so a chip settles it in one click.

use std::collections::HashSet;

use design_model::{
    AUDIENCES, ArtifactKind, COLOR_MODES, CUSTOM_ANSWER_LIMIT, DATA_STATES, DECK_SCENARIOS,
    DECK_VARIETY_LEVELS, DEMO_SCOPES, EVIDENCE_STYLES, PRODUCT_KINDS, SLIDE_DENSITIES, TONES,
    Viewport, WorkflowState,
};
use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::CountChips;
use crate::icons;

/// The readable name of a workflow state.
pub(crate) fn state_label(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Intake => "Starting",
        WorkflowState::Clarifying => "Questions",
        WorkflowState::Generating => "Generating",
        WorkflowState::Reviewing => "Reviewing",
        WorkflowState::Stopped => "Stopped",
        WorkflowState::Error => "Error",
    }
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

/// One app question's chips: the fixed choices, with a leading
/// `the agent decides` that the card draws as the dashed chip.
fn fixed_choices(choices: &[(&'static str, &'static str)]) -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    options.extend(
        choices
            .iter()
            .map(|(value, label)| ((*value).to_owned(), (*label).to_owned())),
    );
    options
}

/// Most label characters a card may hold before it takes the full row.
const NARROW_CARD_CHARACTERS: usize = 90;

/// Most chips a card may hold before it takes the full row.
const NARROW_CARD_CHIPS: usize = 5;

/// True when a card's chips need the full row. A card with few short
/// chips shares a row with its neighbours, so the question grid stays
/// short.
fn is_wide_card(choices: &[(String, String)]) -> bool {
    let chips = choices
        .iter()
        .filter(|(value, _)| !is_judgment_choice(value))
        .count();
    let characters: usize = choices
        .iter()
        .filter(|(value, _)| !is_judgment_choice(value))
        .map(|(_, label)| label.chars().count())
        .sum();
    chips > NARROW_CARD_CHIPS || characters > NARROW_CARD_CHARACTERS
}

/// One app-owned question: the field it writes, its wording, and its
/// fixed choices.
struct Axis {
    /// Which `SessionOptions` field the pick lands in.
    key: &'static str,
    /// The question as the user reads it.
    label: &'static str,
    /// The fixed choices, before the judgment chip is added.
    choices: &'static [(&'static str, &'static str)],
}

/// The app-owned questions for `kind`, in the order they are shown.
///
/// These recur in every session, so the app asks them from a fixed list
/// and the agent never invents options for them. The agent may still
/// add questions this list does not cover.
fn axes_for(kind: ArtifactKind) -> Vec<Axis> {
    let mut axes = vec![
        Axis {
            key: "audience",
            label: "Who is it for?",
            choices: &AUDIENCES,
        },
        Axis {
            key: "tone",
            label: "What tone should it have?",
            choices: &TONES,
        },
    ];
    match kind {
        ArtifactKind::Demo => axes.extend([
            Axis {
                key: "color_mode",
                label: "How should the colors read?",
                choices: &COLOR_MODES,
            },
            Axis {
                key: "scope",
                label: "How much should I build?",
                choices: &DEMO_SCOPES,
            },
            Axis {
                key: "product_kind",
                label: "What kind of product is it?",
                choices: &PRODUCT_KINDS,
            },
            Axis {
                key: "data_state",
                label: "What should the screens show?",
                choices: &DATA_STATES,
            },
        ]),
        // A deck draws its colors card beside the scenario card, and
        // its evidence card last, so `DeckQuestions` draws both.
        ArtifactKind::Deck => axes.push(Axis {
            key: "slide_density",
            label: "How much goes on a slide?",
            choices: &SLIDE_DENSITIES,
        }),
    }
    axes
}

/// The value stored for `key`, if any.
fn axis_value(options: &api::SessionOptions, key: &str) -> Option<String> {
    match key {
        "audience" => options.audience.clone(),
        "tone" => options.tone.clone(),
        "color_mode" => options.color_mode.clone(),
        "scope" => options.scope.clone(),
        "product_kind" => options.product_kind.clone(),
        "data_state" => options.data_state.clone(),
        "slide_density" => options.slide_density.clone(),
        "evidence_style" => options.evidence_style.clone(),
        _ => None,
    }
}

/// The options with `key` set to `value`. An empty value clears it,
/// which leaves that axis to the agent.
fn with_axis(options: &api::SessionOptions, key: &str, value: String) -> api::SessionOptions {
    let mut next = options.clone();
    let picked = (!value.is_empty()).then_some(value);
    match key {
        "audience" => next.audience = picked,
        "tone" => next.tone = picked,
        "color_mode" => next.color_mode = picked,
        "scope" => next.scope = picked,
        "product_kind" => next.product_kind = picked,
        "data_state" => next.data_state = picked,
        "slide_density" => next.slide_density = picked,
        "evidence_style" => next.evidence_style = picked,
        _ => {}
    }
    next
}

/// The app's own questions for this kind, as cards of chips.
#[component]
pub(crate) fn SharedQuestions(
    session_id: String,
    kind: ArtifactKind,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    // The cards the user has touched. A server default is not an answer.
    let mut picked = use_signal(HashSet::<String>::new);
    let saved = options.clone();
    let save = use_callback(move |next: api::SessionOptions| {
        if next == saved {
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
        for axis in axes_for(kind) {
            {
                let options = options.clone();
                let key = axis.key.to_owned();
                // A stored answer shows on every visit, so a reload
                // never hides what the run will use. These fields have
                // no server default: absent means unanswered. The
                // judgment choice stores nothing, so it is remembered
                // for this page only.
                let current = axis_value(&options, axis.key)
                    .or_else(|| picked().contains(axis.key).then(String::new));
                let choices = fixed_choices(axis.choices);
                let is_wide = is_wide_card(&choices);
                rsx! {
                    ChoiceCard {
                        key: "{axis.key}",
                        label: axis.label,
                        current,
                        choices,
                        is_wide,
                        allows_custom: true,
                        on_pick: move |value: String| {
                            picked.write().insert(key.clone());
                            save.call(with_axis(&options, &key, value));
                        },
                    }
                }
            }
        }
    }
}

/// The settings for a demo before the first candidates exist: how many
/// variations, and which canvases. A deck asks its own questions on
/// the question card instead.
#[component]
pub(crate) fn RunSettings(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
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
                kind: ArtifactKind::Demo,
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
            div { class: "deck-questions",
                DeckQuestions { session_id, options, on_error }
            }
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

/// The scenario choices as (value, label): the agent decides, then
/// the presets.
pub(crate) fn scenario_choices() -> Vec<(String, String)> {
    let mut choices = vec![(String::new(), "The agent decides".to_owned())];
    choices.extend(
        DECK_SCENARIOS
            .iter()
            .map(|name| ((*name).to_owned(), (*name).to_owned())),
    );
    choices
}

/// The candidate count choices as (value, label), 1 to the limit.
pub(crate) fn candidate_choices() -> Vec<(String, String)> {
    (1..=api::VARIATION_LIMIT)
        .map(|count| {
            let label = if count == 1 {
                "1 candidate".to_owned()
            } else {
                format!("{count} candidates")
            };
            (count.to_string(), label)
        })
        .collect()
}

/// The variety choices as (value, label), from the shared levels.
pub(crate) fn variety_choices() -> Vec<(String, String)> {
    DECK_VARIETY_LEVELS
        .iter()
        .map(|(value, label)| ((*value).to_owned(), (*label).to_owned()))
        .collect()
}

/// The app's own deck questions, the Swift Deck way: the scenario,
/// the length, the candidate count, and the variety. Each is a card
/// of chips next to the agent's questions, in the same grid. A card
/// starts blank: nothing is chosen until the user picks a chip or
/// `Use your best judgment`. A pick saves the session options at once;
/// judgment saves the server default.
#[component]
pub(crate) fn DeckQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    // The cards the user has touched on this page. The server keeps
    // defaults for the rest, and a default is not an answer.
    let mut picked = use_signal(HashSet::<&'static str>::new);
    let saved = options.clone();
    let save = use_callback(move |next: api::SessionOptions| {
        if next == saved {
            return;
        }
        let id = session_id.clone();
        spawn(async move {
            if let Err(message) = api::save_session_options(&id, &next).await {
                on_error.call(message);
            }
        });
    });
    let shown = |key: &'static str, value: String| picked().contains(key).then_some(value);
    let scenario = shown("scenario", options.scenario.clone().unwrap_or_default());
    let slides = shown(
        "slides",
        options
            .slide_count
            .map(|count| count.to_string())
            .unwrap_or_default(),
    );
    let candidates = shown(
        "candidates",
        options
            .variations
            .map(|count| count.to_string())
            .unwrap_or_default(),
    );
    let variety = shown("variety", options.variety.clone());
    let evidence = shown(
        "evidence",
        options.evidence_style.clone().unwrap_or_default(),
    );
    let colors = shown("colors", options.color_mode.clone().unwrap_or_default());
    let pick_colors = {
        let options = options.clone();
        move |value: String| {
            picked.write().insert("colors");
            save.call(with_axis(&options, "color_mode", value));
        }
    };
    let pick_evidence = {
        let options = options.clone();
        move |value: String| {
            picked.write().insert("evidence");
            save.call(with_axis(&options, "evidence_style", value));
        }
    };
    let pick_scenario = {
        let options = options.clone();
        move |value: String| {
            picked.write().insert("scenario");
            let mut next = options.clone();
            next.scenario = (!value.is_empty()).then_some(value);
            save.call(next);
        }
    };
    let pick_slides = {
        let options = options.clone();
        move |value: String| {
            picked.write().insert("slides");
            let mut next = options.clone();
            next.slide_count = value.parse::<u32>().ok();
            save.call(next);
        }
    };
    let pick_candidates = {
        let options = options.clone();
        move |value: String| {
            picked.write().insert("candidates");
            let mut next = options.clone();
            next.variations = value.parse::<usize>().ok();
            save.call(next);
        }
    };
    let pick_variety = {
        let options = options.clone();
        move |value: String| {
            picked.write().insert("variety");
            let mut next = options.clone();
            next.variety = if value.is_empty() {
                "medium".to_owned()
            } else {
                value
            };
            save.call(next);
        }
    };
    rsx! {
        // The two long cards share one row, each at half width, so
        // they cost one row between them instead of two.
        div { class: "question-pair",
            ChoiceCard {
                label: "How should the colors read?",
                current: colors,
                choices: fixed_choices(&COLOR_MODES),
                allows_custom: true,
                on_pick: pick_colors,
            }
            ChoiceCard {
                label: "What scenario is the deck for?",
                current: scenario,
                choices: scenario_choices(),
                allows_custom: true,
                on_pick: pick_scenario,
            }
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: variety,
            choices: variety_choices(),
            on_pick: pick_variety,
        }
        ChoiceCard {
            label: "How long should the deck be?",
            current: slides,
            choices: slide_count_options(),
            on_pick: pick_slides,
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: candidates,
            choices: candidate_choices(),
            on_pick: pick_candidates,
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            allows_custom: true,
            on_pick: pick_evidence,
        }
    }
}

/// True when a choice is the judgment one: an empty value stands for
/// `the agent decides`, and the card draws it as the dashed chip.
pub(crate) fn is_judgment_choice(value: &str) -> bool {
    value.is_empty()
}

/// One app question as a card of chips, styled like the agent's
/// questions. `current` is `None` until the user picks; `Some("")`
/// means the user chose `Use your best judgment`.
#[component]
fn ChoiceCard(
    label: &'static str,
    current: Option<String>,
    choices: Vec<(String, String)>,
    /// True for a card with many chips: it takes the full row.
    #[props(default)]
    is_wide: bool,
    /// True when the user may type an answer the chips do not carry.
    #[props(default)]
    allows_custom: bool,
    on_pick: EventHandler<String>,
) -> Element {
    let is_judgment = current.as_deref() == Some("");
    let card_class = if is_wide {
        "question-card app-question wide"
    } else {
        "question-card app-question"
    };
    let typed_answer = typed_answer(current.as_deref(), &choices);
    let mut is_typing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let mut commit = move |()| {
        let answer = draft().trim().to_owned();
        if answer.is_empty() {
            return;
        }
        is_typing.set(false);
        draft.set(String::new());
        on_pick.call(answer);
    };
    rsx! {
        div { class: "{card_class}",
            div { class: "question-head",
                span { class: "question-label", "{label}" }
            }
            div { class: "option-chips",
                for (value, text) in choices.into_iter().filter(|(value, _)| !is_judgment_choice(value)) {
                    button {
                        key: "{value}",
                        class: if current.as_deref() == Some(value.as_str()) { "option-chip selected" } else { "option-chip" },
                        "aria-pressed": if current.as_deref() == Some(value.as_str()) { "true" } else { "false" },
                        onclick: move |_| on_pick.call(value.clone()),
                        "{text}"
                    }
                }
                // An answer the user typed reads as a chip of its own,
                // so the card still shows what the run will use.
                if let Some(answer) = typed_answer.clone() {
                    button {
                        class: "option-chip selected",
                        "aria-pressed": "true",
                        onclick: move |_| {
                            draft.set(answer.clone());
                            is_typing.set(true);
                        },
                        "{answer}"
                    }
                }
                if allows_custom && !is_typing() {
                    button {
                        class: "option-chip write-in",
                        onclick: move |_| is_typing.set(true),
                        "Something else…"
                    }
                }
                button {
                    class: if is_judgment { "option-chip skip selected" } else { "option-chip skip" },
                    onclick: move |_| on_pick.call(String::new()),
                    "Use your best judgment"
                }
            }
            if allows_custom && is_typing() {
                div { class: "write-in-row",
                    input {
                        class: "write-in-field",
                        r#type: "text",
                        autofocus: true,
                        maxlength: CUSTOM_ANSWER_LIMIT as i64,
                        placeholder: "Type your own answer",
                        value: "{draft()}",
                        oninput: move |event| draft.set(event.value()),
                        onkeydown: move |event: Event<KeyboardData>| {
                            if event.key() == Key::Enter {
                                event.prevent_default();
                                commit(());
                            }
                            if event.key() == Key::Escape {
                                is_typing.set(false);
                                draft.set(String::new());
                            }
                        },
                    }
                    button {
                        class: "option-chip write-in-save",
                        disabled: draft().trim().is_empty(),
                        onclick: move |_| commit(()),
                        "Use this"
                    }
                }
            }
        }
    }
}

/// The answer the user typed, when the current value is not one of the
/// chips. An empty value is the judgment choice, not a typed answer.
fn typed_answer(current: Option<&str>, choices: &[(String, String)]) -> Option<String> {
    let value = current?;
    if value.is_empty() || choices.iter().any(|(known, _)| known == value) {
        return None;
    }
    Some(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_axis_offers_the_judgment_choice_first() {
        let choices = fixed_choices(&AUDIENCES);
        assert_eq!(choices.len(), AUDIENCES.len() + 1);
        assert_eq!(choices[0], (String::new(), "The agent decides".to_owned()));
        assert!(is_judgment_choice(&choices[0].0));
        assert_eq!(choices[1].0, "newcomers".to_owned());
        assert_eq!(choices[1].1, "Newcomers to the subject".to_owned());
    }

    #[test]
    fn a_card_takes_the_full_row_only_when_its_chips_need_it() {
        // Few short chips share a row.
        assert!(!is_wide_card(&fixed_choices(&EVIDENCE_STYLES)));
        assert!(!is_wide_card(&fixed_choices(&SLIDE_DENSITIES)));
        assert!(!is_wide_card(&fixed_choices(&DATA_STATES)));
        // Many chips, or long ones, take the row.
        assert!(is_wide_card(&fixed_choices(&COLOR_MODES)));
        assert!(is_wide_card(&fixed_choices(&PRODUCT_KINDS)));
        assert!(is_wide_card(&fixed_choices(&AUDIENCES)));
        assert!(is_wide_card(&scenario_choices()));
        // The judgment choice is drawn as a chip too, but it is short
        // and always present, so it does not count.
        assert!(!is_wide_card(&[(
            String::new(),
            "The agent decides".to_owned()
        )]));
    }

    #[test]
    fn a_value_outside_the_chips_reads_as_a_typed_answer() {
        let choices = fixed_choices(&TONES);
        assert_eq!(
            typed_answer(Some("wry, like a changelog"), &choices),
            Some("wry, like a changelog".to_owned())
        );
        // A chip value is a chip, not a typed answer.
        assert_eq!(typed_answer(Some("playful"), &choices), None);
        // An empty value is the judgment choice.
        assert_eq!(typed_answer(Some(""), &choices), None);
        // Nothing picked yet.
        assert_eq!(typed_answer(None, &choices), None);
    }

    #[test]
    fn the_fixed_axes_never_change_between_runs() {
        // The whole point of hardcoding them: two calls are identical.
        for axis in [&AUDIENCES[..], &TONES[..], &DEMO_SCOPES[..]] {
            assert_eq!(fixed_choices(axis), fixed_choices(axis));
        }
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
    fn the_empty_choice_is_the_judgment_one() {
        assert!(is_judgment_choice(""));
        assert!(!is_judgment_choice("5"));
    }

    #[test]
    fn the_deck_choice_lists_start_with_the_agent_and_use_the_option_values() {
        let scenarios = scenario_choices();
        assert_eq!(scenarios[0].1, "The agent decides");
        assert_eq!(scenarios.len(), DECK_SCENARIOS.len() + 1);
        let candidates = candidate_choices();
        assert_eq!(candidates[0], ("1".to_owned(), "1 candidate".to_owned()));
        assert_eq!(candidates[candidates.len() - 1].1, "5 candidates");
        let variety = variety_choices();
        assert_eq!(variety[1].0, "medium");
    }

    #[test]
    fn state_labels_are_readable() {
        assert_eq!(state_label(WorkflowState::Clarifying), "Questions");
        assert_eq!(state_label(WorkflowState::Error), "Error");
        assert_eq!(state_label(WorkflowState::Stopped), "Stopped");
    }
}
