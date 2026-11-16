#![allow(dead_code)]

use design_model::Design;
use gloo_net::http::Request;
use serde::Deserialize;

use crate::api::*;

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

/// Fetches a temporary draft without ever substituting it for saved data.
pub async fn fetch_design_preview(id: &str) -> Result<Option<Design>, String> {
    #[derive(Deserialize)]
    struct PreviewResponse {
        design: Design,
    }
    let url = format!("/designs/{id}/previews");
    let response = built(Request::get(&url))?
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == 404 {
        return Ok(None);
    }
    if !response.ok() {
        return Err(format!(
            "Cannot load live draft: status {}",
            response.status()
        ));
    }
    response
        .json::<PreviewResponse>()
        .await
        .map(|preview| Some(preview.design))
        .map_err(|error| error.to_string())
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

/// Fetches the field paths the user changed in this design.
pub async fn fetch_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/designs/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
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
