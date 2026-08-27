//! The artifact kind: what a session builds.
//!
//! A session builds one kind of artifact. A `demo` is a software demo
//! such as a landing page or a set of app screens, laid out on a device
//! viewport. A `deck` is a slide presentation on a 1920 by 1080 px
//! canvas. The two kinds have separate types, stores, routes, and
//! editors; the brief-first workflow is the same for both.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a session builds: a software demo or a deck.
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
}

impl ArtifactKind {
    /// Every kind, in the order the UI shows them.
    pub const ALL: [ArtifactKind; 2] = [ArtifactKind::Demo, ArtifactKind::Deck];

    /// The snake_case name used in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Demo => "demo",
            ArtifactKind::Deck => "deck",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            ArtifactKind::Demo => "Software demo",
            ArtifactKind::Deck => "Deck",
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
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(ArtifactKind::Demo.label(), "Software demo");
        assert_eq!(ArtifactKind::Deck.label(), "Deck");
    }
}
