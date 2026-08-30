//! Page type: one HTML fragment plus its own CSS, for a document.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One page: an HTML fragment for the document's paper, CSS for that
/// fragment, and author notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// The page as one HTML fragment. The canvas is the document's
    /// `paper` in px: 794 by 1123 for A4, 816 by 1056 for Letter. Use
    /// px units. Allowed: headings, text, lists, tables, `<img>` with
    /// `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, `<form>`,
    /// `<button>`, `<input>`, media, comments, `on*` attributes,
    /// `javascript:` and `data:` URLs, external images. Close every tag.
    pub html: String,
    /// CSS for this page only. The server scopes every selector to
    /// this page. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Author notes: intent, sources, or handoff remarks. Never rendered
    /// on the page and never exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Page;
    use crate::test_support::sample_document;

    #[test]
    fn page_round_trips_through_json() {
        let page = Page {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Cite the survey.".to_owned()),
        };
        let json = serde_json::to_string(&page).unwrap();
        let restored: Page = serde_json::from_str(&json).unwrap();
        assert_eq!(page, restored);
    }

    #[test]
    fn absent_page_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_document().pages[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn a_page_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Cover", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Page>(named).is_err());
    }
}
