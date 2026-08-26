//! A dropdown that matches the Swift Design chrome.
//!
//! The browser's own `select` paints its menu in the system style, so
//! every picker renders as a trigger button plus a popover list. Arrow
//! keys on the trigger step through the options, Escape and a click
//! outside close the list.

use std::sync::atomic::{AtomicUsize, Ordering};

use dioxus::document;
use dioxus::prelude::*;

use crate::icons;

/// Counter behind each trigger's DOM id, so the measuring script can
/// find the right button.
static NEXT_TRIGGER_ID: AtomicUsize = AtomicUsize::new(0);

/// A fresh DOM id for a popover trigger, like `trigger-3`.
pub(crate) fn next_trigger_id() -> String {
    format!(
        "trigger-{}",
        NEXT_TRIGGER_ID.fetch_add(1, Ordering::Relaxed)
    )
}

/// The trigger's place on screen: `[left, top, bottom, width,
/// viewport_height]` in CSS pixels.
pub(crate) type Anchor = [f64; 5];

/// Measures the element with `id`. `None` when the page did not answer.
pub(crate) async fn measure_trigger(id: String) -> Option<Anchor> {
    let mut channel = document::eval(MEASURE_TRIGGER);
    channel.send(id).ok()?;
    channel.recv::<Anchor>().await.ok()
}

/// The option `value` comes before or after in `options`: the next one
/// for `step` of `1`, the previous for `-1`, clamped at the ends. The
/// first option when `value` is not listed.
fn stepped_option(options: &[(String, String)], value: &str, step: i32) -> Option<String> {
    let position = options.iter().position(|(candidate, _)| candidate == value);
    let target = match position {
        Some(index) if step < 0 => index.saturating_sub(1),
        Some(index) => (index + 1).min(options.len().saturating_sub(1)),
        None => 0,
    };
    options.get(target).map(|(candidate, _)| candidate.clone())
}

/// The label shown for `value`: the matching option's label, else the
/// value itself.
fn label_for(options: &[(String, String)], value: &str) -> String {
    options
        .iter()
        .find(|(candidate, _)| candidate == value)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| value.to_owned())
}

/// Reads the trigger's place on screen, so the list can sit beside it
/// with `position: fixed` and escape any clipped container. Posts
/// `[left, top, bottom, width, viewport_height]`.
const MEASURE_TRIGGER: &str = "\
const id = await dioxus.recv();
const element = document.getElementById(id);
const rect = element ? element.getBoundingClientRect() : { left: 0, top: 0, bottom: 0, width: 0 };
dioxus.send([rect.left, rect.top, rect.bottom, rect.width, window.innerHeight]);
";

/// The inline style that pins a popover to its trigger: below it, or
/// above it when `opens_up`.
pub(crate) fn menu_style(rect: Anchor, opens_up: bool) -> String {
    let [left, top, bottom, width, viewport_height] = rect;
    if opens_up {
        format!(
            "left: {left:.0}px; bottom: {:.0}px; min-width: {width:.0}px",
            viewport_height - top + 4.0
        )
    } else {
        format!(
            "left: {left:.0}px; top: {:.0}px; min-width: {width:.0}px",
            bottom + 4.0
        )
    }
}

