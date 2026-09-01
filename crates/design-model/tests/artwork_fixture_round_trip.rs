//! Round-trips the checked-in sample artwork that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::{Artwork, CoverSize};

#[test]
fn sample_artwork_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-artwork.json");
    let raw = fs::read_to_string(path).unwrap();
    let artwork: Artwork = serde_json::from_str(&raw).unwrap();
    assert_eq!(artwork.validate(), Vec::new());
    assert_eq!(artwork.covers.len(), 2);
    assert_eq!(artwork.size, CoverSize::Thumbnail);
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&artwork).unwrap();
    assert_eq!(original, reserialized);
}
