//! The viewport: the fixed px canvas every screen of a design is laid
//! out on. The renderer scales it to fit any frame.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Width of the default (desktop) viewport, in px.
pub const DEFAULT_VIEWPORT_WIDTH: u32 = 1440;

/// Height of the default (desktop) viewport, in px.
pub const DEFAULT_VIEWPORT_HEIGHT: u32 = 900;

/// Shortest side a viewport may have, in px.
pub const MIN_VIEWPORT_SIDE: u32 = 320;

/// Longest side a viewport may have, in px.
pub const MAX_VIEWPORT_SIDE: u32 = 4096;

/// The canvas size of every screen in a design, in px. Screens use px
/// units against this size; the server scales the result to the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    /// Canvas width in px, from 320 to 4096.
    pub width: u32,
    /// Canvas height in px, from 320 to 4096.
    pub height: u32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: DEFAULT_VIEWPORT_WIDTH,
            height: DEFAULT_VIEWPORT_HEIGHT,
        }
    }
}

impl Viewport {
    /// The usual canvas for a target platform named in a brief: a phone
    /// gets 390 by 844, a tablet 1024 by 768, anything else the desktop
    /// default. The match is on words, so `iOS app` and `mobile web`
    /// both count as a phone.
    pub fn for_platform(platform: &str) -> Viewport {
        let name = platform.to_ascii_lowercase();
        let words: Vec<&str> = name
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        let has_word = |candidates: &[&str]| words.iter().any(|word| candidates.contains(word));
        if has_word(&["tablet", "ipad"]) {
            return Viewport {
                width: 1024,
                height: 768,
            };
        }
        if has_word(&[
            "mobile",
            "phone",
            "smartphone",
            "ios",
            "iphone",
            "android",
            "handset",
        ]) {
            return Viewport {
                width: 390,
                height: 844,
            };
        }
        Viewport::default()
    }

    /// True when both sides are inside the allowed range.
    pub fn is_valid(self) -> bool {
        let range = MIN_VIEWPORT_SIDE..=MAX_VIEWPORT_SIDE;
        range.contains(&self.width) && range.contains(&self.height)
    }

    /// The CSS `aspect-ratio` value for this viewport, like `1440 / 900`.
    pub fn aspect_ratio_css(self) -> String {
        format!("{} / {}", self.width, self.height)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{DEFAULT_VIEWPORT_HEIGHT, DEFAULT_VIEWPORT_WIDTH, Viewport};

    #[test]
    fn viewport_defaults_to_1440_by_900() {
        let viewport = Viewport::default();
        assert_eq!(viewport.width, DEFAULT_VIEWPORT_WIDTH);
        assert_eq!(viewport.height, DEFAULT_VIEWPORT_HEIGHT);
        assert_eq!(viewport.aspect_ratio_css(), "1440 / 900");
    }

    #[test]
    fn viewport_for_platform_maps_phone_tablet_and_desktop() {
        assert_eq!(Viewport::for_platform("iOS app").width, 390);
        assert_eq!(Viewport::for_platform("Mobile web").height, 844);
        assert_eq!(Viewport::for_platform("iPad kiosk").width, 1024);
        assert_eq!(
            Viewport::for_platform("Web, desktop first"),
            Viewport::default()
        );
        assert_eq!(Viewport::for_platform(""), Viewport::default());
    }

    #[test]
    fn viewports_outside_the_limits_are_invalid() {
        assert!(Viewport::default().is_valid());
        assert!(
            !Viewport {
                width: 100,
                height: 900
            }
            .is_valid()
        );
        assert!(
            !Viewport {
                width: 1440,
                height: 5000
            }
            .is_valid()
        );
    }

    #[test]
    fn viewport_round_trips_through_json() {
        let viewport = Viewport {
            width: 390,
            height: 844,
        };
        let json = serde_json::to_string(&viewport).unwrap();
        assert_eq!(json, r#"{"width":390,"height":844}"#);
        assert_eq!(serde_json::from_str::<Viewport>(&json).unwrap(), viewport);
        assert!(serde_json::from_str::<Viewport>(r#"{"width":1,"height":1,"depth":1}"#).is_err());
    }
}
