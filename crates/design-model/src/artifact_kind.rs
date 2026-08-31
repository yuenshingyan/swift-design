//! The artifact kind: what a session builds.
//!
//! A session builds one kind of artifact. A `demo` is a software demo
//! such as a landing page or a set of app screens, laid out on a device
//! viewport. A `deck` is a slide presentation on a 1920 by 1080 px
//! canvas. A `document` is a paged document, such as a report or a
//! memo, on A4 or Letter paper. A `social` is a post or a carousel on
//! a social canvas. The kinds have separate types, stores, routes, and
//! editors; the brief-first workflow is the same for all.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a session builds: a software demo, a deck, a document, or a
/// social.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// A software demo: a landing page, app screens, or a similar layout
    /// on a device viewport. Written as a design.
    #[default]
    Demo,
    /// A slide presentation on a 1920 by 1080 px canvas. Written as a
    /// deck.
    Deck,
    /// A paged document on A4 or Letter paper: a report, a memo, a
    /// proposal, a letter, or a guide. Written as a document.
    Document,
    /// A social post or a carousel on a square, portrait, story, or
    /// landscape canvas. Written as a social.
    Social,
}

impl ArtifactKind {
    /// Every kind, in the order the UI shows them.
    pub const ALL: [ArtifactKind; 4] = [
        ArtifactKind::Demo,
        ArtifactKind::Deck,
        ArtifactKind::Document,
        ArtifactKind::Social,
    ];

    /// The snake_case name used in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Demo => "demo",
            ArtifactKind::Deck => "deck",
            ArtifactKind::Document => "document",
            ArtifactKind::Social => "social",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            ArtifactKind::Demo => "Software demo",
            ArtifactKind::Deck => "Deck",
            ArtifactKind::Document => "Document",
            ArtifactKind::Social => "Social post",
        }
    }

    /// The kind for a JSON name, if `value` is one.
    pub fn from_name(value: &str) -> Option<ArtifactKind> {
        ArtifactKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::ArtifactKind;

    #[test]
    fn artifact_kind_round_trips_through_json() {
        for kind in ArtifactKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let restored: ArtifactKind = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, kind);
        }
    }

    #[test]
    fn artifact_kind_defaults_to_demo() {
        assert_eq!(ArtifactKind::default(), ArtifactKind::Demo);
    }

    #[test]
    fn unknown_kinds_are_rejected() {
        assert!(serde_json::from_str::<ArtifactKind>("\"poster\"").is_err());
        assert_eq!(ArtifactKind::from_name("poster"), None);
        assert_eq!(ArtifactKind::from_name("deck"), Some(ArtifactKind::Deck));
        assert_eq!(
            ArtifactKind::from_name("document"),
            Some(ArtifactKind::Document)
        );
        assert_eq!(
            ArtifactKind::from_name("social"),
            Some(ArtifactKind::Social)
        );
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(ArtifactKind::Demo.label(), "Software demo");
        assert_eq!(ArtifactKind::Deck.label(), "Deck");
        assert_eq!(ArtifactKind::Document.label(), "Document");
        assert_eq!(ArtifactKind::Social.label(), "Social post");
    }
}
