//! The top-level deck type: a slide presentation.
//!
//! A deck is the second artifact kind next to `Design`. It keeps the
//! Swift Deck JSON shape: a title, a theme, `slides`, an optional
//! `outline`, and an optional `transition`. Every slide is laid out on a
//! fixed 1920 by 1080 px canvas, so a deck has no `viewport` field.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::slide::Slide;
use crate::theme::Theme;
use crate::transition::Transition;
use crate::viewport::Viewport;

/// Width of the deck canvas, in px.
pub const DECK_WIDTH: u32 = 1920;

/// Height of the deck canvas, in px.
pub const DECK_HEIGHT: u32 = 1080;

/// The deck canvas as a viewport, for the shared render helpers.
pub const DECK_VIEWPORT: Viewport = Viewport {
    width: DECK_WIDTH,
    height: DECK_HEIGHT,
};

/// A complete presentation: one theme applied to an ordered list of slides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Deck {
    /// Presentation title, shown on the title slide and in the editor.
    pub title: String,
    /// Visual identity applied to every slide.
    pub theme: Theme,
    /// Slides in presentation order.
    pub slides: Vec<Slide>,
    /// The slide titles of the complete deck, in order. A preview deck
    /// lists more titles than it has slides: the app continues it from
    /// this list later. Leave it empty when the deck is complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
    /// How the presentation moves from one slide to the next. Leave it
    /// out for a deck that scrolls instead of animating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
}

impl Deck {
    /// True when the outline plans more slides than the deck has: the
    /// deck is a preview that waits for its remaining slides.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.slides.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_deck;
    use crate::{DECK_VIEWPORT, Deck, Transition, TransitionAxis, TransitionEffect};

    #[test]
    fn deck_round_trips_through_json() {
        let deck = sample_deck();
        let json = serde_json::to_string(&deck).unwrap();
        assert!(json.contains("\"slides\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Deck = serde_json::from_str(&json).unwrap();
        assert_eq!(deck, restored);
    }

    #[test]
    fn rejects_unknown_deck_fields() {
        let mut value = serde_json::to_value(sample_deck()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Deck>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 1920, "height": 1080});
        assert!(serde_json::from_value::<Deck>(value).is_err());
    }

    #[test]
    fn an_outline_longer_than_the_slides_marks_a_preview() {
        let mut deck = sample_deck();
        assert!(!deck.is_preview());
        assert!(!serde_json::to_string(&deck).unwrap().contains("outline"));
        deck.outline = vec!["Sample".to_owned(), "Next".to_owned()];
        assert!(deck.is_preview());
        let json = serde_json::to_string(&deck).unwrap();
        let restored: Deck = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, deck.outline);
        deck.outline.truncate(1);
        assert!(!deck.is_preview());
    }

    #[test]
    fn a_deck_transition_round_trips() {
        let mut deck = sample_deck();
        assert!(!serde_json::to_string(&deck).unwrap().contains("transition"));
        deck.transition = Some(Transition {
            effect: TransitionEffect::Zoom,
            axis: TransitionAxis::Horizontal,
            duration_ms: 300,
        });
        let json = serde_json::to_string(&deck).unwrap();
        let restored: Deck = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.transition, deck.transition);
    }

    #[test]
    fn the_deck_canvas_is_1920_by_1080() {
        assert_eq!(DECK_VIEWPORT.width, 1920);
        assert_eq!(DECK_VIEWPORT.height, 1080);
        assert!(DECK_VIEWPORT.is_valid());
        assert_eq!(DECK_VIEWPORT.aspect_ratio_css(), "1920 / 1080");
    }
}
