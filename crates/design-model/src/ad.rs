//! Ad type: one HTML fragment plus its own CSS, for a campaign.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One ad: an HTML fragment for the campaign's size, CSS for that
/// fragment, and author notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ad {
    /// The ad as one HTML fragment. The canvas is the campaign's
    /// `size` in px: 300 by 250 for medium rectangle, 728 by 90 for
    /// leaderboard, 300 by 600 for half page, 160 by 600 for
    /// skyscraper, 320 by 100 for mobile banner. Use px units.
    /// Allowed: headings, text, lists, tables, `<img>` with
    /// `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`,
    /// `<form>`, `<button>`, `<input>`, media, comments, `on*`
    /// attributes, `javascript:` and `data:` URLs, external images.
    /// Close every tag.
    pub html: String,
    /// CSS for this ad only. The server scopes every selector to this
    /// ad. Use plain selectors such as `.title` or `h1`. Use the theme
    /// through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Author notes: the click-through URL and the alt text, as a
    /// `Link:` line and an `Alt:` line, plus intent or handoff
    /// remarks. Never rendered on the ad and never exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Ad;
    use crate::test_support::sample_campaign;

    #[test]
    fn ad_round_trips_through_json() {
        let ad = Ad {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Link: https://example.com. Alt: A short hello.".to_owned()),
        };
        let json = serde_json::to_string(&ad).unwrap();
        let restored: Ad = serde_json::from_str(&json).unwrap();
        assert_eq!(ad, restored);
    }

    #[test]
    fn absent_ad_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_campaign().ads[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn an_ad_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Primary", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Ad>(named).is_err());
    }
}
