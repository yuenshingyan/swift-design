//! Round-trips the checked-in sample document that agents use as an example.

use std::fs;
use std::path::Path;

use design_model::{Document, Paper};

#[test]
fn sample_document_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-document.json");
    let raw = fs::read_to_string(path).expect("test content should be readable as text");
    let document: Document = serde_json::from_str(&raw).expect("test JSON should deserialize");
    assert_eq!(document.validate(), Vec::new());
    assert_eq!(document.pages.len(), 3);
    assert_eq!(document.paper, Paper::A4);
    let original: serde_json::Value =
        serde_json::from_str(&raw).expect("test JSON should deserialize");
    let reserialized = serde_json::to_value(&document).expect("test value should convert to JSON");
    assert_eq!(original, reserialized);
}
