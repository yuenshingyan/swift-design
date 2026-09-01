//! The top-level artwork type: a piece of cover art or a set of cover
//! variants.
//!
//! An artwork is the eighth artifact kind next to `Design`, `Deck`,
//! `Document`, `Social`, `Print`, `Mailing`, and `Campaign`. It has a
//! title, a theme, a `size`, `covers`, and an optional `outline`. One
//! cover is a single piece; two or more are A/B variants of the same
//! size. Every cover is laid out on the fixed px canvas of its size, a
//! standard cover-art unit, so an artwork has no `viewport` field and
//! no transition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cover::Cover;
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Most covers an artwork run may write: a primary cover and its A/B
/// variants. More variants are written in a later run.
pub const COVER_COUNT_LIMIT: u32 = 4;

/// The thumbnail: 1280 by 720 px, the video thumbnail.
pub const THUMBNAIL_COVER_VIEWPORT: Viewport = Viewport {
    width: 1280,
    height: 720,
};

/// The channel banner: 2560 by 1440 px, the channel art.
pub const BANNER_COVER_VIEWPORT: Viewport = Viewport {
    width: 2560,
    height: 1440,
};

/// The profile header: 1500 by 500 px, the wide strip above a profile.
pub const HEADER_COVER_VIEWPORT: Viewport = Viewport {
    width: 1500,
    height: 500,
};

/// The album cover: 3000 by 3000 px, the music and podcast art.
pub const ALBUM_COVER_VIEWPORT: Viewport = Viewport {
    width: 3000,
    height: 3000,
};

/// The book cover: 1600 by 2560 px, the ebook cover.
pub const BOOK_COVER_VIEWPORT: Viewport = Viewport {
    width: 1600,
    height: 2560,
};

/// The canvas a cover is laid out on. Every size is a standard
/// cover-art unit in px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CoverSize {
    /// Thumbnail: 1280 by 720 px.
    #[default]
    Thumbnail,
    /// Channel banner: 2560 by 1440 px.
    Banner,
    /// Profile header: 1500 by 500 px.
    Header,
    /// Album cover: 3000 by 3000 px.
    Album,
    /// Book cover: 1600 by 2560 px.
    Book,
}

impl CoverSize {
    /// Every size, in the order the UI shows them.
    pub const ALL: [CoverSize; 5] = [
        CoverSize::Thumbnail,
        CoverSize::Banner,
        CoverSize::Header,
        CoverSize::Album,
        CoverSize::Book,
    ];

    /// The px canvas of one cover.
    pub fn viewport(self) -> Viewport {
        match self {
            CoverSize::Thumbnail => THUMBNAIL_COVER_VIEWPORT,
            CoverSize::Banner => BANNER_COVER_VIEWPORT,
            CoverSize::Header => HEADER_COVER_VIEWPORT,
            CoverSize::Album => ALBUM_COVER_VIEWPORT,
            CoverSize::Book => BOOK_COVER_VIEWPORT,
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            CoverSize::Thumbnail => "thumbnail",
            CoverSize::Banner => "banner",
            CoverSize::Header => "header",
            CoverSize::Album => "album",
            CoverSize::Book => "book",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            CoverSize::Thumbnail => "Thumbnail",
            CoverSize::Banner => "Channel banner",
            CoverSize::Header => "Profile header",
            CoverSize::Album => "Album cover",
            CoverSize::Book => "Book cover",
        }
    }

    /// The size for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<CoverSize> {
        CoverSize::ALL
            .into_iter()
            .find(|size| size.as_str() == value.trim())
    }
}

/// A complete artwork: one theme applied to an ordered list of covers
/// on one size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Artwork {
    /// Artwork title, shown in the editor. Not rendered on a cover.
    pub title: String,
    /// Visual identity applied to every cover.
    pub theme: Theme,
    /// The size every cover is laid out on. Absent means thumbnail.
    #[serde(default)]
    pub size: CoverSize,
    /// Covers in priority order. One cover is a single piece; two or
    /// more are A/B variants of the same size.
    pub covers: Vec<Cover>,
    /// The cover titles of the complete artwork, in order. A preview
    /// artwork lists more titles than it has covers: the app continues
    /// it from this list later. Leave it empty when the artwork is
    /// complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
}

impl Artwork {
    /// True when the outline plans more covers than the artwork has:
    /// the artwork is a preview that waits for its remaining covers.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.covers.len()
    }

    /// The px canvas of every cover.
    pub fn viewport(&self) -> Viewport {
        self.size.viewport()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_artwork;
    use crate::{
        ALBUM_COVER_VIEWPORT, Artwork, BANNER_COVER_VIEWPORT, BOOK_COVER_VIEWPORT, CoverSize,
        HEADER_COVER_VIEWPORT, THUMBNAIL_COVER_VIEWPORT,
    };

    #[test]
    fn artwork_round_trips_through_json() {
        let artwork = sample_artwork();
        let json = serde_json::to_string(&artwork).unwrap();
        assert!(json.contains("\"covers\""));
        assert!(json.contains("\"size\":\"thumbnail\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Artwork = serde_json::from_str(&json).unwrap();
        assert_eq!(artwork, restored);
    }

    #[test]
    fn rejects_unknown_artwork_fields() {
        let mut value = serde_json::to_value(sample_artwork()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Artwork>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 1280, "height": 720});
        assert!(serde_json::from_value::<Artwork>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("viewport");
        value["transition"] = serde_json::json!({"effect": "fade"});
        assert!(serde_json::from_value::<Artwork>(value).is_err());
    }

    #[test]
    fn the_size_defaults_to_thumbnail() {
        let mut value = serde_json::to_value(sample_artwork()).unwrap();
        value.as_object_mut().unwrap().remove("size");
        let restored: Artwork = serde_json::from_value(value).unwrap();
        assert_eq!(restored.size, CoverSize::Thumbnail);
        assert_eq!(restored.viewport(), THUMBNAIL_COVER_VIEWPORT);
    }

    #[test]
    fn an_outline_longer_than_the_covers_marks_a_preview() {
        let mut artwork = sample_artwork();
        assert!(!artwork.is_preview());
        assert!(!serde_json::to_string(&artwork).unwrap().contains("outline"));
        artwork.outline = vec!["Primary".to_owned(), "Variant B".to_owned()];
        assert!(artwork.is_preview());
        let json = serde_json::to_string(&artwork).unwrap();
        let restored: Artwork = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, artwork.outline);
        artwork.outline.truncate(1);
        assert!(!artwork.is_preview());
    }

    // Unlike the IAB ad units, every cover size sits inside the valid
    // viewport range, so this test asserts both.
    #[test]
    fn the_sizes_are_the_cover_units_and_named() {
        for size in CoverSize::ALL {
            assert_eq!(CoverSize::from_name(size.as_str()), Some(size));
            assert!(size.viewport().is_valid(), "{}", size.as_str());
            let json = serde_json::to_string(&size).unwrap();
            assert_eq!(json, format!("\"{}\"", size.as_str()));
        }
        assert_eq!(THUMBNAIL_COVER_VIEWPORT.aspect_ratio_css(), "1280 / 720");
        assert_eq!(BANNER_COVER_VIEWPORT.aspect_ratio_css(), "2560 / 1440");
        assert_eq!(HEADER_COVER_VIEWPORT.aspect_ratio_css(), "1500 / 500");
        assert_eq!(ALBUM_COVER_VIEWPORT.aspect_ratio_css(), "3000 / 3000");
        assert_eq!(BOOK_COVER_VIEWPORT.aspect_ratio_css(), "1600 / 2560");
        assert_eq!(CoverSize::from_name("poster"), None);
    }
}
