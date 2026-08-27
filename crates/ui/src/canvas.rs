//! The candidate canvas: the designs or decks a generation run writes,
//! each in a live preview iframe, plus placeholder cards for ids the run
//! reports before they reach disk.

use std::collections::HashMap;

use design_model::ArtifactKind;
use dioxus::prelude::*;

use crate::api;
use crate::settings::stepped_screen;

/// One card on the canvas: a design or a deck candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanvasCard {
    /// The design or deck id.
    pub id: String,
    /// Which store the card comes from.
    pub kind: ArtifactKind,
    /// How many screens or slides are written.
    pub count: usize,
    /// How many titles the outline plans. 0 without an outline.
    pub outline_count: usize,
    /// The CSS aspect-ratio of the preview frame.
    pub ratio: String,
}

impl CanvasCard {
    /// The word for one unit of this card: `screen` or `slide`.
    pub fn unit(&self) -> &'static str {
        match self.kind {
            ArtifactKind::Demo => "screen",
            ArtifactKind::Deck => "slide",
        }
    }

    /// True when the artifact is a preview that waits for more units.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.count
    }

    /// The render URL of unit `current` (1-based) at `revision`.
    pub fn preview_url(&self, revision: u64, current: usize) -> String {
        match self.kind {
            ArtifactKind::Demo => {
                format!("/designs/{}/render?v={revision}&screen={current}", self.id)
            }
            ArtifactKind::Deck => {
                format!("/decks/{}/render?v={revision}&slide={current}", self.id)
            }
        }
    }
}

/// The design cards that belong to `session_id`, chosen design first.
pub(crate) fn cards_from_designs(
    designs: &[api::DesignSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = designs
        .iter()
        .filter(|summary| crate::settings::artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            kind: ArtifactKind::Demo,
            count: summary.screen_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// The deck cards that belong to `session_id`, chosen deck first.
pub(crate) fn cards_from_decks(
    decks: &[api::DeckSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = decks
        .iter()
        .filter(|summary| crate::settings::artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            kind: ArtifactKind::Deck,
            count: summary.slide_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// A short card label for one candidate.
pub(crate) fn card_label(card: &CanvasCard, chosen: Option<&str>) -> String {
    let unit = card.unit();
    if Some(card.id.as_str()) == chosen {
        return format!("Chosen · {} {unit}s", card.count);
    }
    let number = card
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
    if card.is_preview() {
        format!(
            "{name} · preview {} of {} {unit}s",
            card.count, card.outline_count
        )
    } else {
        format!("{name} · {} {unit}s", card.count)
    }
}

/// The candidate canvas for a session.
#[component]
pub(crate) fn CandidateCanvas(
    session_id: String,
    cards: Vec<CanvasCard>,
    run_designs: HashMap<String, u8>,
    revision: u64,
    chosen: Option<String>,
    on_open: EventHandler<(ArtifactKind, String)>,
) -> Element {
    let mut shown = use_signal(HashMap::<String, usize>::new);
    // Placeholder ids the run reports that are not on disk yet.
    let placeholders: Vec<String> = run_designs
        .keys()
        .filter(|id| !cards.iter().any(|card| &card.id == *id))
        .cloned()
        .collect();
    rsx! {
        div { class: "canvas-grid",
            for card in cards {
                {
                    let id = card.id.clone();
                    let kind = card.kind;
                    let count = card.count.max(1);
                    let current = shown.read().get(&id).copied().unwrap_or(1).min(count);
                    let progress = run_designs.get(&id).copied();
                    let is_chosen = chosen.as_deref() == Some(id.as_str());
                    let ratio = card.ratio.clone();
                    let preview = card.preview_url(revision, current);
                    rsx! {
                        article { key: "{id}", class: if is_chosen { "canvas-card chosen" } else { "canvas-card" },
                            div { class: "card-preview",
                                iframe { src: "{preview}", style: "aspect-ratio: {ratio}", title: "{id}" }
                            }
                            div { class: "card-footer",
                                span { class: "card-label", "{card_label(&card, chosen.as_deref())}" }
                                div { class: "card-pager",
                                    button {
                                        onclick: {
                                            let id = id.clone();
                                            move |_| {
                                                shown.write().insert(id.clone(), stepped_screen(current, -1, count));
                                            }
                                        },
                                        "‹"
                                    }
                                    span { "{current}/{count}" }
                                    button {
                                        onclick: {
                                            let id = id.clone();
                                            move |_| {
                                                shown.write().insert(id.clone(), stepped_screen(current, 1, count));
                                            }
                                        },
                                        "›"
                                    }
                                }
                                button {
                                    class: "open-card",
                                    onclick: {
                                        let id = id.clone();
                                        move |_| on_open.call((kind, id.clone()))
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
                            div {
                                class: "progress-fill",
                                style: "width: {percent}%",
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use design_model::ArtifactKind;

    use super::{CanvasCard, card_label, cards_from_decks, cards_from_designs};
    use crate::api::{DeckSummary, DesignSummary};

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

    fn deck_summary(id: &str) -> DeckSummary {
        DeckSummary {
            id: id.to_owned(),
            title: "T".to_owned(),
            theme: "slate".to_owned(),
            slide_count: 5,
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
        let mine = cards_from_designs(&designs, "talk", Some("talk-candidate-2"));
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].id, "talk-candidate-2");
        assert_eq!(mine[0].kind, ArtifactKind::Demo);
        assert_eq!(
            mine[0].preview_url(7, 2),
            "/designs/talk-candidate-2/render?v=7&screen=2"
        );
    }

    #[test]
    fn card_labels_shorten_candidate_ids() {
        let cards = cards_from_designs(&[summary("talk-candidate-2")], "talk", None);
        assert_eq!(card_label(&cards[0], None), "Candidate 2 · 3 screens");
        let chosen = cards_from_designs(&[summary("talk")], "talk", Some("talk"));
        assert_eq!(card_label(&chosen[0], Some("talk")), "Chosen · 3 screens");
        let preview = CanvasCard {
            outline_count: 12,
            ..cards[0].clone()
        };
        assert_eq!(
            card_label(&preview, None),
            "Candidate 2 · preview 3 of 12 screens"
        );
    }

    #[test]
    fn deck_cards_count_slides_and_use_the_deck_render_url() {
        let decks = [deck_summary("talk-candidate-1"), deck_summary("other")];
        let cards = cards_from_decks(&decks, "talk", None);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, ArtifactKind::Deck);
        assert_eq!(cards[0].ratio, "1920 / 1080");
        assert_eq!(card_label(&cards[0], None), "Candidate 1 · 5 slides");
        assert_eq!(
            cards[0].preview_url(3, 4),
            "/decks/talk-candidate-1/render?v=3&slide=4"
        );
    }
}
