//! Sheet type: one HTML fragment plus its own CSS, for a print.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One sheet: an HTML fragment for the print's size, CSS for that
/// fragment, and author notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    /// The sheet as one HTML fragment. The canvas is the print's `size`
    /// in px, rotated by its `orientation`: 559 by 794 for A5, 794 by
    /// 1123 for A4, 1123 by 1587 for A3, 816 by 1056 for Letter, 1056
    /// by 1632 for Tabloid. Use px units. Allowed: headings, text,
    /// lists, tables, `<img>` with `src="/uploads/{name}"`, inline
    /// `<svg>`, `<pre><code>`, `<blockquote>`, `<a>`. Not allowed:
    /// `<script>`, `<style>`, `<iframe>`, `<object>`, `<embed>`,
    /// `<link>`, `<meta>`, `<form>`, `<button>`, `<input>`, media,
    /// comments, `on*` attributes, `javascript:` and `data:` URLs,
    /// external images. Close every tag.
    pub html: String,
    /// CSS for this sheet only. The server scopes every selector to
    /// this sheet. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Author notes: print instructions such as paper stock or bleed,
    /// intent, or handoff remarks. Never rendered on the sheet and
    /// never exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Sheet;
    use crate::test_support::sample_print;

    #[test]
    fn sheet_round_trips_through_json() {
        let sheet = Sheet {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Stock: 300 gsm matte.".to_owned()),
        };
        let json = serde_json::to_string(&sheet).unwrap();
        let restored: Sheet = serde_json::from_str(&json).unwrap();
        assert_eq!(sheet, restored);
    }

    #[test]
    fn absent_sheet_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_print().sheets[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn a_sheet_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Front", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Sheet>(named).is_err());
    }
}
