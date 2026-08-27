//! The candidate canvas: the designs or decks a generation run writes,
//! each in a live preview iframe, plus placeholder cards for ids the run
//! reports before they reach disk.

use std::collections::HashMap;

use design_model::{ArtifactKind, DECK_VIEWPORT, Viewport};
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
    /// The canvas the artifact was written for. A demo run writes one
    /// design per canvas, and the tabs group by this.
    pub viewport: Viewport,
}

impl CanvasCard {
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
            viewport: summary.viewport,
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
            viewport: DECK_VIEWPORT,
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}

/// A short card label for one candidate. The pager next to it counts
/// the screens or slides, and the `Finish` button marks a preview, so
/// the label only names the card.
pub(crate) fn card_label(card: &CanvasCard, chosen: Option<&str>) -> String {
    if Some(card.id.as_str()) == chosen {
        return "Chosen".to_owned();
    }
    let number = card
        .id
        .rsplit("-candidate-")
        .next()
        .filter(|tail| tail.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or("");
    if number.is_empty() {
        return "Candidate".to_owned();
    }
    format!("Candidate {number}")
}

/// The name of one canvas, for a tab: `Desktop · 1440 × 900`.
pub(crate) fn canvas_label(viewport: Viewport) -> String {
    let name = match (viewport.width, viewport.height) {
        (390, 844) => "Phone",
        (1024, 768) => "Tablet",
        (1920, 1080) => "Slides",
        _ => "Desktop",
    };
    format!("{name} · {} × {}", viewport.width, viewport.height)
}

/// True for a canvas narrower than 16:10, like a phone or a tablet. A
/// card is as wide as its grid column and keeps the canvas ratio, so
/// the grid gives a narrow canvas narrower columns.
pub(crate) fn is_narrow_canvas(viewport: Viewport) -> bool {
    viewport.width * 10 < viewport.height * 16
}

/// The canvases the cards were written for, in first-seen order.
pub(crate) fn canvas_tabs(cards: &[CanvasCard]) -> Vec<Viewport> {
    let mut tabs: Vec<Viewport> = Vec::new();
    for card in cards {
        if !tabs.contains(&card.viewport) {
            tabs.push(card.viewport);
        }
    }
    tabs
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
    on_continue: EventHandler<String>,
) -> Element {
    let mut shown = use_signal(HashMap::<String, usize>::new);
    let mut open_tab = use_signal(usize::default);
    // Placeholder ids the run reports that are not on disk yet.
    let placeholders: Vec<String> = run_designs
        .keys()
        .filter(|id| !cards.iter().any(|card| &card.id == *id))
        .cloned()
        .collect();
    // A run writes one design per canvas. Showing every canvas at once
    // would put a phone card next to a desktop card at different
    // scales, so each canvas gets a tab.
    let tabs = canvas_tabs(&cards);
    let tab = open_tab().min(tabs.len().saturating_sub(1));
    let shown_cards: Vec<CanvasCard> = match tabs.len() > 1 {
        true => cards
            .iter()
            .filter(|card| Some(&card.viewport) == tabs.get(tab))
            .cloned()
            .collect(),
        false => cards.clone(),
    };
    let is_narrow = shown_cards
        .first()
        .is_some_and(|card| is_narrow_canvas(card.viewport));
    rsx! {
        if tabs.len() > 1 {
            div { class: "canvas-tabs",
                for (index, viewport) in tabs.iter().enumerate() {
                    button {
                        key: "{index}",
                        class: if index == tab { "canvas-tab open" } else { "canvas-tab" },
                        onclick: move |_| open_tab.set(index),
                        "{canvas_label(*viewport)}"
                    }
                }
            }
        }
        div { class: if is_narrow { "canvas-grid narrow" } else { "canvas-grid" },
            for card in shown_cards {
                {
                    let id = card.id.clone();
                    let kind = card.kind;
                    let count = card.count.max(1);
                    let current = shown.read().get(&id).copied().unwrap_or(1).min(count);
                    let progress = run_designs.get(&id).copied();
                    let is_chosen = chosen.as_deref() == Some(id.as_str());
                    let ratio = card.ratio.clone();
                    let preview = card.preview_url(revision, current);
                    let open = {
                        let id = id.clone();
                        move |_| on_open.call((kind, id.clone()))
                    };
                    rsx! {
                        article {
                            key: "{id}",
                            class: if is_chosen { "canvas-card chosen" } else { "canvas-card" },
                            onclick: open,
                            div { class: "card-preview",
                                iframe { src: "{preview}", style: "aspect-ratio: {ratio}", title: "{id}" }
                            }
                            div { class: "card-footer",
                                span { class: "card-label", "{card_label(&card, chosen.as_deref())}" }
                                div { class: "card-pager",
                                    // The pager sits inside the card, and the
                                    // card opens on click, so the pager must
                                    // keep its clicks to itself.
                                    button {
                                        onclick: {
                                            let id = id.clone();
                                            move |event: MouseEvent| {
                                                event.stop_propagation();
                                                shown.write().insert(id.clone(), stepped_screen(current, -1, count));
                                            }
                                        },
                                        "‹"
                                    }
                                    span { "{current}/{count}" }
                                    button {
                                        onclick: {
                                            let id = id.clone();
                                            move |event: MouseEvent| {
                                                event.stop_propagation();
                                                shown.write().insert(id.clone(), stepped_screen(current, 1, count));
                                            }
                                        },
                                        "›"
                                    }
                                }
                                // A preview waits for the rest of its outline.
                                // The button asks the app for it, and the run
                                // shows its progress on this card.
                                if card.is_preview() && progress.is_none() {
                                    button {
                                        class: "card-continue",
                                        title: "Write the remaining screens from the outline",
                                        onclick: {
                                            let id = id.clone();
                                            move |event: MouseEvent| {
                                                event.stop_propagation();
                                                on_continue.call(id.clone());
                                            }
                                        },
                                        "Finish"
                                    }
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
    use super::*;

    #[test]
    fn the_tabs_are_the_canvases_in_first_seen_order() {
        let card = |id: &str, viewport: Viewport| CanvasCard {
            id: id.to_owned(),
            kind: ArtifactKind::Demo,
            count: 3,
            outline_count: 0,
            ratio: viewport.aspect_ratio_css(),
            viewport,
        };
        let phone = Viewport {
            width: 390,
            height: 844,
        };
        let cards = vec![
            card("a-candidate-1", Viewport::default()),
            card("a-candidate-2", Viewport::default()),
            card("a-candidate-3", phone),
        ];
        assert_eq!(canvas_tabs(&cards), vec![Viewport::default(), phone]);
        assert_eq!(canvas_tabs(&[]), Vec::new());
        assert_eq!(canvas_label(Viewport::default()), "Desktop · 1440 × 900");
        assert_eq!(canvas_label(phone), "Phone · 390 × 844");
        assert_eq!(canvas_label(DECK_VIEWPORT), "Slides · 1920 × 1080");
    }

    #[test]
    fn phones_and_tablets_are_narrow_canvases() {
        let phone = Viewport {
            width: 390,
            height: 844,
        };
        let tablet = Viewport {
            width: 1024,
            height: 768,
        };
        assert!(is_narrow_canvas(phone));
        assert!(is_narrow_canvas(tablet));
        assert!(!is_narrow_canvas(Viewport::default()));
        assert!(!is_narrow_canvas(DECK_VIEWPORT));
    }

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
        assert_eq!(card_label(&cards[0], None), "Candidate 2");
        let chosen = cards_from_designs(&[summary("talk")], "talk", Some("talk"));
        assert_eq!(card_label(&chosen[0], Some("talk")), "Chosen");
    }

    #[test]
    fn deck_cards_count_slides_and_use_the_deck_render_url() {
        let decks = [deck_summary("talk-candidate-1"), deck_summary("other")];
        let cards = cards_from_decks(&decks, "talk", None);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, ArtifactKind::Deck);
        assert_eq!(cards[0].ratio, "1920 / 1080");
        assert_eq!(card_label(&cards[0], None), "Candidate 1");
        assert_eq!(
            cards[0].preview_url(3, 4),
            "/decks/talk-candidate-1/render?v=3&slide=4"
        );
    }
}
