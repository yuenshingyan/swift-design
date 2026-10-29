use super::*;

/// The page transition picker: the effect between screens, the direction
/// they travel, and how long the move takes.
///
/// Demos default to an instant cut; decks retain their optional scroll layout.
#[component]
pub(crate) fn TransitionForm(
    transition: Option<Transition>,
    on_change: EventHandler<Option<Transition>>,
    #[props(default)] default_cut: bool,
) -> Element {
    let current = transition.or_else(|| {
        default_cut.then_some(Transition {
            effect: TransitionEffect::None,
            ..Transition::default()
        })
    });
    let effect = current.map(|transition| transition.effect);
    let axis = current.unwrap_or_default().axis;
    let duration = current.unwrap_or_default().duration_ms;
    let pick_effect = use_callback(move |value: &'static str| {
        on_change.call(effect_from_value(value).map(|effect| Transition {
            effect,
            ..current.unwrap_or_default()
        }));
    });
    // Direction and duration show only for effects that move: a scroll
    // page and a cut have nothing to direct or time.
    rsx! {
        div { class: "sheet-section",
            div { class: "head", "Page transition" }
            div { class: "effect-chips",
                if !default_cut {
                    button {
                        class: if effect.is_none() { "selected" } else { "" },
                        title: "No transition: the deck scrolls",
                        onclick: move |_| pick_effect.call("scroll"),
                        "Scroll"
                    }
                }
                for (value, label) in TRANSITION_EFFECTS {
                    button {
                        key: "{value}",
                        class: if effect.map(TransitionEffect::as_str) == Some(value) { "selected" } else { "" },
                        onclick: move |_| pick_effect.call(value),
                        "{label}"
                    }
                }
            }
            if effect_uses_motion(effect) {
                div { class: "theme-grid",
                    div { class: "field",
                        span { class: "field-label", "Direction" }
                        Select {
                            value: if axis == TransitionAxis::Horizontal { "horizontal" } else { "vertical" },
                            options: vec![
                                ("vertical".to_owned(), "Vertical".to_owned()),
                                ("horizontal".to_owned(), "Horizontal".to_owned()),
                            ],
                            on_change: move |direction: String| {
                                let is_horizontal = direction == "horizontal";
                                let mut transition = current.unwrap_or_default();
                                transition.axis = if is_horizontal {
                                    TransitionAxis::Horizontal
                                } else {
                                    TransitionAxis::Vertical
                                };
                                on_change.call(Some(transition));
                            },
                        }
                    }
                    label {
                        "Duration (ms)"
                        input {
                            r#type: "number",
                            min: "0",
                            max: "{MAX_TRANSITION_MS}",
                            step: "50",
                            value: "{duration}",
                            oninput: move |event| {
                                let Ok(value) = event.value().parse::<u32>() else {
                                    return;
                                };
                                let value = value.min(MAX_TRANSITION_MS);
                                let mut transition = current.unwrap_or_default();
                                transition.duration_ms = value;
                                on_change.call(Some(transition));
                            },
                        }
                    }
                }
            }
            p { class: "inspector-hint",
                "Direction moves Push and Cover. Duration applies to every effect except Cut. "
                "A change saves at once and plays in the exported HTML. "
                "The editor preview shows one screen, so it does not animate."
            }
        }
    }
}

/// True when `effect` has a direction and a duration to set: every
/// effect except the scroll page (`None`) and a cut.
pub(super) fn effect_uses_motion(effect: Option<TransitionEffect>) -> bool {
    !matches!(effect, None | Some(TransitionEffect::None))
}

/// The transition effects the picker offers, as (JSON value, label).
const TRANSITION_EFFECTS: [(&str, &str); 5] = [
    ("none", "Cut"),
    ("fade", "Fade"),
    ("push", "Push"),
    ("cover", "Cover"),
    ("zoom", "Zoom"),
];

/// The effect a picker value names. `None` is the scroll page.
fn effect_from_value(value: &str) -> Option<TransitionEffect> {
    match value {
        "none" => Some(TransitionEffect::None),
        "fade" => Some(TransitionEffect::Fade),
        "push" => Some(TransitionEffect::Push),
        "cover" => Some(TransitionEffect::Cover),
        "zoom" => Some(TransitionEffect::Zoom),
        _ => None,
    }
}
