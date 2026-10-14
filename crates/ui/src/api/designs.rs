use super::*;

// -- Designs -------------------------------------------------------------

/// One row of `GET /designs`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DesignSummary {
    /// Design id used in `/designs/{id}` routes.
    pub id: String,
    /// Design title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The px canvas of every screen.
    #[serde(default)]
    pub viewport: design_model::Viewport,
    /// Number of screens.
    pub screen_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder screens a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl DesignSummary {
    /// True when the design is a preview that waits for its screens.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.screen_count
    }

    /// True when the design still owes screens.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The CSS aspect-ratio for the design's viewport.
    pub fn aspect_ratio(&self) -> String {
        self.viewport.aspect_ratio_css()
    }
}

/// Fetches the design listing.
pub async fn fetch_design_list() -> Result<Vec<DesignSummary>, String> {
    get_json("/designs").await
}

/// Fetches one design.
pub async fn fetch_design(id: &str) -> Result<Design, String> {
    get_json(&format!("/designs/{id}")).await
}

/// The CSS class the server puts on a placeholder screen.
pub(crate) const PENDING_SCREEN_CLASS: &str = "swift-design-pending";

/// How many screens of the design are placeholders.
pub fn pending_screen_count(design: &Design) -> usize {
    design
        .screens
        .iter()
        .filter(|screen| screen.html.contains(PENDING_SCREEN_CLASS))
        .count()
}

/// Saves one design as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_design(id: &str, design: &Design) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/designs/{id}"))
        .header("x-swift-design-author", "user")
        .json(design)
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
            "PUT /designs/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one design.
pub async fn delete_design(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/designs/{id}")),
        "DELETE /designs",
    )
    .await
}

/// Response of a fork: the id of the new candidate.
#[derive(Debug, Deserialize)]
struct ForkResponse {
    id: String,
}

/// Copies one design candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_design(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/designs/{id}/fork")))?;
    let response = send_checked(request, "POST /designs/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one deck candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_deck(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/decks/{id}/fork")))?;
    let response = send_checked(request, "POST /decks/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one document candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_document(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/documents/{id}/fork")))?;
    let response = send_checked(request, "POST /documents/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one social candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_social(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/socials/{id}/fork")))?;
    let response = send_checked(request, "POST /socials/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one print candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_print(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/prints/{id}/fork")))?;
    let response = send_checked(request, "POST /prints/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one mailing candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_mailing(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/mailings/{id}/fork")))?;
    let response = send_checked(request, "POST /mailings/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one campaign candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_campaign(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/campaigns/{id}/fork")))?;
    let response = send_checked(request, "POST /campaigns/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Copies one artwork candidate under the next free number of its
/// session. Returns the new id.
pub async fn fork_artwork(id: &str) -> Result<String, String> {
    let request = built(Request::post(&format!("/artworks/{id}/fork")))?;
    let response = send_checked(request, "POST /artworks/fork").await?;
    response
        .json::<ForkResponse>()
        .await
        .map(|fork| fork.id)
        .map_err(|error| error.to_string())
}

/// Response of `GET /designs/{id}/authors`.
#[derive(Debug, Deserialize)]
pub(crate) struct AuthorsResponse {
    pub(crate) user_paths: Vec<String>,
}

/// Fetches the field paths the user changed in this design.
pub async fn fetch_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/designs/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// One row of `GET /designs/{id}/history`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct HistorySnapshot {
    /// File stem of the snapshot.
    pub stamp: String,
    /// When it was taken, RFC 3339.
    pub saved_at: String,
    /// Size of the snapshot file.
    pub size_bytes: u64,
}

/// Fetches the saved snapshots of one design.
pub async fn fetch_design_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/designs/{id}/history")).await
}

/// Writes one snapshot back as the current design.
pub async fn restore_design_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/designs/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
