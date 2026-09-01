//! The top-level campaign type: a display ad or a set of ad variants.
//!
//! A campaign is the seventh artifact kind next to `Design`, `Deck`,
//! `Document`, `Social`, `Print`, and `Mailing`. It has a title, a
//! theme, a `size`, `ads`, and an optional `outline`. One ad is a
//! single placement; two or more are A/B variants of the same size.
//! Every ad is laid out on the fixed px canvas of its size, an IAB
//! display unit, so a campaign has no `viewport` field and no
//! transition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ad::Ad;
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Most ads a campaign run may write: a primary ad and its A/B
/// variants. More variants are written in a later run.
pub const AD_COUNT_LIMIT: u32 = 4;

/// The medium rectangle: 300 by 250 px, the most served display unit.
pub const MEDIUM_RECTANGLE_AD_VIEWPORT: Viewport = Viewport {
    width: 300,
    height: 250,
};

/// The leaderboard: 728 by 90 px, the banner above the page content.
pub const LEADERBOARD_AD_VIEWPORT: Viewport = Viewport {
    width: 728,
    height: 90,
};

/// The half page: 300 by 600 px, the tall sidebar unit.
pub const HALF_PAGE_AD_VIEWPORT: Viewport = Viewport {
    width: 300,
    height: 600,
};

/// The skyscraper: 160 by 600 px, the narrow sidebar unit.
pub const SKYSCRAPER_AD_VIEWPORT: Viewport = Viewport {
    width: 160,
    height: 600,
};

/// The mobile banner: 320 by 100 px, the phone-width strip.
pub const MOBILE_BANNER_AD_VIEWPORT: Viewport = Viewport {
    width: 320,
    height: 100,
};

/// The canvas an ad is laid out on. Every size is a standard IAB
/// display unit in px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AdSize {
    /// Medium rectangle: 300 by 250 px.
    #[default]
    MediumRectangle,
    /// Leaderboard: 728 by 90 px.
    Leaderboard,
    /// Half page: 300 by 600 px.
    HalfPage,
    /// Skyscraper: 160 by 600 px.
    Skyscraper,
    /// Mobile banner: 320 by 100 px.
    MobileBanner,
}

impl AdSize {
    /// Every size, in the order the UI shows them.
    pub const ALL: [AdSize; 5] = [
        AdSize::MediumRectangle,
        AdSize::Leaderboard,
        AdSize::HalfPage,
        AdSize::Skyscraper,
        AdSize::MobileBanner,
    ];

    /// The px canvas of one ad.
    pub fn viewport(self) -> Viewport {
        match self {
            AdSize::MediumRectangle => MEDIUM_RECTANGLE_AD_VIEWPORT,
            AdSize::Leaderboard => LEADERBOARD_AD_VIEWPORT,
            AdSize::HalfPage => HALF_PAGE_AD_VIEWPORT,
            AdSize::Skyscraper => SKYSCRAPER_AD_VIEWPORT,
            AdSize::MobileBanner => MOBILE_BANNER_AD_VIEWPORT,
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            AdSize::MediumRectangle => "medium_rectangle",
            AdSize::Leaderboard => "leaderboard",
            AdSize::HalfPage => "half_page",
            AdSize::Skyscraper => "skyscraper",
            AdSize::MobileBanner => "mobile_banner",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            AdSize::MediumRectangle => "Medium rectangle",
            AdSize::Leaderboard => "Leaderboard",
            AdSize::HalfPage => "Half page",
            AdSize::Skyscraper => "Skyscraper",
            AdSize::MobileBanner => "Mobile banner",
        }
    }

    /// The size for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<AdSize> {
        AdSize::ALL
            .into_iter()
            .find(|size| size.as_str() == value.trim())
    }
}

