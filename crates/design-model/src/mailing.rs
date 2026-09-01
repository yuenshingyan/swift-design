//! The top-level mailing type: an email or an email sequence.
//!
//! A mailing is the sixth artifact kind next to `Design`, `Deck`,
//! `Document`, `Social`, and `Print`. It has a title, a theme, a
//! `format`, `emails`, and an optional `outline`. One email is a
//! single send; two or more are a sequence, in send order. Every email
//! is laid out on the fixed px canvas of its format, 600 px wide, so a
//! mailing has no `viewport` field and no transition.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::email::Email;
use crate::theme::Theme;
use crate::viewport::Viewport;

/// Most emails a mailing run may write: a welcome sequence or a short
/// promotion series. A longer sequence is written in a later run.
pub const EMAIL_COUNT_LIMIT: u32 = 5;

/// The short email: 600 by 800 px, an announcement read in one glance.
pub const SHORT_EMAIL_VIEWPORT: Viewport = Viewport {
    width: 600,
    height: 800,
};

/// The standard email: 600 by 1200 px, the common one-scroll email.
pub const STANDARD_EMAIL_VIEWPORT: Viewport = Viewport {
    width: 600,
    height: 1200,
};

/// The long email: 600 by 1800 px, a newsletter or a digest.
pub const LONG_EMAIL_VIEWPORT: Viewport = Viewport {
    width: 600,
    height: 1800,
};

/// The canvas an email is laid out on. Every format is 600 px wide,
/// the email-client column; the formats differ in height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmailFormat {
    /// Short: 600 by 800 px.
    Short,
    /// Standard: 600 by 1200 px.
    #[default]
    Standard,
    /// Long: 600 by 1800 px.
    Long,
}

impl EmailFormat {
    /// Every format, in the order the UI shows them.
    pub const ALL: [EmailFormat; 3] =
        [EmailFormat::Short, EmailFormat::Standard, EmailFormat::Long];

    /// The px canvas of one email.
    pub fn viewport(self) -> Viewport {
        match self {
            EmailFormat::Short => SHORT_EMAIL_VIEWPORT,
            EmailFormat::Standard => STANDARD_EMAIL_VIEWPORT,
            EmailFormat::Long => LONG_EMAIL_VIEWPORT,
        }
    }

    /// The snake_case name used in JSON and in the run options.
    pub fn as_str(self) -> &'static str {
        match self {
            EmailFormat::Short => "short",
            EmailFormat::Standard => "standard",
            EmailFormat::Long => "long",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            EmailFormat::Short => "Short",
            EmailFormat::Standard => "Standard",
            EmailFormat::Long => "Long",
        }
    }

    /// The format for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<EmailFormat> {
        EmailFormat::ALL
            .into_iter()
            .find(|format| format.as_str() == value.trim())
    }
}

/// A complete mailing: one theme applied to an ordered list of emails
/// on one format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mailing {
    /// Mailing title, shown in the editor. Not rendered on an email.
    pub title: String,
    /// Visual identity applied to every email.
    pub theme: Theme,
    /// The format every email is laid out on. Absent means standard.
    #[serde(default)]
    pub format: EmailFormat,
    /// Emails in send order. One email is a single send; two or more
    /// are a sequence.
    pub emails: Vec<Email>,
    /// The email titles of the complete mailing, in order. A preview
    /// mailing lists more titles than it has emails: the app continues
    /// it from this list later. Leave it empty when the mailing is
    /// complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<String>,
}

impl Mailing {
    /// True when the outline plans more emails than the mailing has:
    /// the mailing is a preview that waits for its remaining emails.
    pub fn is_preview(&self) -> bool {
        self.outline.len() > self.emails.len()
    }

    /// The px canvas of every email.
    pub fn viewport(&self) -> Viewport {
        self.format.viewport()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::sample_mailing;
    use crate::{
        EmailFormat, LONG_EMAIL_VIEWPORT, Mailing, SHORT_EMAIL_VIEWPORT, STANDARD_EMAIL_VIEWPORT,
    };

    #[test]
    fn mailing_round_trips_through_json() {
        let mailing = sample_mailing();
        let json = serde_json::to_string(&mailing).unwrap();
        assert!(json.contains("\"emails\""));
        assert!(json.contains("\"format\":\"standard\""));
        assert!(!json.contains("\"viewport\""));
        let restored: Mailing = serde_json::from_str(&json).unwrap();
        assert_eq!(mailing, restored);
    }

    #[test]
    fn rejects_unknown_mailing_fields() {
        let mut value = serde_json::to_value(sample_mailing()).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Mailing>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("extra");
        value["viewport"] = serde_json::json!({"width": 600, "height": 1200});
        assert!(serde_json::from_value::<Mailing>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("viewport");
        value["transition"] = serde_json::json!({"effect": "fade"});
        assert!(serde_json::from_value::<Mailing>(value).is_err());
    }

    #[test]
    fn the_format_defaults_to_standard() {
        let mut value = serde_json::to_value(sample_mailing()).unwrap();
        value.as_object_mut().unwrap().remove("format");
        let restored: Mailing = serde_json::from_value(value).unwrap();
        assert_eq!(restored.format, EmailFormat::Standard);
        assert_eq!(restored.viewport(), STANDARD_EMAIL_VIEWPORT);
    }

    #[test]
    fn an_outline_longer_than_the_emails_marks_a_preview() {
        let mut mailing = sample_mailing();
        assert!(!mailing.is_preview());
        assert!(!serde_json::to_string(&mailing).unwrap().contains("outline"));
        mailing.outline = vec!["Launch".to_owned(), "Follow-up".to_owned()];
        assert!(mailing.is_preview());
        let json = serde_json::to_string(&mailing).unwrap();
        let restored: Mailing = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.outline, mailing.outline);
        mailing.outline.truncate(1);
        assert!(!mailing.is_preview());
    }

    #[test]
    fn the_formats_are_valid_and_named() {
        for format in EmailFormat::ALL {
            let viewport = format.viewport();
            assert!(viewport.is_valid(), "{}", format.label());
            assert_eq!(viewport.width, 600, "{}", format.label());
            assert_eq!(EmailFormat::from_name(format.as_str()), Some(format));
            let json = serde_json::to_string(&format).unwrap();
            assert_eq!(json, format!("\"{}\"", format.as_str()));
        }
        assert_eq!(SHORT_EMAIL_VIEWPORT.aspect_ratio_css(), "600 / 800");
        assert_eq!(STANDARD_EMAIL_VIEWPORT.aspect_ratio_css(), "600 / 1200");
        assert_eq!(LONG_EMAIL_VIEWPORT.aspect_ratio_css(), "600 / 1800");
        assert_eq!(EmailFormat::from_name("tall"), None);
    }
}
