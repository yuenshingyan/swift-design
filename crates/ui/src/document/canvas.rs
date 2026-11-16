//! Builds candidate cards for this artifact.

use design_model::ArtifactKind;

use crate::canvas::CanvasCard;
use crate::settings::artifact_project;

/// The document cards that belong to `session_id`, chosen document
/// first.
pub(crate) fn cards_from_documents(
    documents: &[crate::document::api::DocumentSummary],
    session_id: &str,
    chosen: Option<&str>,
) -> Vec<CanvasCard> {
    let mut mine: Vec<CanvasCard> = documents
        .iter()
        .filter(|summary| artifact_project(&summary.id) == session_id)
        .map(|summary| CanvasCard {
            id: summary.id.clone(),
            title: summary.title.clone(),
            kind: ArtifactKind::Document,
            count: summary.page_count,
            outline_count: summary.outline_count,
            ratio: summary.aspect_ratio(),
            viewport: summary.viewport(),
        })
        .collect();
    mine.sort_by_key(|card| Some(card.id.as_str()) != chosen);
    mine
}