/// A complete campaign: one theme applied to an ordered list of ads on
/// one size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Campaign {
    /// Campaign title, shown in the editor. Not rendered on an ad.
    pub title: String,
    /// Visual identity applied to every ad.
    pub theme: Theme,
    /// The size every ad is laid out on. Absent means medium rectangle.
    #[serde(default)]
    pub size: AdSize,
    /// Ads in priority order. One ad is a single placement; two or
    /// more are A/B variants of the same size.
    pub ads: Vec<Ad>,
    /// The ad titles of the complete campaign, in order. A preview
    /// campaign lists more titles than it has ads: the app continues
    /// it from this list later. Leave it empty when the campaign is
    /// complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
}

impl Campaign {
    /// True when the outline plans more ads than the campaign has: the
    /// campaign is a preview that waits for its remaining ads.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.ads.len()
    }

    /// The px canvas of every ad.
    pub fn viewport(&self) -> Viewport {
        self.size.viewport()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_campaign;
    use crate::{
        AdSize, Campaign, HALF_PAGE_AD_VIEWPORT, LEADERBOARD_AD_VIEWPORT,
        MEDIUM_RECTANGLE_AD_VIEWPORT, MOBILE_BANNER_AD_VIEWPORT, SKYSCRAPER_AD_VIEWPORT,
    };

    #[test]
    fn campaign_round_trips_through_json() {
        let campaign = sample_campaign();
        let json = serde_json::to_string(&campaign).unwrap();
        assert!(json.contains("\"ads\""));
        assert!(json.contains("\"size\":\"medium_rectangle\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Campaign = serde_json::from_str(&json).unwrap();
        assert_eq!(campaign, restored);
    }

    #[test]
    fn rejects_unknown_campaign_fields() {
        let mut value = serde_json::to_value(sample_campaign()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Campaign>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 300, "height": 250});
        assert!(serde_json::from_value::<Campaign>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("viewport");
        value["transition"] = serde_json::json!({"effect": "fade"});
        assert!(serde_json::from_value::<Campaign>(value).is_err());
    }

    #[test]
    fn the_size_defaults_to_medium_rectangle() {
        let mut value = serde_json::to_value(sample_campaign()).unwrap();
        value.as_object_mut().unwrap().remove("size");
        let restored: Campaign = serde_json::from_value(value).unwrap();
        assert_eq!(restored.size, AdSize::MediumRectangle);
        assert_eq!(restored.viewport(), MEDIUM_RECTANGLE_AD_VIEWPORT);
    }

    #[test]
    fn an_outline_longer_than_the_ads_marks_a_preview() {
        let mut campaign = sample_campaign();
        assert!(!campaign.is_preview());
        assert!(
            !serde_json::to_string(&campaign)
                .unwrap()
                .contains("outline")
        );
        campaign.outline = vec!["Primary".to_owned(), "Variant B".to_owned()];
        assert!(campaign.is_preview());
        let json = serde_json::to_string(&campaign).unwrap();
        let restored: Campaign = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, campaign.outline);
        campaign.outline.truncate(1);
        assert!(!campaign.is_preview());
    }

    // IAB units sit below MIN_VIEWPORT_SIDE on purpose, so this test
    // asserts the exact px sizes instead of Viewport::is_valid.
    #[test]
    fn the_sizes_are_the_iab_units_and_named() {
        for size in AdSize::ALL {
            assert_eq!(AdSize::from_name(size.as_str()), Some(size));
            let json = serde_json::to_string(&size).unwrap();
            assert_eq!(json, format!("\"{}\"", size.as_str()));
        }
        assert_eq!(MEDIUM_RECTANGLE_AD_VIEWPORT.aspect_ratio_css(), "300 / 250");
        assert_eq!(LEADERBOARD_AD_VIEWPORT.aspect_ratio_css(), "728 / 90");
        assert_eq!(HALF_PAGE_AD_VIEWPORT.aspect_ratio_css(), "300 / 600");
        assert_eq!(SKYSCRAPER_AD_VIEWPORT.aspect_ratio_css(), "160 / 600");
        assert_eq!(MOBILE_BANNER_AD_VIEWPORT.aspect_ratio_css(), "320 / 100");
        assert_eq!(AdSize::from_name("banner"), None);
    }
}
