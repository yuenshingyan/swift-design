//! Page transition settings: how one screen gives way to the next.
//!
//! A design without a `transition` scrolls, which is what every design did
//! before this field existed. A design with one stacks its screens and
//! animates between them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest transition the renderer accepts, in milliseconds.
pub const MAX_TRANSITION_MS: u32 = 3000;

/// The transition duration used when the design does not set one.
pub const DEFAULT_TRANSITION_MS: u32 = 450;

/// How the design moves from one screen to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// The visual effect between two screens.
    #[serde(default)]
    pub effect: TransitionEffect,
    /// The direction the screens travel. Ignored by `none`, `fade`, and
    /// `zoom`, which do not travel.
    #[serde(default)]
    pub axis: TransitionAxis,
    /// How long one transition takes, in milliseconds. Use 0 to 3000.
    /// Ignored by `none`, which always cuts.
    #[serde(default = "default_transition_ms")]
    pub duration_ms: u32,
}

/// The default transition duration, for serde.
fn default_transition_ms() -> u32 {
    DEFAULT_TRANSITION_MS
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            effect: TransitionEffect::default(),
            axis: TransitionAxis::default(),
            duration_ms: DEFAULT_TRANSITION_MS,
        }
    }
}

/// The visual effect between two screens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransitionEffect {
    /// The next screen replaces this one at once.
    None,
    /// The two screens cross-fade.
    Fade,
    /// Both screens travel along the axis. The old screen leaves as the
    /// new one arrives.
    #[default]
    Push,
    /// The new screen travels in over the old one. The old one stays
    /// still.
    Cover,
    /// The new screen grows in as the old one shrinks away.
    Zoom,
}

impl TransitionEffect {
    /// The value used in the rendered page's `data-swift-design-effect`
    /// attribute and in its CSS selectors.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fade => "fade",
            Self::Push => "push",
            Self::Cover => "cover",
            Self::Zoom => "zoom",
        }
    }
}

/// The direction the screens travel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAxis {
    /// The next screen arrives from below.
    #[default]
    Vertical,
    /// The next screen arrives from the right.
    Horizontal,
}

impl TransitionAxis {
    /// The value used in the rendered page's `data-swift-design-axis`
    /// attribute and in its CSS selectors.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{Transition, TransitionAxis, TransitionEffect};

    #[test]
    fn transition_round_trips_through_json() {
        let transition = Transition {
            effect: TransitionEffect::Cover,
            axis: TransitionAxis::Horizontal,
            duration_ms: 600,
        };
        let json = serde_json::to_string(&transition).unwrap();
        assert_eq!(
            json,
            r#"{"effect":"cover","axis":"horizontal","duration_ms":600}"#
        );
        assert_eq!(
            serde_json::from_str::<Transition>(&json).unwrap(),
            transition
        );
    }

    #[test]
    fn missing_fields_fall_back_to_the_defaults() {
        let transition: Transition = serde_json::from_str("{}").unwrap();
        assert_eq!(transition, Transition::default());
        assert_eq!(transition.effect, TransitionEffect::Push);
        assert_eq!(transition.axis, TransitionAxis::Vertical);
        assert_eq!(transition.duration_ms, super::DEFAULT_TRANSITION_MS);
    }

    #[test]
    fn unknown_effects_and_fields_are_rejected() {
        assert!(serde_json::from_str::<Transition>(r#"{"effect":"spin"}"#).is_err());
        assert!(serde_json::from_str::<Transition>(r#"{"speed":2}"#).is_err());
    }

    #[test]
    fn effect_and_axis_names_match_the_json_values() {
        assert_eq!(TransitionEffect::Zoom.as_str(), "zoom");
        assert_eq!(TransitionEffect::None.as_str(), "none");
        assert_eq!(TransitionAxis::Horizontal.as_str(), "horizontal");
    }
}
