//! The top-level social type: a post or a carousel for a social feed.
//!
//! A social is the fourth artifact kind next to `Design`, `Deck`, and
//! `Document`. It has a title, a theme, a `format`, `frames`, and an
//! optional `outline`. One frame is a single post; several frames are
//! a carousel. Every frame is laid out on the fixed px canvas of its
//! format, so a social has no `viewport` field and no transition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::frame::Frame;
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Most frames a social run may write. A carousel past ten frames is
/// cut by every platform.
pub const FRAME_COUNT_LIMIT: u32 = 10;

/// The square post: 1080 by 1080 px.
pub const SQUARE_VIEWPORT: Viewport = Viewport {
    width: 1080,
    height: 1080,
};

/// The portrait post: 1080 by 1350 px, the 4:5 feed post and carousel.
pub const PORTRAIT_VIEWPORT: Viewport = Viewport {
    width: 1080,
    height: 1350,
};

/// The story: 1080 by 1920 px, the 9:16 full screen.
pub const STORY_VIEWPORT: Viewport = Viewport {
    width: 1080,
    height: 1920,
};

/// The landscape post: 1200 by 630 px, the link preview and the wide
/// feed post.
pub const LANDSCAPE_VIEWPORT: Viewport = Viewport {
    width: 1200,
    height: 630,
};

/// The canvas a social is laid out on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Square: 1080 by 1080 px.
    Square,
    /// Portrait: 1080 by 1350 px.
    #[default]
    Portrait,
    /// Story: 1080 by 1920 px.
    Story,
    /// Landscape: 1200 by 630 px.
    Landscape,
}

impl Format {
    /// Every format, in the order the UI shows them.
    pub const ALL: [Format; 4] = [
        Format::Square,
        Format::Portrait,
        Format::Story,
        Format::Landscape,
    ];

    /// The px canvas of one frame.
    pub fn viewport(self) -> Viewport {
        match self {
            Format::Square => SQUARE_VIEWPORT,
            Format::Portrait => PORTRAIT_VIEWPORT,
            Format::Story => STORY_VIEWPORT,
            Format::Landscape => LANDSCAPE_VIEWPORT,
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Square => "square",
            Format::Portrait => "portrait",
            Format::Story => "story",
            Format::Landscape => "landscape",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            Format::Square => "Square",
            Format::Portrait => "Portrait",
            Format::Story => "Story",
            Format::Landscape => "Landscape",
        }
    }

    /// The format for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<Format> {
        Format::ALL
            .into_iter()
            .find(|format| format.as_str() == value.trim())
    }
}

/// A complete social: one theme applied to an ordered list of frames
/// on one format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Social {
    /// Social title, shown in the editor. Not rendered on a frame.
    pub title: String,
    /// Visual identity applied to every frame.
    pub theme: Theme,
    /// The format every frame is laid out on. Absent means portrait.
    #[serde(default)]
    pub format: Format,
    /// Frames in swipe order. One frame is a single post; two or more
    /// are a carousel.
    pub frames: Vec<Frame>,
    /// The frame titles of the complete social, in order. A preview
    /// social lists more titles than it has frames: the app continues
    /// it from this list later. Leave it empty when the social is
    /// complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
}

impl Social {
    /// True when the outline plans more frames than the social has: the
    /// social is a preview that waits for its remaining frames.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.frames.len()
    }

    /// The px canvas of every frame.
    pub fn viewport(&self) -> Viewport {
        self.format.viewport()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_social;
    use crate::{
        Format, LANDSCAPE_VIEWPORT, PORTRAIT_VIEWPORT, SQUARE_VIEWPORT, STORY_VIEWPORT, Social,
    };

    #[test]
    fn social_round_trips_through_json() {
        let social = sample_social();
        let json = serde_json::to_string(&social).unwrap();
        assert!(json.contains("\"frames\""));
        assert!(json.contains("\"format\":\"portrait\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Social = serde_json::from_str(&json).unwrap();
        assert_eq!(social, restored);
    }

    #[test]
    fn rejects_unknown_social_fields() {
        let mut value = serde_json::to_value(sample_social()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Social>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 1080, "height": 1350});
        assert!(serde_json::from_value::<Social>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("viewport");
        value["transition"] = serde_json::json!({"effect": "fade"});
        assert!(serde_json::from_value::<Social>(value).is_err());
    }

    #[test]
    fn the_format_defaults_to_portrait() {
        let mut value = serde_json::to_value(sample_social()).unwrap();
        value.as_object_mut().unwrap().remove("format");
        let restored: Social = serde_json::from_value(value).unwrap();
        assert_eq!(restored.format, Format::Portrait);
        assert_eq!(restored.viewport(), PORTRAIT_VIEWPORT);
    }

    #[test]
    fn an_outline_longer_than_the_frames_marks_a_preview() {
        let mut social = sample_social();
        assert!(!social.is_preview());
        assert!(!serde_json::to_string(&social).unwrap().contains("outline"));
        social.outline = vec!["Sample".to_owned(), "Next".to_owned()];
        assert!(social.is_preview());
        let json = serde_json::to_string(&social).unwrap();
        let restored: Social = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, social.outline);
        social.outline.truncate(1);
        assert!(!social.is_preview());
    }

    #[test]
    fn the_formats_are_valid_and_named() {
        for format in Format::ALL {
            let viewport = format.viewport();
            assert!(viewport.is_valid(), "{}", format.label());
            assert_eq!(Format::from_name(format.as_str()), Some(format));
            let json = serde_json::to_string(&format).unwrap();
            assert_eq!(json, format!("\"{}\"", format.as_str()));
        }
        assert_eq!(SQUARE_VIEWPORT.aspect_ratio_css(), "1080 / 1080");
        assert_eq!(PORTRAIT_VIEWPORT.aspect_ratio_css(), "1080 / 1350");
        assert_eq!(STORY_VIEWPORT.aspect_ratio_css(), "1080 / 1920");
        assert_eq!(LANDSCAPE_VIEWPORT.aspect_ratio_css(), "1200 / 630");
        assert_eq!(Format::from_name("banner"), None);
    }
}
