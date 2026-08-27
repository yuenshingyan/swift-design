//! The controls at the bottom of a chat box: the model chip with its
//! menu, and the Send button.
//!
//! The chip names the account that runs the next turn. Its menu lists
//! the current provider's models, holds the effort setting where a run
//! takes one, and opens the setup panel for another provider. Nothing
//! here calls a model: it only records the user's choice.

use dioxus::prelude::*;

use crate::api;
use crate::icons;
use crate::select::{Anchor, measure_trigger, menu_style, next_trigger_id};

/// The effort levels a run can use.
pub(crate) const EFFORT_LEVELS: [&str; 3] = ["low", "medium", "high"];

/// The models the chip menu lists for the current choice: the
/// provider's catalog, with the chosen model first when the catalog
/// does not list it. Empty when no model is chosen.
fn menu_models(settings: Option<&api::SettingsView>) -> Vec<String> {
    let Some(view) = settings else {
        return Vec::new();
    };
    let Some(current) = &view.current else {
        return Vec::new();
    };
    let mut models: Vec<String> = view
        .providers
        .iter()
        .find(|provider| provider.name == current.provider)
        .map(|provider| {
            provider
                .models
                .iter()
                .map(|model| model.id.clone())
                .collect()
        })
        .unwrap_or_default();
    if !models.contains(&current.model) {
        models.insert(0, current.model.clone());
    }
    models
}

/// Which page the chip menu shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuPage {
    /// Models, then the effort row and the setup row.
    Models,
    /// The three effort levels.
    Effort,
}

/// The model chip: `provider/model`, or `Choose a model`. With a model
/// chosen, a click opens its menu; without one, it opens the setup
/// panel. `effort` is the run's effort signal where the chat box
/// starts runs, so the menu can change it.
#[component]
pub(crate) fn ModelChip(
    settings: Signal<Option<api::SettingsView>>,
    is_configuring: Signal<bool>,
    #[props(default)] effort: Option<Signal<String>>,
) -> Element {
    let mut anchor = use_signal(|| Option::<Anchor>::None);
    let mut page = use_signal(|| MenuPage::Models);
    let mut error = use_signal(|| Option::<String>::None);
    let trigger_id = use_hook(next_trigger_id);
    let current = settings().and_then(|view| view.current);
    let (class, label) = match &current {
        Some(current) => (
            "model-chip mono",
            format!("{}/{}", current.provider, current.model),
        ),
        None => ("model-chip mono unset", "Choose a model".to_owned()),
    };
    let models = menu_models(settings().as_ref());
    let current_model = current.as_ref().map(|current| current.model.clone());
    let provider = current.as_ref().map(|current| current.provider.clone());
    let is_model_chosen = current.is_some();
    let measure_id = trigger_id.clone();
    let toggle = move |_| {
        if !is_model_chosen {
            is_configuring.set(!is_configuring());
            return;
        }
        if anchor().is_some() {
            anchor.set(None);
            return;
        }
        page.set(MenuPage::Models);
        let id = measure_id.clone();
        spawn(async move {
            if let Some(rect) = measure_trigger(id).await {
                anchor.set(Some(rect));
            }
        });
    };
    let choose_model = use_callback(move |model: String| {
        let Some(provider) = provider.clone() else {
            return;
        };
        anchor.set(None);
        spawn(async move {
            // Same provider, no key: the server keeps the stored
            // credentials and only the model changes.
            match api::save_settings(&provider, &model, None).await {
                Ok(()) => error.set(None),
                Err(message) => error.set(Some(message)),
            }
        });
    });
    rsx! {
        div { class: "model-chip-wrap",
            button {
                id: "{trigger_id}",
                class: "{class}",
                title: "{label}",
                "aria-haspopup": "menu",
                "aria-expanded": "{anchor().is_some()}",
                onclick: toggle,
                span { class: "model-chip-text", "{label}" }
                if is_model_chosen {
                    span {
                        class: "model-chip-chevron",
                        dangerous_inner_html: icons::CHEVRON_DOWN,
                    }
                }
            }
            if let Some(rect) = anchor() {
                div {
                    class: "menu-backdrop",
                    onclick: move |event: Event<MouseData>| {
                        event.prevent_default();
                        anchor.set(None);
                    },
                }
                div {
                    class: "popover-menu",
                    role: "menu",
                    style: "{menu_style(rect, true)}",
                    if page() == MenuPage::Effort {
                        button {
                            class: "menu-item menu-back",
                            onclick: move |_| page.set(MenuPage::Models),
                            span {
                                class: "menu-glyph",
                                dangerous_inner_html: icons::CHEVRON_LEFT,
                            }
                            span { class: "menu-title", "Effort" }
                        }
                        div { class: "menu-rule" }
                        for level in EFFORT_LEVELS {
                            button {
                                key: "{level}",
                                class: "menu-item",
                                onclick: move |_| {
                                    if let Some(mut effort) = effort {
                                        effort.set(level.to_owned());
                                    }
                                    anchor.set(None);
                                },
                                span { class: "menu-title", "{level}" }
                                if effort.is_some_and(|effort| effort() == level) {
                                    span {
                                        class: "menu-tick",
                                        dangerous_inner_html: icons::CHECK,
                                    }
                                }
                            }
                        }
                    } else {
                        for model in models {
                            button {
                                key: "{model}",
                                class: "menu-item",
                                onclick: {
                                    let model = model.clone();
                                    move |_| choose_model.call(model.clone())
                                },
                                span { class: "menu-title mono", "{model}" }
                                if current_model.as_deref() == Some(model.as_str()) {
                                    span {
                                        class: "menu-tick",
                                        dangerous_inner_html: icons::CHECK,
                                    }
                                }
                            }
                        }
                        div { class: "menu-rule" }
                        if let Some(effort) = effort {
                            button {
                                class: "menu-item menu-row",
                                onclick: move |_| page.set(MenuPage::Effort),
                                span { class: "menu-title", "Effort" }
                                span { class: "menu-value", "{effort()}" }
                                span {
                                    class: "menu-glyph",
                                    dangerous_inner_html: icons::CHEVRON_RIGHT,
                                }
                            }
                        }
                        button {
                            class: "menu-item menu-row",
                            onclick: move |_| {
                                anchor.set(None);
                                is_configuring.set(true);
                            },
                            span { class: "menu-title", "More models" }
                            span {
                                class: "menu-glyph",
                                dangerous_inner_html: icons::CHEVRON_RIGHT,
                            }
                        }
                    }
                }
            }
            if let Some(message) = error() {
                p { class: "error", "{message}" }
            }
        }
    }
}

