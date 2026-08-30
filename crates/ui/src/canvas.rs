//! The candidate canvas: the designs or decks a generation run writes,
//! each in a live preview iframe, plus placeholder cards for ids the run
//! reports before they reach disk.
//!
//! Every card shares one stage height. The frame width follows the
//! canvas ratio, so a desktop card is wide, a deck card wider, and a
//! phone card is a small device inside a bezel.

use std::collections::{HashMap, HashSet};

use design_model::{ArtifactKind, DECK_VIEWPORT, Viewport};
use dioxus::prelude::*;

use crate::api;
use crate::icons;
use crate::settings::stepped_screen;

/// The card stage height in rem for a tab full of cards: the floor.
const CARD_STAGE_HEIGHT_REM: f64 = 13.5;
/// The tallest stage, for a tab with one or two cards.
const STAGE_HEIGHT_CAP_REM: f64 = 30.0;
/// The row the cards of one tab share, in rem: the canvas column of a
/// laptop window. The stage height is the tallest that fits every card
/// of the tab in that row.
const ROW_WIDTH_REM: f64 = 84.0;
/// The gap between two cards in a row, in rem. Matches `.canvas-grid`.
const CARD_GAP_REM: f64 = 1.0;
/// The room the bezel padding and the stage padding take above and
/// below a phone frame, in rem.
const BEZEL_INSET_REM: f64 = 1.5;

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

    /// How many planned units are not written yet.
    pub fn remaining_count(&self) -> usize {
        self.outline_count.saturating_sub(self.count)
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

/// The card name from its id: `Candidate 2` from `talk-candidate-2`, or
/// `Candidate` when the id has no number.
pub(crate) fn candidate_label(id: &str) -> String {
    let number = id
        .rsplit("-candidate-")
        .next()
        .filter(|tail| tail.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or("");
    if number.is_empty() {
        return "Candidate".to_owned();
    }
    format!("Candidate {number}")
}

/// The name of one canvas, for a tab: `Desktop`, `Tablet`, `Phone`, or
/// `Deck`.
pub(crate) fn canvas_name(viewport: Viewport) -> &'static str {
    match (viewport.width, viewport.height) {
        (390, 844) => "Phone",
        (1024, 768) => "Tablet",
        (1920, 1080) => "Deck",
        _ => "Desktop",
    }
}

/// The canvas size, for a tab tooltip: `1440 × 900`.
pub(crate) fn canvas_size(viewport: Viewport) -> String {
    format!("{} × {}", viewport.width, viewport.height)
}

/// True for a canvas narrower than 16:10, like a phone or a tablet. The
/// main editor preview limits such a canvas by height, not by width.
pub(crate) fn is_narrow_canvas(viewport: Viewport) -> bool {
    viewport.width * 10 < viewport.height * 16
}

/// True for a canvas taller than wide, like a phone. Only a portrait
/// canvas gets the device bezel: a tablet is narrow but not portrait.
pub(crate) fn is_portrait_canvas(viewport: Viewport) -> bool {
    viewport.width < viewport.height
}

/// The width in rem of a frame `height_rem` tall that keeps the ratio
/// of `viewport`, with two decimals: `21.60`.
pub(crate) fn frame_width_rem(viewport: Viewport, height_rem: f64) -> String {
    let width = height_rem * f64::from(viewport.width) / f64::from(viewport.height);
    format!("{width:.2}")
}

/// The stage height in rem for `count` cards of `viewport` on one tab:
/// the tallest that lets them share one row of `ROW_WIDTH_REM`, kept
/// between the floor and the cap. A portrait canvas is narrow, so it
/// stays tall for more cards than a desktop canvas does.
pub(crate) fn stage_height_rem(viewport: Viewport, count: usize) -> f64 {
    let count = count.max(1) as f64;
    let width_each = (ROW_WIDTH_REM - CARD_GAP_REM * (count - 1.0)) / count;
    let ratio = f64::from(viewport.width) / f64::from(viewport.height);
    (width_each / ratio).clamp(CARD_STAGE_HEIGHT_REM, STAGE_HEIGHT_CAP_REM)
}

/// The sizes one card sets as CSS variables, in rem: the frame width at
/// the canvas ratio, the stage height, and the bezel height a phone
/// card uses.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CardFrame {
    /// The frame width: the bezel frame for a portrait canvas, else the
    /// full stage height at the canvas ratio.
    pub(crate) width: String,
    /// The stage height.
    pub(crate) stage_height: String,
    /// The bezel height, `BEZEL_INSET_REM` under the stage.
    pub(crate) bezel_height: String,
}

