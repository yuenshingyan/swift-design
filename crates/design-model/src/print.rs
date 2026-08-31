//! The top-level print type: a poster, a flyer, or a similar print
//! piece.
//!
//! A print is the fifth artifact kind next to `Design`, `Deck`,
//! `Document`, and `Social`. It has a title, a theme, a `size`, an
//! `orientation`, `sheets`, and an optional `outline`. One sheet is a
//! poster; two sheets are the front and back of a flyer. Every sheet
//! is laid out on the fixed px canvas of its size, rotated by its
//! orientation, so a print has no `viewport` field and no transition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::document::{A4_VIEWPORT, LETTER_VIEWPORT};
use crate::sheet::Sheet;
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Most sheets a print run may write: the front and back of a flyer,
/// or the panels of a folded piece.
pub const SHEET_COUNT_LIMIT: u32 = 4;

/// The A5 sheet as a px canvas at 96 dpi, portrait.
pub const A5_VIEWPORT: Viewport = Viewport {
    width: 559,
    height: 794,
};

/// The A3 sheet as a px canvas at 96 dpi, portrait.
pub const A3_VIEWPORT: Viewport = Viewport {
    width: 1123,
    height: 1587,
};

/// The Tabloid sheet as a px canvas at 96 dpi, portrait.
pub const TABLOID_VIEWPORT: Viewport = Viewport {
    width: 1056,
    height: 1632,
};

/// The paper size a print is laid out on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrintSize {
    /// A5: 559 by 794 px, portrait.
    A5,
    /// A4: 794 by 1123 px, portrait.
    #[default]
    A4,
    /// A3: 1123 by 1587 px, portrait.
    A3,
    /// US Letter: 816 by 1056 px, portrait.
    Letter,
    /// Tabloid: 1056 by 1632 px, portrait.
    Tabloid,
}

impl PrintSize {
    /// Every size, in the order the UI shows them.
    pub const ALL: [PrintSize; 5] = [
        PrintSize::A5,
        PrintSize::A4,
        PrintSize::A3,
        PrintSize::Letter,
        PrintSize::Tabloid,
    ];

    /// The px canvas of one sheet, portrait. `Orientation::apply`
    /// rotates it.
    pub fn viewport(self) -> Viewport {
        match self {
            PrintSize::A5 => A5_VIEWPORT,
            PrintSize::A4 => A4_VIEWPORT,
            PrintSize::A3 => A3_VIEWPORT,
            PrintSize::Letter => LETTER_VIEWPORT,
            PrintSize::Tabloid => TABLOID_VIEWPORT,
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            PrintSize::A5 => "a5",
            PrintSize::A4 => "a4",
            PrintSize::A3 => "a3",
            PrintSize::Letter => "letter",
            PrintSize::Tabloid => "tabloid",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            PrintSize::A5 => "A5",
            PrintSize::A4 => "A4",
            PrintSize::A3 => "A3",
            PrintSize::Letter => "Letter",
            PrintSize::Tabloid => "Tabloid",
        }
    }

    /// The size for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<PrintSize> {
        PrintSize::ALL
            .into_iter()
            .find(|size| size.as_str() == value.trim())
    }
}

/// How the sheet is turned. Landscape swaps the width and the height
/// of the size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// The short edge is the width.
    #[default]
    Portrait,
    /// The long edge is the width.
    Landscape,
}

impl Orientation {
    /// Every orientation, in the order the UI shows them.
    pub const ALL: [Orientation; 2] = [Orientation::Portrait, Orientation::Landscape];

    /// `viewport` rotated for this orientation. Portrait returns it
    /// unchanged; landscape swaps the width and the height.
    pub fn apply(self, viewport: Viewport) -> Viewport {
        match self {
            Orientation::Portrait => viewport,
            Orientation::Landscape => Viewport {
                width: viewport.height,
                height: viewport.width,
            },
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            Orientation::Portrait => "portrait",
            Orientation::Landscape => "landscape",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            Orientation::Portrait => "Portrait",
            Orientation::Landscape => "Landscape",
        }
    }

    /// The orientation for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<Orientation> {
        Orientation::ALL
            .into_iter()
            .find(|orientation| orientation.as_str() == value.trim())
    }
}

/// A complete print: one theme applied to an ordered list of sheets on
/// one size and orientation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Print {
    /// Print title, shown in the editor. Not rendered on a sheet.
    pub title: String,
    /// Visual identity applied to every sheet.
    pub theme: Theme,
    /// The paper size every sheet is laid out on. Absent means A4.
    #[serde(default)]
    pub size: PrintSize,
    /// How every sheet is turned. Absent means portrait.
    #[serde(default)]
    pub orientation: Orientation,
    /// Sheets in reading order. One sheet is a poster; two sheets are
    /// the front and back of a flyer.
    pub sheets: Vec<Sheet>,
    /// The sheet titles of the complete print, in order. A preview
    /// print lists more titles than it has sheets: the app continues
    /// it from this list later. Leave it empty when the print is
    /// complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
}

