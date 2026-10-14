use super::*;

// -- Documents -----------------------------------------------------------

/// One row of `GET /documents`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DocumentSummary {
    /// Document id used in `/documents/{id}` routes.
    pub id: String,
    /// Document title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The paper the pages are laid out on.
    #[serde(default)]
    pub paper: Paper,
    /// Number of pages.
    pub page_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder pages a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl DocumentSummary {
    /// True when the document is a preview that waits for its pages.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.page_count
    }

    /// True when the document still owes pages.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every page.
    pub fn viewport(&self) -> design_model::Viewport {
        self.paper.viewport()
    }

    /// The CSS aspect-ratio of the document's paper.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the document listing.
pub async fn fetch_document_list() -> Result<Vec<DocumentSummary>, String> {
    get_json("/documents").await
}

/// Fetches one document.
pub async fn fetch_document(id: &str) -> Result<Document, String> {
    get_json(&format!("/documents/{id}")).await
}

/// Saves one document as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_document(id: &str, document: &Document) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/documents/{id}"))
        .header("x-swift-design-author", "user")
        .json(document)
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
            "PUT /documents/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one document.
pub async fn delete_document(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/documents/{id}")),
        "DELETE /documents",
    )
    .await
}

/// Fetches the field paths the user changed in this document.
pub async fn fetch_document_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/documents/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one document.
pub async fn fetch_document_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/documents/{id}/history")).await
}

/// Writes one snapshot back as the current document.
pub async fn restore_document_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/documents/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