impl CardFrame {
    /// The `style` attribute of the card.
    pub(crate) fn style(&self) -> String {
        format!(
            "--frame-width: {}rem; --stage-height: {}rem; --bezel-height: {}rem",
            self.width, self.stage_height, self.bezel_height
        )
    }
}

/// The frame of a card on a tab that holds `count` cards of `viewport`.
pub(crate) fn card_frame(viewport: Viewport, count: usize) -> CardFrame {
    let stage = stage_height_rem(viewport, count);
    let bezel = stage - BEZEL_INSET_REM;
    let height = match is_portrait_canvas(viewport) {
        true => bezel,
        false => stage,
    };
    CardFrame {
        width: frame_width_rem(viewport, height),
        stage_height: format!("{stage:.2}"),
        bezel_height: format!("{bezel:.2}"),
    }
}

/// How many cards were written for `viewport`, for the tab count.
pub(crate) fn cards_on_canvas(cards: &[CanvasCard], viewport: Viewport) -> usize {
    cards
        .iter()
        .filter(|card| card.viewport == viewport)
        .count()
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

/// What decides a card's class list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CardFlags {
    /// The user picked this card.
    pub is_chosen: bool,
    /// The canvas is portrait, so the frame sits in a bezel.
    pub is_phone: bool,
    /// The card stands in for an artifact the run has not saved yet.
    pub is_placeholder: bool,
    /// The user ticked this card for a bulk action.
    pub is_selected: bool,
}

/// The class list of a card: `canvas-card` plus one word per flag.
pub(crate) fn card_class(flags: CardFlags) -> String {
    let mut class = String::from("canvas-card");
    if flags.is_chosen {
        class.push_str(" chosen");
    }
    if flags.is_phone {
        class.push_str(" phone");
    }
    if flags.is_placeholder {
        class.push_str(" placeholder");
    }
    if flags.is_selected {
        class.push_str(" selected");
    }
    class
}

/// The ids whose Finish the running turn still has to serve: the
/// trailing continue requests, as the server reads them. The walk stops
/// at the first user message that is not a continue. With no run in
/// flight the list is empty, so a failed run gives the button back.
pub(crate) fn queued_finishes(messages: &[api::ChatMessage], is_running: bool) -> HashSet<String> {
    let mut ids = HashSet::new();
    if !is_running {
        return ids;
    }
    for message in messages.iter().rev() {
        if message.role != "user" {
            continue;
        }
        if !message.is_continue {
            break;
        }
        if let Some(id) = &message.design {
            ids.insert(id.clone());
        }
    }
    ids
}

