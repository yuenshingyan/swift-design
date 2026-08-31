//! Email type: one HTML fragment plus its own CSS, for a mailing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One email: an HTML fragment for the mailing's format, CSS for that
/// fragment, and author notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Email {
    /// The email as one HTML fragment. The canvas is the mailing's
    /// `format` in px, 600 wide: 600 by 800 for short, 600 by 1200 for
    /// standard, 600 by 1800 for long. Use px units. Allowed: headings,
    /// text, lists, tables, `<img>` with `src="/uploads/{name}"`,
    /// inline `<svg>`, `<pre><code>`, `<blockquote>`, `<a>`. Not
    /// allowed: `<script>`, `<style>`, `<iframe>`, `<object>`,
    /// `<embed>`, `<link>`, `<meta>`, `<form>`, `<button>`, `<input>`,
    /// media, comments, `on*` attributes, `javascript:` and `data:`
    /// URLs, external images. Close every tag.
    pub html: String,
    /// CSS for this email only. The server scopes every selector to
    /// this email. Use plain selectors such as `.title` or `h1`. Use the
    /// theme through `var(--background)`, `var(--text)`, `var(--accent)`,
    /// `var(--muted)`, `var(--heading-font)`, `var(--body-font)`,
    /// `var(--mono-font)`. No `@import`, no viewport units, no `<`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Author notes: the subject line and the preheader text, as a
    /// `Subject:` line and a `Preheader:` line, plus intent or handoff
    /// remarks. Never rendered on the email and never exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::Email;
    use crate::test_support::sample_mailing;

    #[test]
    fn email_round_trips_through_json() {
        let email = Email {
            html: "<h1>Hi</h1>".to_owned(),
            css: Some("h1 { color: red; }".to_owned()),
            notes: Some("Subject: Hi. Preheader: A short hello.".to_owned()),
        };
        let json = serde_json::to_string(&email).unwrap();
        let restored: Email = serde_json::from_str(&json).unwrap();
        assert_eq!(email, restored);
    }

    #[test]
    fn absent_email_fields_stay_out_of_the_json() {
        let json = serde_json::to_value(&sample_mailing().emails[0]).unwrap();
        assert!(json.get("notes").is_none());
        assert!(json.get("html").is_some());
    }

    #[test]
    fn an_email_with_a_name_is_rejected() {
        let named = serde_json::json!({ "name": "Launch", "html": "<h1>Hi</h1>" });
        assert!(serde_json::from_value::<Email>(named).is_err());
    }
}
