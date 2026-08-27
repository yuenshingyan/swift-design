//! Round-trips the checked-in sample deck that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::Deck;

#[test]
fn sample_deck_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-deck.json");
    let raw = fs::read_to_string(path).unwrap();
    let deck: Deck = serde_json::from_str(&raw).unwrap();
    assert_eq!(deck.validate(), Vec::new());
    assert_eq!(deck.slides.len(), 3);
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&deck).unwrap();
    assert_eq!(original, reserialized);
}