/// The candidate canvas for a session.
#[component]
pub(crate) fn CandidateCanvas(
    session_id: String,
    cards: Vec<CanvasCard>,
    run_designs: HashMap<String, u8>,
    /// The ids whose Finish was pressed and not yet served.
    #[props(default)]
    queued: HashSet<String>,
    revision: u64,
    chosen: Option<String>,
    on_open: EventHandler<(ArtifactKind, String)>,
    on_continue: EventHandler<String>,
    on_error: EventHandler<String>,
) -> Element {
    let mut shown = use_signal(HashMap::<String, usize>::new);
    let mut open_tab = use_signal(usize::default);
    // The ticked cards, and whether the next Delete is the confirming
    // one. A delete cannot be undone, so it takes two clicks.
    let mut selected = use_signal(HashSet::<String>::new);
    let mut is_confirming_delete = use_signal(|| false);
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
    let tab_viewport = tabs.get(tab).copied().unwrap_or_default();
    let shown_cards: Vec<CanvasCard> = cards
        .iter()
        .filter(|card| tabs.len() <= 1 || card.viewport == tab_viewport)
        .cloned()
        .collect();
    // Every card of the tab shares one height, picked so they fit one
    // row: the placeholders count too, since they take the same shape.
    let on_tab = shown_cards.len() + placeholders.len();
    let kinds: HashMap<String, ArtifactKind> = cards
        .iter()
        .map(|card| (card.id.clone(), card.kind))
        .collect();
    let selected_count = selected().len();
    let delete_selected = use_callback(move |_: ()| {
        let ids: Vec<String> = selected().iter().cloned().collect();
        let kinds = kinds.clone();
        spawn(async move {
            for id in ids {
                let deleted = match kinds.get(&id) {
                    Some(ArtifactKind::Deck) => api::delete_deck(&id).await,
                    _ => api::delete_design(&id).await,
                };
                if let Err(message) = deleted {
                    on_error.call(message);
                }
            }
            selected.write().clear();
            is_confirming_delete.set(false);
        });
    });
    rsx! {
        // The bar is always here. Showing it only with a selection moved
        // the whole canvas down on the first tick.
        div { class: "selection-bar",
            span { class: "selection-count",
                if selected_count > 0 {
                    "{selected_count} selected"
                } else {
                    "Tick a card to select it"
                }
            }
            button {
                class: if is_confirming_delete() { "selection-delete confirm" } else { "selection-delete" },
                disabled: selected_count == 0,
                onclick: move |_| {
                    if is_confirming_delete() {
                        delete_selected.call(());
                    } else {
                        is_confirming_delete.set(true);
                    }
                },
                if is_confirming_delete() {
                    "Delete {selected_count}? This cannot be undone"
                } else if selected_count > 0 {
                    "Delete {selected_count}"
                } else {
                    "Delete"
                }
            }
            button {
                class: "selection-clear",
                disabled: selected_count == 0,
                onclick: move |_| {
                    selected.write().clear();
                    is_confirming_delete.set(false);
                },
                "Clear"
            }
        }
        if !tabs.is_empty() {
            div { class: "canvas-tabs", role: "tablist",
                for (index, viewport) in tabs.iter().enumerate() {
                    button {
                        key: "{index}",
                        role: "tab",
                        class: if index == tab { "canvas-tab open" } else { "canvas-tab" },
                        title: "{canvas_size(*viewport)}",
                        onclick: move |_| open_tab.set(index),
                        span { class: "tab-name", "{canvas_name(*viewport)}" }
                        span { class: "tab-count", "{cards_on_canvas(&cards, *viewport)}" }
                    }
                }
            }
        }
        div { class: "canvas-grid",
            for card in shown_cards {
                {
                    let id = card.id.clone();
                    let current = shown.read().get(&id).copied().unwrap_or(1);
                    let progress = run_designs.get(&id).copied();
                    let is_queued = queued.contains(&id);
                    let is_chosen = chosen.as_deref() == Some(id.as_str());
                    let is_selected = selected().contains(&id);
                    rsx! {
                        CandidateCard {
                            key: "{id}",
                            card,
                            on_tab,
                            current,
                            progress,
                            is_queued,
                            is_chosen,
                            is_selected,
                            revision,
                            on_open,
                            on_continue,
                            on_select: move |id: String| {
                                let mut picks = selected.write();
                                if !picks.remove(&id) {
                                    picks.insert(id);
                                }
                                is_confirming_delete.set(false);
                            },
                            on_page: move |(id, next): (String, usize)| {
                                shown.write().insert(id, next);
                            },
                        }
                    }
                }
            }
            for id in placeholders {
                PlaceholderCard {
                    key: "{id}",
                    percent: run_designs.get(&id).copied(),
                    viewport: tab_viewport,
                    on_tab,
                    id,
                }
            }
        }
    }
}

/// The screen step for an arrow key on a focused card.
pub(crate) fn key_step(key: &Key) -> Option<i32> {
    match key {
        Key::ArrowLeft => Some(-1),
        Key::ArrowRight => Some(1),
        _ => None,
    }
}

