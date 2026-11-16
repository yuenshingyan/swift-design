//! Round-trips the checked-in sample campaign that agents use as an example.

use std::fs;
use std::path::Path;

use design_model::{AdSize, Campaign};

#[test]
fn sample_campaign_fixture_parses_validates_and_round_trips() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-campaign.json");
    let raw = fs::read_to_string(path).expect("test content should be readable as text");
    let campaign: Campaign = serde_json::from_str(&raw).expect("test JSON should deserialize");
    assert_eq!(campaign.validate(), Vec::new());
    assert_eq!(campaign.ads.len(), 2);
    assert_eq!(campaign.size, AdSize::MediumRectangle);
    let original: serde_json::Value =
        serde_json::from_str(&raw).expect("test JSON should deserialize");
    let reserialized = serde_json::to_value(&campaign).expect("test value should convert to JSON");
    assert_eq!(original, reserialized);
}
