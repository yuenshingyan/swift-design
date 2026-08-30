//! The top-level document type: a paged document.
//!
//! A document is the third artifact kind next to `Design` and `Deck`.
//! It has a title, a theme, `paper`, `pages`, and an optional
//! `outline`. Every page is laid out on the fixed px canvas of its
//! paper, so a document has no `viewport` field and no transition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::page::Page;
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Most pages a document run may write.
pub const PAGE_COUNT_LIMIT: u32 = 40;

/// The A4 page as a px canvas at 96 dpi.
pub const A4_VIEWPORT: Viewport = Viewport {
    width: 794,
    height: 1123,
};

/// The US Letter page as a px canvas at 96 dpi.
pub const LETTER_VIEWPORT: Viewport = Viewport {
    width: 816,
    height: 1056,
};

/// The paper a document is laid out on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Paper {
    /// A4: 794 by 1123 px.
    #[default]
    A4,
    /// US Letter: 816 by 1056 px.
    Letter,
}

impl Paper {
    /// Every paper, in the order the UI shows them.
    pub const ALL: [Paper; 2] = [Paper::A4, Paper::Letter];

    /// The px canvas of one page.
    pub fn viewport(self) -> Viewport {
        match self {
            Paper::A4 => A4_VIEWPORT,
            Paper::Letter => LETTER_VIEWPORT,
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            Paper::A4 => "a4",
            Paper::Letter => "letter",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::Letter => "Letter",
        }
    }

    /// The paper for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<Paper> {
        Paper::ALL
            .into_iter()
            .find(|paper| paper.as_str() == value.trim())
    }
}

/// A complete document: one theme applied to an ordered list of pages
/// on one paper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Document title, shown on the first page and in the editor.
    pub title: String,
    /// Visual identity applied to every page.
    pub theme: Theme,
    /// The paper every page is laid out on. Absent means A4.
    #[serde(default)]
    pub paper: Paper,
    /// Pages in reading order.
    pub pages: Vec<Page>,
    /// The page titles of the complete document, in order. A preview
    /// document lists more titles than it has pages: the app continues
    /// it from this list later. Leave it empty when the document is
    /// complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
}

impl Document {
    /// True when the outline plans more pages than the document has: the
    /// document is a preview that waits for its remaining pages.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.pages.len()
    }

    /// The px canvas of every page.
    pub fn viewport(&self) -> Viewport {
        self.paper.viewport()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_document;
    use crate::{A4_VIEWPORT, Document, LETTER_VIEWPORT, Paper};

    #[test]
    fn document_round_trips_through_json() {
        let document = sample_document();
        let json = serde_json::to_string(&document).unwrap();
        assert!(json.contains("\"pages\""));
        assert!(json.contains("\"paper\":\"a4\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(document, restored);
    }

    #[test]
    fn rejects_unknown_document_fields() {
        let mut value = serde_json::to_value(sample_document()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Document>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 794, "height": 1123});
        assert!(serde_json::from_value::<Document>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("viewport");
        value["transition"] = serde_json::json!({"effect": "fade"});
        assert!(serde_json::from_value::<Document>(value).is_err());
    }

    #[test]
    fn the_paper_defaults_to_a4() {
        let mut value = serde_json::to_value(sample_document()).unwrap();
        value.as_object_mut().unwrap().remove("paper");
        let restored: Document = serde_json::from_value(value).unwrap();
        assert_eq!(restored.paper, Paper::A4);
        assert_eq!(restored.viewport(), A4_VIEWPORT);
    }

    #[test]
    fn an_outline_longer_than_the_pages_marks_a_preview() {
        let mut document = sample_document();
        assert!(!document.is_preview());
        assert!(
            !serde_json::to_string(&document)
                .unwrap()
                .contains("outline")
        );
        document.outline = vec!["Sample".to_owned(), "Next".to_owned()];
        assert!(document.is_preview());
        let json = serde_json::to_string(&document).unwrap();
        let restored: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, document.outline);
        document.outline.truncate(1);
        assert!(!document.is_preview());
    }

    #[test]
    fn the_papers_are_portrait_and_valid() {
        for paper in Paper::ALL {
            let viewport = paper.viewport();
            assert!(viewport.is_valid());
            assert!(viewport.width < viewport.height, "{}", paper.label());
            assert_eq!(Paper::from_name(paper.as_str()), Some(paper));
            let json = serde_json::to_string(&paper).unwrap();
            assert_eq!(json, format!("\"{}\"", paper.as_str()));
        }
        assert_eq!(A4_VIEWPORT.aspect_ratio_css(), "794 / 1123");
        assert_eq!(LETTER_VIEWPORT.aspect_ratio_css(), "816 / 1056");
        assert_eq!(Paper::from_name("tabloid"), None);
    }
}