/// One candidate card: the live preview on a stage, the overlays, and
/// the footer. A click on the card opens the artifact.
#[component]
fn CandidateCard(
    card: CanvasCard,
    /// How many cards the tab holds, placeholders included. Sets the
    /// shared height.
    on_tab: usize,
    current: usize,
    progress: Option<u8>,
    /// True while the run has this card's Finish in its queue but has
    /// not reported progress on it yet.
    #[props(default)]
    is_queued: bool,
    is_chosen: bool,
    is_selected: bool,
    revision: u64,
    on_open: EventHandler<(ArtifactKind, String)>,
    on_continue: EventHandler<String>,
    on_select: EventHandler<String>,
    on_page: EventHandler<(String, usize)>,
) -> Element {
    let id = card.id.clone();
    let kind = card.kind;
    let count = card.count.max(1);
    let current = current.clamp(1, count);
    let is_phone = is_portrait_canvas(card.viewport);
    let frame = card_frame(card.viewport, on_tab);
    let class = card_class(CardFlags {
        is_chosen,
        is_phone,
        is_placeholder: false,
        is_selected,
    });
    let ratio = card.ratio.clone();
    let preview = card.preview_url(revision, current);
    let remaining = card.remaining_count();
    // A preview waits for the rest of its outline. The button asks the
    // app for it, and the run then shows its progress on this card.
    let is_finish_offered = card.is_preview() && progress.is_none();
    let is_finishing = is_finish_offered && is_queued;
    let has_pages = count > 1;
    // The edge arrows and the arrow keys step through the screens. The
    // arrows sit inside the card, and the card opens on click, so they
    // keep their clicks to themselves.
    let step = {
        let id = id.clone();
        use_callback(move |delta: i32| {
            on_page.call((id.clone(), stepped_screen(current, delta, count)));
        })
    };
    let open = {
        let id = id.clone();
        move |_| on_open.call((kind, id.clone()))
    };
    let arrow = move |delta: i32| {
        move |event: MouseEvent| {
            event.stop_propagation();
            step(delta);
        }
    };
    rsx! {
        article {
            class: "{class}",
            style: "{frame.style()}",
            tabindex: "0",
            onclick: open,
            onkeydown: {
                let id = id.clone();
                move |event: KeyboardEvent| {
                    if let Some(delta) = key_step(&event.key()) {
                        event.prevent_default();
                        step(delta);
                    } else if event.key() == Key::Enter {
                        on_open.call((kind, id.clone()));
                    }
                }
            },
            div { class: "card-stage",
                // A phone sits in a bezel drawn by a wrapper, so the
                // iframe itself carries no rounded corners or shadows.
                if is_phone {
                    div { class: "bezel",
                        iframe { src: "{preview}", title: "{id}", tabindex: "-1" }
                    }
                } else {
                    iframe {
                        src: "{preview}",
                        style: "aspect-ratio: {ratio}",
                        title: "{id}",
                        tabindex: "-1",
                    }
                }
                button {
                    class: if is_selected { "card-select ticked" } else { "card-select" },
                    title: "Select this candidate",
                    "aria-pressed": "{is_selected}",
                    tabindex: "-1",
                    onclick: {
                        let id = id.clone();
                        move |event: MouseEvent| {
                            event.stop_propagation();
                            on_select.call(id.clone());
                        }
                    },
                    if is_selected {
                        span { dangerous_inner_html: icons::CHECK }
                    }
                }
                if has_pages {
                    button {
                        class: "card-arrow left",
                        aria_label: "Previous screen",
                        tabindex: "-1",
                        disabled: current == 1,
                        onclick: arrow(-1),
                        "‹"
                    }
                    button {
                        class: "card-arrow right",
                        aria_label: "Next screen",
                        tabindex: "-1",
                        disabled: current == count,
                        onclick: arrow(1),
                        "›"
                    }
                }
                span { class: "card-count", "{current}/{count}" }
                div { class: "card-pills",
                    if is_chosen {
                        span { class: "chosen-pill",
                            span { dangerous_inner_html: icons::CHECK }
                            "Chosen"
                        }
                    }
                    if progress.is_some() {
                        span { class: "card-pill",
                            span { class: "dot" }
                            "writing"
                        }
                    }
                    if is_finishing {
                        span { class: "card-pill",
                            span { class: "dot" }
                            "queued"
                        }
                    }
                }
                if let Some(percent) = progress {
                    div { class: "card-progress",
                        div {
                            class: "card-progress-fill",
                            style: "width: {percent}%",
                        }
                    }
                }
                // A queued Finish has no percentage yet: the bar slides
                // until the run reports one.
                if is_finishing {
                    div { class: "card-progress indeterminate",
                        div { class: "card-progress-fill" }
                    }
                }
            }
            div { class: "card-footer",
                div { class: "card-name",
                    span { class: "card-label", "{candidate_label(&id)}" }
                }
                if is_finishing {
                    button {
                        class: "card-finish finishing",
                        disabled: true,
                        title: "The run will write the remaining screens",
                        span { class: "finish-spinner" }
                        span { class: "finish-text", "Finishing…" }
                    }
                } else if is_finish_offered {
                    button {
                        class: "card-finish",
                        title: "Write the remaining screens from the outline",
                        onclick: {
                            let id = id.clone();
                            move |event: MouseEvent| {
                                event.stop_propagation();
                                on_continue.call(id.clone());
                            }
                        },
                        span { dangerous_inner_html: icons::PLAY }
                        span { class: "finish-text", "Finish {remaining}" }
                    }
                }
            }
        }
    }
}

