use super::*;

// -- Prints --------------------------------------------------------------

/// One row of `GET /prints`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PrintSummary {
    /// Print id used in `/prints/{id}` routes.
    pub id: String,
    /// Print title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The paper size the sheets are laid out on.
    #[serde(default)]
    pub size: PrintSize,
    /// How the sheets are turned.
    #[serde(default)]
    pub orientation: Orientation,
    /// Number of sheets.
    pub sheet_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder sheets a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl PrintSummary {
    /// True when the print is a preview that waits for its sheets.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.sheet_count
    }

    /// True when the print still owes sheets.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every sheet: the size rotated by the
    /// orientation.
    pub fn viewport(&self) -> design_model::Viewport {
        self.orientation.apply(self.size.viewport())
    }

    /// The CSS aspect-ratio of the print's canvas.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the print listing.
pub async fn fetch_print_list() -> Result<Vec<PrintSummary>, String> {
    get_json("/prints").await
}

/// Fetches one print.
pub async fn fetch_print(id: &str) -> Result<Print, String> {
    get_json(&format!("/prints/{id}")).await
}

/// Saves one print as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_print(id: &str, print: &Print) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/prints/{id}"))
        .header("x-swift-design-author", "user")
        .json(print)
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
            "PUT /prints/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one print.
pub async fn delete_print(id: &str) -> Result<(), String> {
    send_empty(Request::delete(&format!("/prints/{id}")), "DELETE /prints").await
}

/// Fetches the field paths the user changed in this print.
pub async fn fetch_print_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/prints/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one print.
pub async fn fetch_print_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/prints/{id}/history")).await
}

/// Writes one snapshot back as the current print.
pub async fn restore_print_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/prints/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
