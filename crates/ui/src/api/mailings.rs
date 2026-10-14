use super::*;

// -- Mailings ------------------------------------------------------------

/// One row of `GET /mailings`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MailingSummary {
    /// Mailing id used in `/mailings/{id}` routes.
    pub id: String,
    /// Mailing title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// The format the emails are laid out on.
    #[serde(default)]
    pub format: EmailFormat,
    /// Number of emails.
    pub email_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder emails a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl MailingSummary {
    /// True when the mailing is a preview that waits for its emails.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.email_count
    }

    /// True when the mailing still owes emails.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The px canvas of every email.
    pub fn viewport(&self) -> design_model::Viewport {
        self.format.viewport()
    }

    /// The CSS aspect-ratio of the mailing's canvas.
    pub fn aspect_ratio(&self) -> String {
        self.viewport().aspect_ratio_css()
    }
}

/// Fetches the mailing listing.
pub async fn fetch_mailing_list() -> Result<Vec<MailingSummary>, String> {
    get_json("/mailings").await
}

/// Fetches one mailing.
pub async fn fetch_mailing(id: &str) -> Result<Mailing, String> {
    get_json(&format!("/mailings/{id}")).await
}

/// Saves one mailing as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_mailing(id: &str, mailing: &Mailing) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/mailings/{id}"))
        .header("x-swift-design-author", "user")
        .json(mailing)
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
            "PUT /mailings/{id} failed with status {status}"
        )]),
    }
}

/// Deletes one mailing.
pub async fn delete_mailing(id: &str) -> Result<(), String> {
    send_empty(
        Request::delete(&format!("/mailings/{id}")),
        "DELETE /mailings",
    )
    .await
}

/// Fetches the field paths the user changed in this mailing.
pub async fn fetch_mailing_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/mailings/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one mailing.
pub async fn fetch_mailing_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/mailings/{id}/history")).await
}

/// Writes one snapshot back as the current mailing.
pub async fn restore_mailing_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/mailings/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
