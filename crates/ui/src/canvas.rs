//! The candidate canvas: the designs a generation run writes, each in a
//! live preview iframe, plus placeholder cards for ids the run reports
//! before they reach disk.

use std::collections::HashMap;

use dioxus::prelude::*;

use crate::api;
use crate::settings::stepped_screen;

/// The designs that belong to `session_id`, chosen design first.
pub(crate) fn session_designs(
    designs: &[api::DesignSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<api::DesignSummary> {
    let mut mine: Vec<api::DesignSummary> = designs
        .iter()
        .filter(|summary| crate::settings::design_project(&summary.id) == session_id)
        .cloned()
        .collect();
    mine.sort_by_key(|summary| Some(summary.id.as_str()) != chosen);
    mine
}

/// A short card label for one design.
pub(crate) fn card_label(summary: &api::DesignSummary, chosen: Option<&str>) -> String {
    if Some(summary.id.as_str()) == chosen {
        return format!("Chosen · {} screens", summary.screen_count);
    }
    let number = summary
        .id
        .rsplit("-candidate-")
        .next()
        .filter(|tail| tail.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or("");
    let name = if number.is_empty() {
        "Candidate".to_owned()
    } else {
        format!("Candidate {number}")
    };
    if summary.is_preview() {
        format!(
            "{name} · preview {} of {} screens",
            summary.screen_count, summary.outline_count
        )
    } else {
        format!("{name} · {} screens", summary.screen_count)
    }
}

/// The candidate canvas for a session.
#[component]
pub(crate) fn CandidateCanvas(
    session_id: String,
    designs: Vec<api::DesignSummary>,
    run_designs: HashMap<String, u8>,
    revision: u64,
    chosen: Option<String>,
    on_open: EventHandler<String>,
) -> Element {
    let mut shown = use_signal(HashMap::<String, usize>::new);
    // Placeholder ids the run reports that are not on disk yet.
    let placeholders: Vec<String> = run_designs
        .keys()
        .filter(|id| !designs.iter().any(|summary| &summary.id == *id))
        .cloned()
        .collect();
    rsx! {
        div { class: "canvas-grid",
            for summary in designs {
                {
                    let id = summary.id.clone();
                    let count = summary.screen_count.max(1);
                    let current = shown.read().get(&id).copied().unwrap_or(1).min(count);
                    let progress = run_designs.get(&id).copied();
                    let is_chosen = chosen.as_deref() == Some(id.as_str());
                    let ratio = summary.aspect_ratio();
                    rsx! {
                        article { key: "{id}",
                            class: if is_chosen { "canvas-card chosen" } else { "canvas-card" },
                            div { class: "card-preview",
                                iframe {
                                    src: "/designs/{id}/render?v={revision}&screen={current}",
                                    style: "aspect-ratio: {ratio}",
                                    title: "{id}",
                                }
                            }
                            div { class: "card-footer",
                                span { class: "card-label", "{card_label(&summary, chosen.as_deref())}" }
                                div { class: "card-pager",
                                    button {
                                        onclick: {
                                            let id = id.clone();
                                            move |_| { shown.write().insert(id.clone(), stepped_screen(current, -1, count)); }
                                        },
                                        "‹"
                                    }
                                    span { "{current}/{count}" }
                                    button {
                                        onclick: {
                                            let id = id.clone();
                                            move |_| { shown.write().insert(id.clone(), stepped_screen(current, 1, count)); }
                                        },
                                        "›"
                                    }
                                }
                                button {
                                    class: "open-card",
                                    onclick: {
                                        let id = id.clone();
                                        move |_| on_open.call(id.clone())
                                    },
                                    "Open"
                                }
                            }
                            if let Some(percent) = progress {
                                div { class: "progress-track",
                                    div { class: "progress-fill", style: "width: {percent}%" }
                                }
                            }
                        }
                    }
                }
            }
            for id in placeholders {
                article { key: "{id}", class: "canvas-card placeholder",
                    div { class: "card-placeholder", "Writing {id}…" }
                    if let Some(percent) = run_designs.get(&id).copied() {
                        div { class: "progress-track",
                            div { class: "progress-fill", style: "width: {percent}%" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{card_label, session_designs};
    use crate::api::DesignSummary;

    fn summary(id: &str) -> DesignSummary {
        DesignSummary {
            id: id.to_owned(),
            title: "T".to_owned(),
            theme: "slate".to_owned(),
            viewport: Default::default(),
            screen_count: 3,
            outline_count: 0,
            pending_count: 0,
        }
    }

    #[test]
    fn session_designs_filter_and_put_the_chosen_first() {
        let designs = [
            summary("talk-candidate-1"),
            summary("talk-candidate-2"),
            summary("other"),
        ];
        let mine = session_designs(&designs, "talk", Some("talk-candidate-2"));
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].id, "talk-candidate-2");
    }

    #[test]
    fn card_labels_shorten_candidate_ids() {
        assert_eq!(
            card_label(&summary("talk-candidate-2"), None),
            "Candidate 2 · 3 screens"
        );
        assert_eq!(
            card_label(&summary("talk"), Some("talk")),
            "Chosen · 3 screens"
        );
        let preview = DesignSummary {
            outline_count: 12,
            ..summary("talk-candidate-1")
        };
        assert_eq!(
            card_label(&preview, None),
            "Candidate 1 · preview 3 of 12 screens"
        );
    }
}
