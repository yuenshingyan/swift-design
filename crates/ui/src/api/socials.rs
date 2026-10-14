use super::*;

// -- Socials -------------------------------------------------------------

/// One row of `GET /socials`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SocialSummary {
    /// Social id used in `/socials/{id}` routes.
    pub id: String,
    /// Social title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The format the frames are laid out on.
    #[serde(default)]
    pub format: Format,
    /// Number of frames.
    pub frame_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder frames a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl SocialSummary {
    /// True when the social is a preview that waits for its frames.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.frame_count
    }

    /// True when the social still owes frames.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every frame.
    pub fn viewport(&self) -> design_model::Viewport {
        self.format.viewport()
    }

    /// The CSS aspect-ratio of the social's format.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the social listing.
pub async fn fetch_social_list() -> Result<Vec<SocialSummary>, String> {
    get_json("/socials").await
}

/// Fetches one social.
pub async fn fetch_social(id: &str) -> Result<Social, String> {
    get_json(&format!("/socials/{id}")).await
}

/// Saves one social as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_social(id: &str, social: &Social) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/socials/{id}"))
        .header("x-swift-design-author", "user")
        .json(social)
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
            "PUT /socials/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one social.
pub async fn delete_social(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/socials/{id}")),
        "DELETE /socials",
    )
    .await
}

/// Fetches the field paths the user changed in this social.
pub async fn fetch_social_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/socials/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one social.
pub async fn fetch_social_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/socials/{id}/history")).await
}

/// Writes one snapshot back as the current social.
pub async fn restore_social_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/socials/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
