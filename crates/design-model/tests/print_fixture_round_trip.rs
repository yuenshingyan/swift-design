//! Round-trips the checked-in sample print that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::{Orientation, Print, PrintSize};

#[test]
fn sample_print_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-print.json");
    let raw = fs::read_to_string(path).unwrap();
    let print: Print = serde_json::from_str(&raw).unwrap();
    assert_eq!(print.validate(), Vec::new());
    assert_eq!(print.sheets.len(), 2);
    assert_eq!(print.size, PrintSize::A4);
    assert_eq!(print.orientation, Orientation::Portrait);
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&print).unwrap();
    assert_eq!(original, reserialized);
}
