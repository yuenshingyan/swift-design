//! Round-trips the checked-in sample mailing that agents use as an example.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use design_model::{EmailFormat, Mailing};

#[test]
fn sample_mailing_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-mailing.json");
    let raw = fs::read_to_string(path).unwrap();
    let mailing: Mailing = serde_json::from_str(&raw).unwrap();
    assert_eq!(mailing.validate(), Vec::new());
    assert_eq!(mailing.emails.len(), 2);
    assert_eq!(mailing.format, EmailFormat::Standard);
    let original: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let reserialized = serde_json::to_value(&mailing).unwrap();
    assert_eq!(original, reserialized);
}
