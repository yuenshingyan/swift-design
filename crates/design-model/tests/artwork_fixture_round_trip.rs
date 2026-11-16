//! Round-trips the checked-in sample artwork that agents use as an example.

use std::fs;
use std::path::Path;

use design_model::{Artwork, CoverSize};

#[test]
fn sample_artwork_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-artwork.json");
    let raw = fs::read_to_string(path).expect("test content should be readable as text");
    let artwork: Artwork = serde_json::from_str(&raw).expect("test JSON should deserialize");
    assert_eq!(artwork.validate(), Vec::new());
    assert_eq!(artwork.covers.len(), 2);
    assert_eq!(artwork.size, CoverSize::Thumbnail);
    let original: serde_json::Value =
        serde_json::from_str(&raw).expect("test JSON should deserialize");
    let reserialized = serde_json::to_value(&artwork).expect("test value should convert to JSON");
    assert_eq!(original, reserialized);
}
