//! Round-trips the checked-in sample document that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::{Document, Paper};

#[test]
fn sample_document_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-document.json");
    let raw = fs::read_to_string(path).unwrap();
    let document: Document = serde_json::from_str(&raw).unwrap();
    assert_eq!(document.validate(), Vec::new());
    assert_eq!(document.pages.len(), 3);
    assert_eq!(document.paper, Paper::A4);
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&document).unwrap();
    assert_eq!(original, reserialized);
}
