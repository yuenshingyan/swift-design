//! Screen type: one HTML fragment plus its own CSS.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One screen: an HTML fragment for a 1920 by 1080 px canvas, CSS for
/// that fragment, and presenter notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    /// The screen as one HTML fragment. The canvas is 1920 by 1080 px;
    /// use px units. Allowed: headings, text, lists, tables, `<img>`
    /// with `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, forms,
    /// media, comments, `on*` attributes, `javascript:` and `data:`
    /// URLs, external images. Close every tag.
    pub html: String,
    /// CSS for this screen only. The server scopes every selector to
    /// this screen. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Presenter notes. Never rendered on the screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Screen;
    use crate::test_support::sample_design;

    #[test]
    fn screen_round_trips_through_json() {
        let screen = Screen {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Speak slowly.".to_owned()),
        };
        let json = serde_json::to_string(&screen).unwrap();
        let restored: Screen = serde_json::from_str(&json).unwrap();
        assert_eq!(screen, restored);
    }

    #[test]
    fn absent_optional_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_design().screens[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn element_designs_are_rejected() {
        let old = serde_json::json!({ "elements": [], "notes": "x" });
        assert!(serde_json::from_value::<Screen>(old).is_err());
    }
}
