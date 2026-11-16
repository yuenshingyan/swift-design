//! Round-trips the checked-in sample design that agents use as an example.

use std::fs;
use std::path::Path;

use design_model::Design;

#[test]
fn sample_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-design.json");
    let raw = fs::read_to_string(path).expect("test content should be readable as text");
    let design: Design = serde_json::from_str(&raw).expect("test JSON should deserialize");
    assert_eq!(design.validate(), Vec::new());
    let original: serde_json::Value =
        serde_json::from_str(&raw).expect("test JSON should deserialize");
    let reserialized = serde_json::to_value(&design).expect("test value should convert to JSON");
    assert_eq!(original, reserialized);
}
