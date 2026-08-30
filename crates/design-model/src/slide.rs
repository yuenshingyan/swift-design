//! Slide type: one HTML fragment plus its own CSS, for a deck.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One slide: an HTML fragment for a 1920 by 1080 px canvas, CSS for
/// that fragment, and presenter notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Slide {
    /// The slide as one HTML fragment. The canvas is 1920 by 1080 px;
    /// use px units. Allowed: headings, text, lists, tables, `<img>`
    /// with `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`, `<details>`, `<label>`, `<input>` with
    /// type `checkbox` or `radio`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, `<form>`,
    /// `<button>`, other input types, media, comments, `on*`
    /// attributes, `javascript:` and `data:` URLs, external images.
    /// Close every tag.
    pub html: String,
    /// CSS for this slide only. The server scopes every selector to
    /// this slide. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Presenter notes. Never rendered on the slide. The presenter view
    /// shows them to the speaker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Slide;
    use crate::test_support::sample_deck;

    #[test]
    fn slide_round_trips_through_json() {
        let slide = Slide {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Speak slowly.".to_owned()),
        };
        let json = serde_json::to_string(&slide).unwrap();
        let restored: Slide = serde_json::from_str(&json).unwrap();
        assert_eq!(slide, restored);
    }

    #[test]
    fn absent_slide_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_deck().slides[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn a_slide_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Hero", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Slide>(named).is_err());
    }
}
