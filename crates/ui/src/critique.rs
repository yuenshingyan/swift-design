//! The critique bar: the user picks a category and asks for a focused
//! revision of the chosen design.

use design_model::{Critique, CritiqueCategory};
use dioxus::prelude::*;

use crate::api;

/// The five critique categories, in the order the UI shows them.
pub(crate) fn critique_categories() -> [CritiqueCategory; 5] {
    CritiqueCategory::ALL
}

/// True when a revision request has usable text.
pub(crate) fn can_request_revision(text: &str) -> bool {
    !text.trim().is_empty()
}

/// The placeholder hint for one category.
pub(crate) fn critique_placeholder(category: CritiqueCategory) -> &'static str {
    match category {
        CritiqueCategory::VisualDirection => "Make the palette warmer; try a bolder headline.",
        CritiqueCategory::Structure => "Move pricing above the FAQ; tighten the hero.",
        CritiqueCategory::Accessibility => "Raise the contrast on the muted text.",
        CritiqueCategory::Content => "Shorten the intro; add a testimonial.",
        CritiqueCategory::FreeForm => "What should change?",
    }
}

/// The critique bar for the chosen design of one session.
#[component]
pub(crate) fn CritiqueBar(
    session_id: String,
    design: Option<String>,
    on_error: EventHandler<String>,
) -> Element {
    let mut category = use_signal(|| CritiqueCategory::FreeForm);
    let mut text = use_signal(String::new);
    let ready = can_request_revision(&text.read());
    let request = {
        let session_id = session_id.clone();
        let design = design.clone();
        move |_| {
            let session_id = session_id.clone();
            let design = design.clone();
            let critique = Critique {
                category: category(),
                text: text.read().clone(),
            };
            let on_error = on_error;
            spawn(async move {
                match api::send_critique(&session_id, &critique, design.as_deref()).await {
                    Ok(()) => text.set(String::new()),
                    Err(error) => on_error.call(error),
                }
            });
        }
    };
    rsx! {
        div { class: "critique-bar",
            div { class: "critique-chips",
                for option in critique_categories() {
                    button {
                        key: "{option.as_str()}",
                        class: if category() == option { "option-chip selected" } else { "option-chip" },
                        onclick: move |_| category.set(option),
                        "{option.label()}"
                    }
                }
            }
            textarea {
                class: "critique-text",
                rows: 2,
                placeholder: critique_placeholder(category()),
                value: "{text}",
                oninput: move |event: FormEvent| text.set(event.value()),
            }
            div { class: "critique-actions",
                button {
                    class: "primary",
                    disabled: !ready,
                    onclick: request,
                    "Request revision"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{can_request_revision, critique_categories};
    use design_model::CritiqueCategory;

    #[test]
    fn critique_categories_list_five_in_order() {
        let categories = critique_categories();
        assert_eq!(categories.len(), 5);
        assert_eq!(categories[0], CritiqueCategory::VisualDirection);
        assert_eq!(categories[4], CritiqueCategory::FreeForm);
        assert_eq!(categories[4].label(), "Free-form");
    }

    #[test]
    fn a_revision_request_needs_text() {
        assert!(!can_request_revision("  "));
        assert!(can_request_revision("tighten the hero"));
    }
}
