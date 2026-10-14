use super::*;

// -- Artworks ------------------------------------------------------------

/// One row of `GET /artworks`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ArtworkSummary {
    /// Artwork id used in `/artworks/{id}` routes.
    pub id: String,
    /// Artwork title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The size the covers are laid out on.
    #[serde(default)]
    pub size: CoverSize,
    /// Number of covers.
    pub cover_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder covers a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl ArtworkSummary {
    /// True when the artwork is a preview that waits for its covers.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.cover_count
    }

    /// True when the artwork still owes covers.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every cover.
    pub fn viewport(&self) -> design_model::Viewport {
        self.size.viewport()
    }

    /// The CSS aspect-ratio of the artwork's canvas.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the artwork listing.
pub async fn fetch_artwork_list() -> Result<Vec<ArtworkSummary>, String> {
    get_json("/artworks").await
}

/// Fetches one artwork.
pub async fn fetch_artwork(id: &str) -> Result<Artwork, String> {
    get_json(&format!("/artworks/{id}")).await
}

/// Saves one artwork as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_artwork(id: &str, artwork: &Artwork) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/artworks/{id}"))
        .header("x-swift-design-author", "user")
        .json(artwork)
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
            "PUT /artworks/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one artwork.
pub async fn delete_artwork(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/artworks/{id}")),
        "DELETE /artworks",
    )
    .await
}

/// Fetches the field paths the user changed in this artwork.
pub async fn fetch_artwork_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/artworks/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one artwork.
pub async fn fetch_artwork_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/artworks/{id}/history")).await
}

/// Writes one snapshot back as the current artwork.
pub async fn restore_artwork_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/artworks/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
