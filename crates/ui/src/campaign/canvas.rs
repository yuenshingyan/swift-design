//! Builds candidate cards for this artifact.

use design_model::ArtifactKind;

use crate::canvas::CanvasCard;
use crate::settings::artifact_project;

/// The campaign cards that belong to `session_id`, chosen campaign
/// first.
pub(crate) fn cards_from_campaigns(
    campaigns: &[crate::campaign::api::CampaignSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = campaigns
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Campaign,
            count: summary.ad_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}
