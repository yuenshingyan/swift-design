//! Round-trips the checked-in sample social that agents use as an example.

use std::fs;
use std::path::Path;

use design_model::{Format, Social};

#[test]
fn sample_social_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-social.json");
    let raw = fs::read_to_string(path).expect("test content should be readable as text");
    let social: Social = serde_json::from_str(&raw).expect("test JSON should deserialize");
    assert_eq!(social.validate(), Vec::new());
    assert_eq!(social.frames.len(), 3);
    assert_eq!(social.format, Format::Portrait);
    let original: serde_json::Value =
        serde_json::from_str(&raw).expect("test JSON should deserialize");
    let reserialized = serde_json::to_value(&social).expect("test value should convert to JSON");
    assert_eq!(original, reserialized);
}
