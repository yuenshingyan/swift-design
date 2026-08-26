//! Theme types: the default colors and fonts of a design.
//!
//! Theme values are defaults. An element that omits a color or a font,
//! or a screen that omits a background, inherits the theme value.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Default colors and fonts that elements inherit when they omit their
/// own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    /// Theme name shown in the editor.
    pub name: String,
    /// Default colors.
    pub colors: Palette,
    /// Default font families.
    pub fonts: FontSet,
}

/// Default colors as `#rrggbb` hex strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    /// Screen background color when a screen sets no background.
    pub background: String,
    /// Text color when an element sets no color.
    pub text: String,
    /// Color for highlights, shapes, and chart series that set no color.
    pub accent: String,
    /// Secondary color for captions, rules, and table stripes.
    pub muted: String,
}

/// Default CSS font family names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontSet {
    /// Family for text elements with role `title` or `heading`.
    pub heading: String,
    /// Family for every other text element that sets no font.
    pub body: String,
    /// Family for code elements.
    pub mono: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Theme;
    use crate::test_support::sample_design;

    #[test]
    fn theme_round_trips_through_json() {
        let theme = sample_design().theme;
        let json = serde_json::to_string(&theme).unwrap();
        let restored: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(theme, restored);
    }
}