impl Print {
    /// True when the outline plans more sheets than the print has: the
    /// print is a preview that waits for its remaining sheets.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.sheets.len()
    }

    /// The px canvas of every sheet: the size rotated by the
    /// orientation.
    pub fn viewport(&self) -> Viewport {
        self.orientation.apply(self.size.viewport())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_print;
    use crate::{
        A3_VIEWPORT, A4_VIEWPORT, A5_VIEWPORT, LETTER_VIEWPORT, Orientation, Print, PrintSize,
        TABLOID_VIEWPORT, Viewport,
    };

    #[test]
    fn print_round_trips_through_json() {
        let print = sample_print();
        let json = serde_json::to_string(&print).unwrap();
        assert!(json.contains("\"sheets\""));
        assert!(json.contains("\"size\":\"a4\""));
        assert!(json.contains("\"orientation\":\"portrait\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Print = serde_json::from_str(&json).unwrap();
        assert_eq!(print, restored);
    }

    #[test]
    fn rejects_unknown_print_fields() {
        let mut value = serde_json::to_value(sample_print()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Print>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 794, "height": 1123});
        assert!(serde_json::from_value::<Print>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("viewport");
        value["transition"] = serde_json::json!({"effect": "fade"});
        assert!(serde_json::from_value::<Print>(value).is_err());
    }

    #[test]
    fn the_size_and_orientation_default_to_a4_portrait() {
        let mut value = serde_json::to_value(sample_print()).unwrap();
        value.as_object_mut().unwrap().remove("size");
        value.as_object_mut().unwrap().remove("orientation");
        let restored: Print = serde_json::from_value(value).unwrap();
        assert_eq!(restored.size, PrintSize::A4);
        assert_eq!(restored.orientation, Orientation::Portrait);
        assert_eq!(restored.viewport(), A4_VIEWPORT);
    }

    #[test]
    fn landscape_swaps_the_width_and_the_height() {
        let mut print = sample_print();
        print.orientation = Orientation::Landscape;
        assert_eq!(
            print.viewport(),
            Viewport {
                width: 1123,
                height: 794,
            }
        );
        assert_eq!(
            Orientation::Portrait.apply(A3_VIEWPORT),
            A3_VIEWPORT,
            "portrait leaves the viewport unchanged"
        );
    }

    #[test]
    fn an_outline_longer_than_the_sheets_marks_a_preview() {
        let mut print = sample_print();
        assert!(!print.is_preview());
        assert!(!serde_json::to_string(&print).unwrap().contains("outline"));
        print.outline = vec!["Front".to_owned(), "Back".to_owned()];
        assert!(print.is_preview());
        let json = serde_json::to_string(&print).unwrap();
        let restored: Print = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, print.outline);
        print.outline.truncate(1);
        assert!(!print.is_preview());
    }

    #[test]
    fn the_sizes_are_valid_and_named() {
        for size in PrintSize::ALL {
            let viewport = size.viewport();
            assert!(viewport.is_valid(), "{}", size.label());
            assert!(viewport.height > viewport.width, "{}", size.label());
            assert_eq!(PrintSize::from_name(size.as_str()), Some(size));
            let json = serde_json::to_string(&size).unwrap();
            assert_eq!(json, format!("\"{}\"", size.as_str()));
        }
        assert_eq!(A5_VIEWPORT.aspect_ratio_css(), "559 / 794");
        assert_eq!(A3_VIEWPORT.aspect_ratio_css(), "1123 / 1587");
        assert_eq!(TABLOID_VIEWPORT.aspect_ratio_css(), "1056 / 1632");
        assert_eq!(PrintSize::A4.viewport(), A4_VIEWPORT);
        assert_eq!(PrintSize::Letter.viewport(), LETTER_VIEWPORT);
        assert_eq!(PrintSize::from_name("a2"), None);
    }

    #[test]
    fn the_orientations_are_named() {
        for orientation in Orientation::ALL {
            assert_eq!(
                Orientation::from_name(orientation.as_str()),
                Some(orientation)
            );
            let json = serde_json::to_string(&orientation).unwrap();
            assert_eq!(json, format!("\"{}\"", orientation.as_str()));
        }
        assert_eq!(Orientation::from_name("upside_down"), None);
    }
}
