use super::questions::{
    axes_for, axis_value, fixed_choices, is_still_suggested, is_wide_card, use_setup_picks,
};
use super::*;

/// The app's own questions for this kind, as cards of chips.
#[component]
pub(crate) fn SharedQuestions(
    session_id: String,
    kind: ArtifactKind,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    rsx! {
        for axis in axes_for(kind) {
            {
                // A pick on this page shows first. Otherwise a stored
                // answer shows on every visit, so a reload never hides
                // what the run will use. These fields have no server
                // default: absent means unanswered.
                let current = picks()
                    .get(axis.key)
                    .cloned()
                    .or_else(|| axis_value(&options, axis.key));
                let choices = fixed_choices(axis.choices);
                let is_wide = is_wide_card(&choices);
                let is_suggested = is_still_suggested(&options, &picks(), axis.key);
                rsx! {
                    ChoiceCard {
                        key: "{axis.key}",
                        label: axis.label,
                        current,
                        choices,
                        is_wide,
                        is_suggested,
                        allows_custom: true,
                        on_pick: move |value: String| pick.call((axis.key.to_owned(), value)),
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

/// The canvas control: which devices a demo is drawn for, how many
/// slides a deck has, how many pages a document has, or how many
/// frames a social has. All write a user brief revision.
///
/// A demo run writes one design per canvas, so this is a multiple pick.
#[component]
pub(crate) fn CanvasPicker(
    session_id: String,
    kind: ArtifactKind,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    match kind {
        ArtifactKind::Deck => {
            return rsx! {
                div { class: "deck-questions",
                    DeckQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Document => {
            return rsx! {
                div { class: "document-questions",
                    DocumentQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Social => {
            return rsx! {
                div { class: "social-questions",
                    SocialQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Print => {
            return rsx! {
                div { class: "print-questions",
                    PrintQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Mailing => {
            return rsx! {
                div { class: "mailing-questions",
                    MailingQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Campaign => {
            return rsx! {
                div { class: "campaign-questions",
                    CampaignQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Artwork => {
            return rsx! {
                div { class: "artwork-questions",
                    ArtworkQuestions { session_id, options, on_error }
                }
            };
        }
        ArtifactKind::Demo => {}
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
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        // The two long cards share one row, each at half width, so
        // they cost one row between them instead of two.
        div { class: "question-pair",
            ChoiceCard {
                label: "How should the colors read?",
                current: colors,
                choices: fixed_choices(&COLOR_MODES),
                is_suggested: is_colors_suggested,
                allows_custom: true,
                on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
            }
            ChoiceCard {
                label: "What scenario is the deck for?",
                current: shown("scenario"),
                choices: scenario_choices(),
                allows_custom: true,
                on_pick: move |value: String| pick.call(("scenario".to_owned(), value)),
            }
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How long should the deck be?",
            current: shown("slides"),
            choices: slide_count_options(),
            on_pick: move |value: String| pick.call(("slides".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
        }
    }
}

/// The app's own document questions: the colors, the variety, the
/// length in pages, the candidate count, and the evidence. Each is a
/// card of chips next to the agent's questions, in the same grid, like
/// `DeckQuestions`. A card starts blank: nothing is chosen until the
/// user picks a chip or `Use your best judgment`.
#[component]
pub(crate) fn DocumentQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        ChoiceCard {
            label: "How should the colors read?",
            current: colors,
            choices: fixed_choices(&COLOR_MODES),
            is_wide: true,
            is_suggested: is_colors_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How long should the document be?",
            current: shown("pages"),
            choices: page_count_options(),
            on_pick: move |value: String| pick.call(("pages".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
        }
    }
}

/// The app's own social questions: the colors, the variety, the
/// length in frames, the candidate count, and the evidence. Each is a
/// card of chips next to the agent's questions, in the same grid, like
/// `DocumentQuestions`. A card starts blank: nothing is chosen until
/// the user picks a chip or `Use your best judgment`.
#[component]
pub(crate) fn SocialQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        ChoiceCard {
            label: "How should the colors read?",
            current: colors,
            choices: fixed_choices(&COLOR_MODES),
            is_wide: true,
            is_suggested: is_colors_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many frames should it have?",
            current: shown("frames"),
            choices: frame_count_options(),
            on_pick: move |value: String| pick.call(("frames".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
        }
    }
}

/// The app's own print questions: the colors, the variety, the length
/// in sheets, the candidate count, and the evidence. Each is a card of
/// chips next to the agent's questions, in the same grid, like
/// `SocialQuestions`. A card starts blank: nothing is chosen until the
/// user picks a chip or `Use your best judgment`.
#[component]
pub(crate) fn PrintQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        ChoiceCard {
            label: "How should the colors read?",
            current: colors,
            choices: fixed_choices(&COLOR_MODES),
            is_wide: true,
            is_suggested: is_colors_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many sheets should it have?",
            current: shown("sheets"),
            choices: sheet_count_options(),
            on_pick: move |value: String| pick.call(("sheets".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
        }
    }
}

/// The app's own mailing questions: the colors, the variety, the
/// length in emails, the candidate count, and the evidence. Each is a
/// card of chips next to the agent's questions, in the same grid, like
/// `PrintQuestions`. A card starts blank: nothing is chosen until the
/// user picks a chip or `Use your best judgment`.
#[component]
pub(crate) fn MailingQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        ChoiceCard {
            label: "How should the colors read?",
            current: colors,
            choices: fixed_choices(&COLOR_MODES),
            is_wide: true,
            is_suggested: is_colors_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many emails are in the sequence?",
            current: shown("emails"),
            choices: email_count_options(),
            on_pick: move |value: String| pick.call(("emails".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
        }
    }
}

/// The campaign setup card: the colors, the variety, the length, the
/// candidates, and the evidence. The ad kind and the size sit in the
/// axis chips above.
#[component]
pub(crate) fn CampaignQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        ChoiceCard {
            label: "How should the colors read?",
            current: colors,
            choices: fixed_choices(&COLOR_MODES),
            is_wide: true,
            is_suggested: is_colors_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many ads are in the set?",
            current: shown("ads"),
            choices: ad_count_options(),
            on_pick: move |value: String| pick.call(("ads".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
        }
    }
}

/// The artwork setup card: the colors, the variety, the length, the
/// candidates, and the evidence. The cover kind and the size sit in
/// the axis chips above.
#[component]
pub(crate) fn ArtworkQuestions(
    session_id: String,
    options: api::SessionOptions,
    on_error: EventHandler<String>,
) -> Element {
    let (picks, pick) = use_setup_picks(session_id, options.clone(), on_error);
    let shown = |key: &str| picks().get(key).cloned();
    // A suggestion shows on the card as picked, so the user sees what
    // the planner read from the request.
    let suggested = |key: &str, value: Option<String>| {
        value.filter(|_| is_still_suggested(&options, &picks(), key))
    };
    let colors =
        shown("color_mode").or_else(|| suggested("color_mode", options.color_mode.clone()));
    let evidence = shown("evidence_style")
        .or_else(|| suggested("evidence_style", options.evidence_style.clone()));
    let is_colors_suggested = suggested("color_mode", options.color_mode.clone()).is_some();
    let is_evidence_suggested =
        suggested("evidence_style", options.evidence_style.clone()).is_some();
    rsx! {
        ChoiceCard {
            label: "How should the colors read?",
            current: colors,
            choices: fixed_choices(&COLOR_MODES),
            is_wide: true,
            is_suggested: is_colors_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("color_mode".to_owned(), value)),
        }
        ChoiceCard {
            label: "How different should the candidates be?",
            current: shown("variety"),
            choices: variety_choices(),
            on_pick: move |value: String| pick.call(("variety".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many covers are in the set?",
            current: shown("covers"),
            choices: cover_count_options(),
            on_pick: move |value: String| pick.call(("covers".to_owned(), value)),
        }
        ChoiceCard {
            label: "How many candidates should I write?",
            current: shown("candidates"),
            choices: candidate_choices(),
            on_pick: move |value: String| pick.call(("candidates".to_owned(), value)),
        }
        ChoiceCard {
            label: "How much does it lean on data?",
            current: evidence,
            choices: fixed_choices(&EVIDENCE_STYLES),
            is_suggested: is_evidence_suggested,
            allows_custom: true,
            on_pick: move |value: String| pick.call(("evidence_style".to_owned(), value)),
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
    /// True when the planner picked `current` from the request. The
    /// card says so, and the user can change it.
    #[props(default)]
    is_suggested: bool,
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
                if is_suggested {
                    span {
                        class: "suggested-tag",
                        title: "Read from your request. Pick a chip to change it.",
                        "suggested"
                    }
                }
            }
            div { class: "option-chips",
                for (value, text) in choices.into_iter().filter(|(value, _)| !is_judgment_choice(value)) {
                    button {
                        key: "{value}",
                        class: if current.as_deref() == Some(value.as_str()) { if is_suggested { "option-chip selected suggested" } else { "option-chip selected" } } else { "option-chip" },
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
pub(crate) fn typed_answer(current: Option<&str>, choices: &[(String, String)]) -> Option<String> {
    let value = current?;
    if value.is_empty() || choices.iter().any(|(known, _)| known == value) {
        return None;
    }
    Some(value.to_owned())
}
