//! Cover type: one HTML fragment plus its own CSS, for an artwork.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One cover: an HTML fragment for the artwork's size, CSS for that
/// fragment, and author notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cover {
    /// The cover as one HTML fragment. The canvas is the artwork's
    /// `size` in px: 1280 by 720 for thumbnail, 2560 by 1440 for
    /// banner, 1500 by 500 for header, 3000 by 3000 for album, 1600
    /// by 2560 for book. Use px units.
    /// Allowed: headings, text, lists, tables, `<img>` with
    /// `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`,
    /// `<form>`, `<button>`, `<input>`, media, comments, `on*`
    /// attributes, `javascript:` and `data:` URLs, external images.
    /// Close every tag.
    pub html: String,
    /// CSS for this cover only. The server scopes every selector to
    /// this cover. Use plain selectors such as `.title` or `h1`. Use
    /// the theme through `var(--background)`, `var(--text)`,
    /// `var(--accent)`, `var(--muted)`, `var(--heading-font)`,
    /// `var(--body-font)`, `var(--mono-font)`. No `@import`, no
    /// viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Author notes: the title context and the alt text, as a
    /// `Title:` line and an `Alt:` line, plus intent or handoff
    /// remarks. Never rendered on the cover and never exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Cover;
    use crate::test_support::sample_artwork;

    #[test]
    fn cover_round_trips_through_json() {
        let cover = Cover {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Title: A short hello. Alt: A red greeting.".to_owned()),
        };
        let json = serde_json::to_string(&cover).unwrap();
        let restored: Cover = serde_json::from_str(&json).unwrap();
        assert_eq!(cover, restored);
    }

    #[test]
    fn absent_cover_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_artwork().covers[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn a_cover_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Primary", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Cover>(named).is_err());
    }
}
