//! Screen type: one HTML fragment plus its own CSS.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One screen: an HTML fragment for the design's viewport, CSS for
/// that fragment, a name, and notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Screen {
    /// A short name for the screen, such as `Home` or `Checkout`. Shown
    /// in the editor. Empty means the editor derives one from the html.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The screen as one HTML fragment. The canvas is the design's
    /// `viewport` in px; use px units. Allowed: headings, text, lists, tables, `<img>`
    /// with `src="/uploads/{name}"`, inline `<svg>`, `<pre><code>`,
    /// `<blockquote>`, `<a>`, `<details>`, `<label>`, `<input>` with
    /// type `checkbox` or `radio`. Not allowed: `<script>`, `<style>`,
    /// `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, `<form>`,
    /// `<button>`, other input types, media, comments, `on*`
    /// attributes, `javascript:` and `data:` URLs, external images.
    /// Close every tag. A link with `href="#screen-3"` opens screen 3,
    /// counted from 1.
    pub html: String,
    /// CSS for this screen only. The server scopes every selector to
    /// this screen. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Design notes: intent, states, or handoff remarks. Never rendered
    /// on the screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Screen {
    /// Whether this screen is on a phone canvas or is named as a standalone
    /// phone specimen inside a larger canvas. Combined desktop/mobile views stay together.
    pub fn is_mobile(&self, viewport: crate::Viewport) -> bool {
        if viewport.width <= 480 && viewport.height > viewport.width {
            return true;
        }
        let mut mobile = false;
        for word in self.name.split(|ch: char| !ch.is_ascii_alphanumeric()) {
            if word.eq_ignore_ascii_case("desktop") {
                return false;
            }
            mobile |= word.eq_ignore_ascii_case("mobile")
                || word.eq_ignore_ascii_case("phone")
                || word
                    .to_ascii_lowercase()
                    .strip_suffix("px")
                    .is_some_and(|width| {
                        width
                            .parse::<u16>()
                            .is_ok_and(|width| (320..=480).contains(&width))
                    });
        }
        mobile
    }
}

#[cfg(test)]
mod tests {
    use crate::Screen;
    use crate::test_support::sample_design;

    #[test]
    fn screen_round_trips_through_json() {
        let screen = Screen {
            name: "Hero".to_owned(),
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Speak slowly.".to_owned()),
        };
        let json = serde_json::to_string(&screen).expect("test value should serialize to JSON");
        let restored: Screen = serde_json::from_str(&json).expect("test JSON should deserialize");
        assert_eq!(screen, restored);
    }

    #[test]
    fn absent_optional_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_design().screens[0])
            .expect("test value should convert to JSON");
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn screen_name_round_trips_and_stays_out_when_empty() {
        let mut screen = sample_design().screens[0].clone();
        screen.name = String::new();
        let json = serde_json::to_value(&screen).expect("test value should convert to JSON");
        assert!(json.get("name").is_none());
        let restored: Screen =
            serde_json::from_value(json).expect("test JSON value should deserialize");
        assert_eq!(restored.name, "");
        screen.name = "Home".to_owned();
        let json = serde_json::to_value(&screen).expect("test value should convert to JSON");
        assert_eq!(json["name"], "Home");
    }

    #[test]
    fn named_phone_specimens_and_phone_canvases_are_mobile() {
        let mut screen = sample_design().screens[0].clone();
        for name in [
            "Project Access · 390px mobile",
            "Activity · 320px containment host",
            "Task Details · Phone",
        ] {
            screen.name = name.into();
            assert!(screen.is_mobile(crate::Viewport::default()), "{name}");
        }
        for name in [
            "Statistics · desktop and mobile",
            "Desktop · 390px notes",
            "Activity · 768px host",
            "Project Access",
        ] {
            screen.name = name.into();
            assert!(!screen.is_mobile(crate::Viewport::default()), "{name}");
        }
        assert!(screen.is_mobile(crate::Viewport {
            width: 390,
            height: 844
        }));
    }

    #[test]
    fn element_designs_are_rejected() {
        let old = serde_json::json!({ "elements": [], "notes": "x" });
        assert!(serde_json::from_value::<Screen>(old).is_err());
    }
}