/// The submit button at the right of a composer: a play glyph only.
/// `label` names the action for the tooltip and screen readers.
#[component]
pub(crate) fn SendButton(
    label: &'static str,
    is_enabled: bool,
    on_send: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            class: "primary send-button",
            title: "{label} (Enter)",
            "aria-label": "{label}",
            disabled: !is_enabled,
            onclick: move |_| on_send.call(()),
            span { dangerous_inner_html: icons::PLAY }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(current_model: Option<&str>) -> api::SettingsView {
        api::SettingsView {
            providers: vec![api::CatalogProvider {
                name: "openai".to_owned(),
                label: "OpenAI".to_owned(),
                models: vec![
                    api::CatalogModel {
                        id: "gpt-5".to_owned(),
                        description: "Best structure and copy.".to_owned(),
                        is_recommended: true,
                    },
                    api::CatalogModel {
                        id: "gpt-5-mini".to_owned(),
                        description: "Cheapest.".to_owned(),
                        is_recommended: false,
                    },
                ],
                needs_api_key: true,
                supports_login: true,
            }],
            current: current_model.map(|model| api::CurrentSettings {
                provider: "openai".to_owned(),
                model: model.to_owned(),
                auth: "api_key".to_owned(),
            }),
            has_chrome: false,
        }
    }

    #[test]
    fn menu_models_list_the_provider_catalog_with_the_chosen_model_first() {
        assert_eq!(
            menu_models(Some(&view(Some("gpt-5-mini")))),
            vec!["gpt-5", "gpt-5-mini"]
        );
        assert_eq!(
            menu_models(Some(&view(Some("gpt-6-custom")))),
            vec!["gpt-6-custom", "gpt-5", "gpt-5-mini"]
        );
        assert!(menu_models(Some(&view(None))).is_empty());
        assert!(menu_models(None).is_empty());
    }
}