/// A dropdown picker. `options` are `(value, label)` pairs; `value` is
/// the chosen one. `opens_up` puts the list above the trigger, for
/// pickers at the bottom of a column.
#[component]
pub(crate) fn Select(
    value: String,
    options: Vec<(String, String)>,
    on_change: EventHandler<String>,
    #[props(default)] opens_up: bool,
    #[props(default)] title: Option<String>,
) -> Element {
    // The list shows while this holds the trigger's measured rectangle.
    let mut anchor = use_signal(|| Option::<Anchor>::None);
    let trigger_id = use_hook(next_trigger_id);
    let label = label_for(&options, &value);
    let is_open = anchor().is_some();
    let keyboard_options = options.clone();
    let keyboard_value = value.clone();
    let measure_id = trigger_id.clone();
    let toggle = move |_| {
        if anchor().is_some() {
            anchor.set(None);
            return;
        }
        let id = measure_id.clone();
        spawn(async move {
            if let Some(rect) = measure_trigger(id).await {
                anchor.set(Some(rect));
            }
        });
    };
    rsx! {
        div { class: "select",
            button {
                id: "{trigger_id}",
                r#type: "button",
                class: "select-trigger",
                "aria-haspopup": "listbox",
                "aria-expanded": "{is_open}",
                title,
                onclick: toggle,
                onkeydown: move |event: Event<KeyboardData>| {
                    let step = match event.key() {
                        Key::ArrowDown => 1,
                        Key::ArrowUp => -1,
                        Key::Escape => {
                            anchor.set(None);
                            return;
                        }
                        _ => return,
                    };
                    event.prevent_default();
                    if let Some(next) = stepped_option(&keyboard_options, &keyboard_value, step) {
                        on_change.call(next);
                    }
                },
                span { class: "select-value", "{label}" }
                span {
                    class: "select-chevron",
                    dangerous_inner_html: icons::CHEVRON_DOWN,
                }
            }
            if let Some(rect) = anchor() {
                div {
                    class: "menu-backdrop",
                    onclick: move |event: Event<MouseData>| {
                        // A label around the picker would forward this click
                        // to the trigger and reopen the list.
                        event.prevent_default();
                        anchor.set(None);
                    },
                }
                div {
                    class: "select-menu",
                    role: "listbox",
                    style: "{menu_style(rect, opens_up)}",
                    for (option_value, option_label) in options.iter().cloned() {
                        button {
                            key: "{option_value}",
                            r#type: "button",
                            role: "option",
                            class: if option_value == value { "select-option selected" } else { "select-option" },
                            "aria-selected": "{option_value == value}",
                            onclick: {
                                let option_value = option_value.clone();
                                move |_| {
                                    anchor.set(None);
                                    on_change.call(option_value.clone());
                                }
                            },
                            span { "{option_label}" }
                            if option_value == value {
                                span {
                                    class: "tick",
                                    dangerous_inner_html: icons::CHECK,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `(value, label)` pairs where the label is the value, for plain
/// string lists like model ids and font families.
pub(crate) fn plain_options<I, S>(values: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    values
        .into_iter()
        .map(|value| {
            let value: String = value.into();
            (value.clone(), value)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Vec<(String, String)> {
        vec![
            ("low".to_owned(), "Low".to_owned()),
            ("medium".to_owned(), "Medium".to_owned()),
            ("high".to_owned(), "High".to_owned()),
        ]
    }

    #[test]
    fn arrow_steps_stay_inside_the_list() {
        assert_eq!(
            stepped_option(&options(), "low", 1).as_deref(),
            Some("medium")
        );
        assert_eq!(
            stepped_option(&options(), "high", 1).as_deref(),
            Some("high")
        );
        assert_eq!(
            stepped_option(&options(), "low", -1).as_deref(),
            Some("low")
        );
        assert_eq!(
            stepped_option(&options(), "medium", -1).as_deref(),
            Some("low")
        );
        assert_eq!(
            stepped_option(&options(), "other", 1).as_deref(),
            Some("low")
        );
        assert_eq!(stepped_option(&[], "low", 1), None);
    }

    #[test]
    fn labels_fall_back_to_the_value() {
        assert_eq!(label_for(&options(), "medium"), "Medium");
        assert_eq!(label_for(&options(), "custom"), "custom");
    }

    #[test]
    fn menu_styles_pin_the_list_below_or_above_the_trigger() {
        let rect = [10.0, 100.0, 130.0, 80.0, 900.0];
        assert_eq!(
            menu_style(rect, false),
            "left: 10px; top: 134px; min-width: 80px"
        );
        assert_eq!(
            menu_style(rect, true),
            "left: 10px; bottom: 804px; min-width: 80px"
        );
    }

    #[test]
    fn plain_options_repeat_the_value_as_the_label() {
        assert_eq!(
            plain_options(["Inter", "Lora"]),
            vec![
                ("Inter".to_owned(), "Inter".to_owned()),
                ("Lora".to_owned(), "Lora".to_owned())
            ]
        );
    }
}
