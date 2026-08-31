//! Frame type: one HTML fragment plus its own CSS, for a social.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One frame: an HTML fragment for the social's format, CSS for that
/// fragment, and author notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// The frame as one HTML fragment. The canvas is the social's
    /// `format` in px: 1080 by 1080 for square, 1080 by 1350 for
    /// portrait, 1080 by 1920 for story, 1200 by 630 for landscape.
    /// Use px units. Allowed: headings, text, lists, tables, `<img>`
    /// with `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, `<form>`,
    /// `<button>`, `<input>`, media, comments, `on*` attributes,
    /// `javascript:` and `data:` URLs, external images. Close every tag.
    pub html: String,
    /// CSS for this frame only. The server scopes every selector to
    /// this frame. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Author notes: the caption to post with the frame, intent, or
    /// handoff remarks. Never rendered on the frame and never exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Frame;
    use crate::test_support::sample_social;

    #[test]
    fn frame_round_trips_through_json() {
        let frame = Frame {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Caption: hello.".to_owned()),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let restored: Frame = serde_json::from_str(&json).unwrap();
        assert_eq!(frame, restored);
    }

    #[test]
    fn absent_frame_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_social().frames[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn a_frame_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Cover", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Frame>(named).is_err());
    }
}
