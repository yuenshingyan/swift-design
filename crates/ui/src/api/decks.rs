use super::*;

// -- Decks ---------------------------------------------------------------

/// One row of `GET /decks`, mirrored from the server.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeckSummary {
    /// Deck id used in `/decks/{id}` routes.
    pub id: String,
    /// Deck title.
    pub title: String,
    /// Theme name.
    pub theme: String,
    /// Number of slides.
    pub slide_count: usize,
    /// Number of titles in the planned outline.
    #[serde(default)]
    pub outline_count: usize,
    /// Number of placeholder slides a run left behind.
    #[serde(default)]
    pub pending_count: usize,
}

impl DeckSummary {
    /// True when the deck is a preview that waits for its slides.
    pub fn is_preview(&self) -> bool {
        self.outline_count > self.slide_count
    }

    /// True when the deck still owes slides.
    pub fn is_unfinished(&self) -> bool {
        self.is_preview() || self.pending_count > 0
    }

    /// The CSS aspect-ratio of every deck: 16:9.
    pub fn aspect_ratio(&self) -> String {
        DECK_VIEWPORT.aspect_ratio_css()
    }
}

/// Fetches the deck listing.
pub async fn fetch_deck_list() -> Result<Vec<DeckSummary>, String> {
    get_json("/decks").await
}

/// Fetches one deck.
pub async fn fetch_deck(id: &str) -> Result<Deck, String> {
    get_json(&format!("/decks/{id}")).await
}

/// How many slides of the deck are placeholders.
pub fn pending_slide_count(deck: &Deck) -> usize {
    deck.slides
        .iter()
        .filter(|slide| slide.html.contains(PENDING_SCREEN_CLASS))
        .count()
}

/// Saves one deck as a user edit. `Err` carries one message per
/// problem, so the editor can show every validation error at once.
pub async fn save_deck(id: &str, deck: &Deck) -> Result<(), Vec<String>> {
    let response = Request::put(&format!("/decks/{id}"))
        .header("x-swift-design-author", "user")
        .json(deck)
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
        Err(_) => Err(vec![format!("PUT /decks/{id} failed with status {status}")]),
    }
}

/// Deletes one deck.
pub async fn delete_deck(id: &str) -> Result<(), String> {
    send_empty(Request::delete(&format!("/decks/{id}")), "DELETE /decks").await
}

/// Fetches the field paths the user changed in this deck.
pub async fn fetch_deck_user_paths(id: &str) -> Result<Vec<String>, String> {
    get_json::<AuthorsResponse>(&format!("/decks/{id}/authors"))
        .await
        .map(|authors| authors.user_paths)
}

/// Fetches the saved snapshots of one deck.
pub async fn fetch_deck_history(id: &str) -> Result<Vec<HistorySnapshot>, String> {
    get_json(&format!("/decks/{id}/history")).await
}

/// Writes one snapshot back as the current deck.
pub async fn restore_deck_history(id: &str, stamp: &str) -> Result<(), String> {
    send_empty(
        Request::post(&format!("/decks/{id}/history/{stamp}/restore")),
        "restore",
    )
    .await
}
