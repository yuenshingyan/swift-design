//! Round-trips the checked-in sample social that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::{Format, Social};

#[test]
fn sample_social_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-social.json");
    let raw = fs::read_to_string(path).unwrap();
    let social: Social = serde_json::from_str(&raw).unwrap();
    assert_eq!(social.validate(), Vec::new());
    assert_eq!(social.frames.len(), 3);
    assert_eq!(social.format, Format::Portrait);
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&social).unwrap();
    assert_eq!(original, reserialized);
}