/// A card for an artifact the run has reported but not saved yet. It
/// takes the shape of the open tab's canvas.
#[component]
fn PlaceholderCard(id: String, viewport: Viewport, on_tab: usize, percent: Option<u8>) -> Element {
    let is_phone = is_portrait_canvas(viewport);
    let class = card_class(CardFlags {
        is_chosen: false,
        is_phone,
        is_placeholder: true,
        is_selected: false,
    });
    let frame = card_frame(viewport, on_tab);
    rsx! {
        article { class: "{class}", style: "{frame.style()}",
            div { class: "card-stage",
                if is_phone {
                    div { class: "bezel",
                        div { class: "card-blank" }
                    }
                } else {
                    div { class: "card-blank" }
                }
                span { class: "card-pill",
                    span { class: "dot" }
                    "writing"
                }
                if let Some(percent) = percent {
                    div { class: "card-progress",
                        div {
                            class: "card-progress-fill",
                            style: "width: {percent}%",
                        }
                    }
                }
            }
            div { class: "card-footer",
                div { class: "card-name",
                    span { class: "card-label", "{candidate_label(&id)}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, design: Option<&str>, is_continue: bool) -> api::ChatMessage {
        api::ChatMessage {
            role: role.to_owned(),
            content: String::new(),
            design: design.map(str::to_owned),
            question_set: None,
            is_continue,
            at: None,
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn queued_finishes_are_the_trailing_continues_of_a_running_turn() {
        let messages = vec![
            message("user", Some("a"), true),
            message("assistant", None, false),
            message("user", None, false),
            message("user", Some("b"), true),
            message("user", Some("c"), true),
        ];
        let queued = queued_finishes(&messages, true);
        assert!(queued.contains("b") && queued.contains("c"));
        assert!(!queued.contains("a"));
        assert!(queued_finishes(&messages, false).is_empty());
    }

    #[test]
    fn arrow_keys_step_and_other_keys_do_not() {
        assert_eq!(key_step(&Key::ArrowLeft), Some(-1));
        assert_eq!(key_step(&Key::ArrowRight), Some(1));
        assert_eq!(key_step(&Key::Enter), None);
        assert_eq!(key_step(&Key::ArrowUp), None);
    }
    use crate::api::{DeckSummary, DesignSummary};

    fn phone() -> Viewport {
        Viewport {
            width: 390,
            height: 844,
        }
    }

    fn tablet() -> Viewport {
        Viewport {
            width: 1024,
            height: 768,
        }
    }

    fn card(id: &str, viewport: Viewport) -> CanvasCard {
        CanvasCard {
            id: id.to_owned(),
            kind: ArtifactKind::Demo,
            count: 3,
            outline_count: 0,
            ratio: viewport.aspect_ratio_css(),
            viewport,
        }
    }

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
    fn the_tabs_are_the_canvases_in_first_seen_order() {
        let cards = vec![
            card("a-candidate-1", Viewport::default()),
            card("a-candidate-2", Viewport::default()),
            card("a-candidate-3", phone()),
        ];
        assert_eq!(canvas_tabs(&cards), vec![Viewport::default(), phone()]);
        assert_eq!(canvas_tabs(&[]), Vec::new());
        assert_eq!(cards_on_canvas(&cards, Viewport::default()), 2);
        assert_eq!(cards_on_canvas(&cards, phone()), 1);
        assert_eq!(cards_on_canvas(&cards, tablet()), 0);
    }

    #[test]
    fn canvas_names_and_sizes_label_the_tabs() {
        assert_eq!(canvas_name(Viewport::default()), "Desktop");
        assert_eq!(canvas_name(phone()), "Phone");
        assert_eq!(canvas_name(tablet()), "Tablet");
        assert_eq!(canvas_name(DECK_VIEWPORT), "Deck");
        assert_eq!(canvas_size(Viewport::default()), "1440 × 900");
        assert_eq!(canvas_size(DECK_VIEWPORT), "1920 × 1080");
    }

    #[test]
    fn phones_and_tablets_are_narrow_but_only_phones_are_portrait() {
        assert!(is_narrow_canvas(phone()));
        assert!(is_narrow_canvas(tablet()));
        assert!(!is_narrow_canvas(Viewport::default()));
        assert!(!is_narrow_canvas(DECK_VIEWPORT));
        assert!(is_portrait_canvas(phone()));
        assert!(!is_portrait_canvas(tablet()));
        assert!(!is_portrait_canvas(Viewport::default()));
        assert!(!is_portrait_canvas(DECK_VIEWPORT));
    }

    #[test]
    fn frame_widths_follow_the_canvas_ratio() {
        assert_eq!(frame_width_rem(Viewport::default(), 13.5), "21.60");
        assert_eq!(frame_width_rem(tablet(), 13.5), "18.00");
        assert_eq!(frame_width_rem(DECK_VIEWPORT, 13.5), "24.00");
        assert_eq!(frame_width_rem(phone(), 12.0), "5.55");
        assert_eq!(frame_width_rem(tablet(), 5.5), "7.33");
        assert_eq!(frame_width_rem(phone(), 5.5), "2.54");
    }

    #[test]
    fn the_stage_height_fits_the_cards_of_the_tab_in_one_row() {
        let desktop = Viewport::default();
        assert!((stage_height_rem(desktop, 1) - 30.0).abs() < 0.01);
        assert!((stage_height_rem(desktop, 2) - 25.94).abs() < 0.01);
        assert!((stage_height_rem(desktop, 3) - 17.08).abs() < 0.01);
        assert!((stage_height_rem(desktop, 4) - 13.5).abs() < 0.01);
        assert!((stage_height_rem(desktop, 0) - 30.0).abs() < 0.01);
        // A phone is narrow, so six still share a tall row.
        assert!((stage_height_rem(phone(), 6) - 28.5).abs() < 0.01);
        assert!((stage_height_rem(tablet(), 3) - 20.5).abs() < 0.01);
    }

    #[test]
    fn a_card_frame_sets_the_width_and_both_heights() {
        let frame = card_frame(Viewport::default(), 3);
        assert_eq!(frame.stage_height, "17.08");
        assert_eq!(frame.width, "27.33");
        assert_eq!(
            frame.style(),
            "--frame-width: 27.33rem; --stage-height: 17.08rem; --bezel-height: 15.58rem"
        );
        // A phone frame follows the bezel, which sits inside the stage.
        let phone_frame = card_frame(phone(), 2);
        assert_eq!(phone_frame.stage_height, "30.00");
        assert_eq!(phone_frame.bezel_height, "28.50");
        assert_eq!(phone_frame.width, "13.17");
    }

    #[test]
    fn candidate_labels_shorten_candidate_ids() {
        assert_eq!(candidate_label("talk-candidate-2"), "Candidate 2");
        assert_eq!(candidate_label("talk-candidate-12"), "Candidate 12");
        assert_eq!(candidate_label("talk"), "Candidate");
    }

    #[test]
    fn card_classes_carry_the_flags() {
        assert_eq!(card_class(CardFlags::default()), "canvas-card");
        let flags = CardFlags {
            is_chosen: true,
            is_phone: true,
            is_placeholder: true,
            is_selected: true,
        };
        assert_eq!(
            card_class(flags),
            "canvas-card chosen phone placeholder selected"
        );
    }

    #[test]
    fn a_preview_counts_its_remaining_units() {
        let mut preview = card("a-candidate-1", Viewport::default());
        preview.count = 6;
        preview.outline_count = 10;
        assert!(preview.is_preview());
        assert_eq!(preview.remaining_count(), 4);
        let complete = card("a-candidate-2", Viewport::default());
        assert!(!complete.is_preview());
        assert_eq!(complete.remaining_count(), 0);
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
    fn deck_cards_count_slides_and_use_the_deck_render_url() {
        let decks = [deck_summary("talk-candidate-1"), deck_summary("other")];
        let cards = cards_from_decks(&decks, "talk", None);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].kind, ArtifactKind::Deck);
        assert_eq!(cards[0].viewport, DECK_VIEWPORT);
        assert_eq!(cards[0].ratio, "1920 / 1080");
        assert_eq!(
            cards[0].preview_url(3, 4),
            "/decks/talk-candidate-1/render?v=3&slide=4"
        );
    }
}
