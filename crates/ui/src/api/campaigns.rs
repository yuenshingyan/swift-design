use super::*;

// -- Campaigns -----------------------------------------------------------

/// One row of `GET /campaigns`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CampaignSummary {
    /// Campaign id used in `/campaigns/{id}` routes.
    pub id: String,
    /// Campaign title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The size the ads are laid out on.
    #[serde(default)]
    pub size: AdSize,
    /// Number of ads.
    pub ad_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder ads a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl CampaignSummary {
    /// True when the campaign is a preview that waits for its ads.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.ad_count
    }

    /// True when the campaign still owes ads.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every ad.
    pub fn viewport(&self) -> design_model::Viewport {
        self.size.viewport()
    }

    /// The CSS aspect-ratio of the campaign's canvas.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the campaign listing.
pub async fn fetch_campaign_list() -> Result<Vec<CampaignSummary>, String> {
    get_json("/campaigns").await
}

/// Fetches one campaign.
pub async fn fetch_campaign(id: &str) -> Result<Campaign, String> {
    get_json(&format!("/campaigns/{id}")).await
}

/// Saves one campaign as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_campaign(id: &str, campaign: &Campaign) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/campaigns/{id}"))
        .header("x-swift-design-author", "user")
        .json(campaign)
        .map_err(|error| vec![error.to_string()])?
        .send()
        .await
        .map_err(|error| vec![error.to_string()])?;
    if response.ok() {
        return Ok(());
    }
    let status = response.status();
    match response.json::<ErrorEnvelope>().await {
        Ok(envelope) if !envelope.error.details.is_empty() => Err(envelope.error.details),
        Ok(envelope) => Err(vec![envelope.error.message]),
        Err(_) => Err(vec![format!(
            "PUT /campaigns/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one campaign.
pub async fn delete_campaign(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/campaigns/{id}")),
        "DELETE /campaigns",
    )
    .await
}

/// Fetches the field paths the user changed in this campaign.
pub async fn fetch_campaign_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/campaigns/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one campaign.
pub async fn fetch_campaign_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/campaigns/{id}/history")).await
}

/// Writes one snapshot back as the current campaign.
pub async fn restore_campaign_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/campaigns/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
