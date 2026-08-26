//! Round-trips the checked-in sample design that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::Design;

#[test]
fn sample_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-design.json");
    let raw = fs::read_to_string(path).unwrap();
    let design: Design = serde_json::from_str(&raw).unwrap();
    assert_eq!(design.validate(), Vec::new());
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&design).unwrap();
    assert_eq!(original, reserialized);
}
