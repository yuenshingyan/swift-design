//! Deck storage and the `/decks` CRUD routes.
//!
//! Decks live as `<id>.json` files in one directory, next to but apart
//! from the designs. Agents may write files into that directory
//! directly; every request re-reads the filesystem, so nothing goes
//! stale. Every save first copies the current file into the
//! `HistoryStore`, and `/decks/{id}/history` lists and restores those
//! snapshots. This module is the deck twin of `designs.rs`.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::{Deck, Slide};
use serde::{Deserialize, Serialize};

use crate::designs::{PENDING_SCREEN_CLASS, is_valid_design_id};
use crate::events::ChangeNotifier;
use crate::history::{HistoryStore, Snapshot, is_valid_stamp};
use crate::{api_error, deck_render, presenter, provenance};

/// The CSS class on a slide that holds the place of one the model has
/// not written yet. The same marker as for screens.
pub const PENDING_SLIDE_CLASS: &str = PENDING_SCREEN_CLASS;

/// True when a slide only holds the place of one still to be written.
pub fn is_pending_slide(slide: &Slide) -> bool {
    slide.html.contains(PENDING_SLIDE_CLASS)
}

/// Filesystem-backed deck storage: one `<id>.json` file per deck.
#[derive(Clone)]
pub struct DeckStore {
    directory: PathBuf,
    /// Where each save keeps the previous file. `None` keeps no history.
    history: Option<HistoryStore>,
    /// Ids of malformed files already reported, so the listing warns
    /// once per file and not on every request.
    reported_malformed: Arc<Mutex<HashSet<String>>>,
}

/// One row in the `GET /decks` listing.
#[derive(Clone, Debug, Serialize)]
pub struct DeckSummary {
    /// File stem of the deck file, used in `/decks/{id}` routes.
    pub id: String,
    /// Deck title.
    pub title: String,
    /// Theme name, shown next to the id on chooser cards.
    pub theme: String,
    /// Number of slides.
    pub slide_count: usize,
    /// Number of titles in the planned outline. More than `slide_count`
    /// for a preview deck; 0 when the deck has no outline.
    pub outline_count: usize,
    /// Number of placeholder slides a running or stopped continuation
    /// left in the deck. 0 for a finished deck.
    pub pending_count: usize,
}

impl DeckStore {
    /// Creates a store over `directory`. The directory may not exist yet;
    /// it is created on the first save.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            history: None,
            reported_malformed: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Keeps a snapshot of the previous file in `history` on every save.
    pub fn with_history(mut self, history: HistoryStore) -> Self {
        self.history = Some(history);
        self
    }

    /// Logs a malformed deck file the first time it is seen.
    fn report_malformed(&self, id: &str, error: &serde_json::Error) {
        let is_first = self
            .reported_malformed
            .lock()
            .map(|mut reported| reported.insert(id.to_owned()))
            .unwrap_or(true);
        if is_first {
            tracing::warn!(
                %id,
                %error,
                "skipping malformed deck file: it does not match the current deck schema; delete or regenerate it"
            );
        }
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn authors_path_of(&self, id: &str) -> PathBuf {
        self.directory.join(".authors").join(format!("{id}.json"))
    }

    /// Field paths the user changed in this deck. Missing sidecar files
    /// mean everything is agent-authored.
    pub async fn user_paths(&self, id: &str) -> anyhow::Result<BTreeSet<String>> {
        match tokio::fs::read_to_string(self.authors_path_of(id)).await {
            Ok(raw) => Ok(serde_json::from_str(&raw)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
            Err(error) => Err(error.into()),
        }
    }

    async fn save_user_paths(&self, id: &str, paths: &BTreeSet<String>) -> anyhow::Result<()> {
        if paths.is_empty() {
            return self.clear_user_paths(id).await;
        }
        let sidecar = self.authors_path_of(id);
        if let Some(parent) = sidecar.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(sidecar, serde_json::to_string_pretty(paths)?).await?;
        Ok(())
    }

    /// Removes the authorship sidecar. Used on delete and when a
    /// candidate replaces a deck wholesale.
    pub async fn clear_user_paths(&self, id: &str) -> anyhow::Result<()> {
        match tokio::fs::remove_file(self.authors_path_of(id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Updates the authorship sidecar after a save: marks the paths this
    /// save changed as user or agent authored, and drops marks on
    /// removed paths.
    pub async fn record_authors(
        &self,
        id: &str,
        previous: Option<&Deck>,
        current: &Deck,
        is_user: bool,
    ) -> anyhow::Result<()> {
        let mut user_paths = self.user_paths(id).await?;
        match previous {
            Some(previous) => {
                let (changed, removed) = provenance::diff_paths(previous, current)?;
                for path in removed {
                    user_paths.remove(&path);
                }
                for path in changed {
                    if is_user {
                        user_paths.insert(path);
                    } else {
                        user_paths.remove(&path);
                    }
                }
            }
            None => {
                user_paths = if is_user {
                    provenance::field_paths(current)?.into_iter().collect()
                } else {
                    BTreeSet::new()
                };
            }
        }
        self.save_user_paths(id, &user_paths).await
    }

    /// Lists every parseable deck, sorted by id. Malformed files are
    /// logged and skipped so one bad file cannot break the listing.
    pub async fn list(&self) -> anyhow::Result<Vec<DeckSummary>> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut summaries = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            // A file can vanish between the directory read and this read
            // when a delete is in progress. Skip it.
            let raw = match tokio::fs::read_to_string(&path).await {
                Ok(raw) => raw,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            match serde_json::from_str::<Deck>(&raw) {
                Ok(deck) => summaries.push(summary_of(id, deck)),
                Err(error) => self.report_malformed(id, &error),
            }
        }
        summaries.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(summaries)
    }

    /// Loads one deck. `Ok(None)` means no file with that id exists.
    pub async fn load(&self, id: &str) -> anyhow::Result<Option<Deck>> {
        match tokio::fs::read_to_string(self.path_of(id)).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes one deck as pretty-printed JSON, creating the directory
    /// when needed. The previous file goes to the history store first.
    pub async fn save(&self, id: &str, deck: &Deck) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.directory).await?;
        self.snapshot_current(id).await;
        let json = serde_json::to_string_pretty(deck)?;
        tokio::fs::write(self.path_of(id), json + "\n").await?;
        Ok(())
    }

    /// Copies the current deck file into the history store. A deck with
    /// no file yet has nothing to keep. A history failure is logged and
    /// does not stop the save.
    async fn snapshot_current(&self, id: &str) {
        let Some(history) = &self.history else {
            return;
        };
        let content = match tokio::fs::read(self.path_of(id)).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(%id, %error, "could not read the deck file for its history snapshot");
                return;
            }
        };
        match history.snapshot(id, &content).await {
            Ok(stamp) => {
                tracing::debug!(%id, %stamp, size_bytes = content.len(), "deck snapshot kept")
            }
            Err(error) => tracing::warn!(%id, %error, "could not keep the deck history snapshot"),
        }
    }

    /// The snapshots of one deck, newest first. Empty without a history
    /// store.
    pub async fn history(&self, id: &str) -> anyhow::Result<Vec<Snapshot>> {
        match &self.history {
            Some(history) => history.list(id).await,
            None => Ok(Vec::new()),
        }
    }

    /// Writes the snapshot `stamp` back as the current deck. `save`
    /// keeps the current deck as a new snapshot first. `Ok(None)` means
    /// no snapshot with that stamp exists.
    pub async fn restore(&self, id: &str, stamp: &str) -> anyhow::Result<Option<Deck>> {
        let Some(history) = &self.history else {
            return Ok(None);
        };
        let Some(content) = history.read(id, stamp).await? else {
            return Ok(None);
        };
        let deck: Deck = serde_json::from_slice(&content)?;
        self.save(id, &deck).await?;
        Ok(Some(deck))
    }

    /// Moves a deck and its authorship sidecar to a new id. Returns
    /// false when no deck with the old id exists.
    pub async fn rename(&self, old_id: &str, new_id: &str) -> anyhow::Result<bool> {
        let Some(deck) = self.load(old_id).await? else {
            return Ok(false);
        };
        let user_paths = self.user_paths(old_id).await?;
        self.save(new_id, &deck).await?;
        self.save_user_paths(new_id, &user_paths).await?;
        self.delete(old_id).await?;
        if let Some(history) = &self.history
            && let Err(error) = history.rename(old_id, new_id).await
        {
            tracing::warn!(old_id, new_id, %error, "could not move the deck history");
        }
        Ok(true)
    }

    /// Deletes one deck and its authorship sidecar. Returns false when
    /// no file with that id exists.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.clear_user_paths(id).await?;
        match tokio::fs::remove_file(self.path_of(id)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

/// The listing row for one parsed deck.
fn summary_of(id: &str, deck: Deck) -> DeckSummary {
    DeckSummary {
        id: id.to_owned(),
        title: deck.title,
        theme: deck.theme.name,
        slide_count: deck.slides.len(),
        pending_count: deck
            .slides
            .iter()
            .filter(|slide| is_pending_slide(slide))
            .count(),
        outline_count: deck.outline.len(),
    }
}

/// The `/decks` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/decks", get(list_decks))
        .route(
            "/decks/{id}",
            get(get_deck).put(put_deck).delete(delete_deck),
        )
        .route("/decks/{id}/render", get(render_stored_deck))
        .route("/decks/{id}/authors", get(get_authors))
        .route("/decks/{id}/history", get(list_history))
        .route("/decks/{id}/history/{stamp}/restore", post(restore_history))
}

/// True for ids that are safe as file stems: the same rule as design
/// ids. `render` is reserved: `/decks/render` is the preview route.
pub(crate) fn is_valid_deck_id(id: &str) -> bool {
    is_valid_design_id(id)
}

/// Lists every stored deck.
async fn list_decks(State(store): State<DeckStore>) -> Response {
    match store.list().await {
        Ok(summaries) => Json(summaries).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Returns one stored deck as JSON.
async fn get_deck(State(store): State<DeckStore>, Path(id): Path<String>) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(deck)) => Json(deck).into_response(),
        Ok(None) => api_error::deck_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Saves a deck under the given id, or reports every validation error.
/// Saves carrying the `x-swift-design-author: user` header mark their
/// changed fields as user-authored; other saves count as agent writes.
async fn put_deck(
    State(store): State<DeckStore>,
    State(sessions): State<crate::sessions::SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(deck): Json<Deck>,
) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    let is_user = headers
        .get(provenance::AUTHOR_HEADER)
        .and_then(|value| value.to_str().ok())
        == Some("user");
    // Gating: decks are written only while the session is generating,
    // and the editor may also save while reviewing.
    if let Err(message) = crate::sessions::write_access(&sessions, &id, is_user).await {
        return api_error::error_response(StatusCode::CONFLICT, &message, Vec::new());
    }
    let errors = deck.validate();
    if !errors.is_empty() {
        return api_error::deck_validation_failed(&errors);
    }
    let previous = match store.load(&id).await {
        Ok(previous) => previous,
        Err(error) => return api_error::internal_error(&error),
    };
    if let Err(error) = store.save(&id, &deck).await {
        return api_error::internal_error(&error);
    }
    if let Err(error) = store
        .record_authors(&id, previous.as_ref(), &deck, is_user)
        .await
    {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!(%id, slide_count = deck.slides.len(), is_user, "deck saved");
    StatusCode::NO_CONTENT.into_response()
}

/// Lists the field paths the user changed in this deck.
async fn get_authors(State(store): State<DeckStore>, Path(id): Path<String>) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    match store.user_paths(&id).await {
        Ok(paths) => Json(serde_json::json!({ "user_paths": paths })).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Lists the saved snapshots of one deck, newest first.
async fn list_history(State(store): State<DeckStore>, Path(id): Path<String>) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return api_error::deck_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    }
    match store.history(&id).await {
        Ok(snapshots) => Json(snapshots).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Writes one snapshot back as the current deck. The deck as it is now
/// becomes the newest snapshot.
async fn restore_history(
    State(store): State<DeckStore>,
    State(sessions): State<crate::sessions::SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path((id, stamp)): Path<(String, String)>,
) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    if let Err(message) = crate::sessions::write_access(&sessions, &id, true).await {
        return api_error::error_response(StatusCode::CONFLICT, &message, Vec::new());
    }
    if !is_valid_stamp(&stamp) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid history stamp `{stamp}`: use a stamp from GET /decks/{id}/history"),
            Vec::new(),
        );
    }
    match store.load(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return api_error::deck_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    }
    match store.restore(&id, &stamp).await {
        Ok(Some(deck)) => {
            notifier.notify();
            tracing::info!(%id, %stamp, slide_count = deck.slides.len(), "deck restored");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("unknown history stamp `{stamp}`: run GET /decks/{id}/history for the list"),
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Deletes one stored deck.
async fn delete_deck(
    State(store): State<DeckStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    match store.delete(&id).await {
        Ok(true) => {
            notifier.notify();
            tracing::info!(%id, "deck deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => api_error::deck_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Query of `GET /decks/{id}/render`.
#[derive(Debug, Deserialize)]
struct RenderQuery {
    /// With `editable=true`, the page reports in-place edits to the
    /// parent window. Used by the editor preview.
    #[serde(default)]
    editable: bool,
    /// Render only this one-based slide. Used by thumbnails and the
    /// editor preview.
    #[serde(default)]
    slide: Option<usize>,
    /// With `audience=true`, the page follows the presenter view of the
    /// same deck instead of its own keyboard and wheel.
    #[serde(default)]
    audience: bool,
}

/// Renders one stored deck to HTML, or reports every validation error.
/// Agents that write deck files directly use this route to check them.
async fn render_stored_deck(
    State(store): State<DeckStore>,
    Path(id): Path<String>,
    Query(query): Query<RenderQuery>,
) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    let deck = match store.load(&id).await {
        Ok(Some(deck)) => deck,
        Ok(None) => return api_error::deck_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    let errors = deck.validate();
    if !errors.is_empty() {
        return api_error::deck_validation_failed(&errors);
    }
    let only_slide = match query.slide {
        Some(number) if number >= 1 && number <= deck.slides.len() => Some(number - 1),
        Some(number) => {
            return api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!(
                    "deck `{id}` has no slide {number}: use 1 to {}",
                    deck.slides.len()
                ),
                Vec::new(),
            );
        }
        None => None,
    };
    let options = deck_render::RenderOptions {
        is_editable: query.editable,
        only_slide,
        audience_channel: query.audience.then(|| presenter::channel_name(&id)),
        ..deck_render::RenderOptions::default()
    };
    Html(deck_render::render_deck_with(&deck, options)).into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::Deck;

    use crate::decks::{DeckStore, is_pending_slide, is_valid_deck_id};
    use crate::history::HistoryStore;

    fn sample_deck() -> Deck {
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json")).unwrap()
    }

    fn store_with_history(directory: &std::path::Path) -> DeckStore {
        DeckStore::new(directory.join("decks"))
            .with_history(HistoryStore::new(directory.join("deck-history")))
    }

    #[tokio::test]
    async fn the_first_deck_save_writes_no_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("deck", &sample_deck()).await.unwrap();
        assert!(store.history("deck").await.unwrap().is_empty());
        assert!(!directory.path().join("deck-history/deck").exists());
    }

    #[tokio::test]
    async fn a_deck_restore_returns_the_deck_to_the_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("deck", &sample_deck()).await.unwrap();
        let mut edited = sample_deck();
        edited.title = "Edited".to_owned();
        store.save("deck", &edited).await.unwrap();
        let stamp = store.history("deck").await.unwrap()[0].stamp.clone();
        let restored = store.restore("deck", &stamp).await.unwrap().unwrap();
        assert_eq!(restored.title, "Swift Design Deck Overview");
        assert_eq!(store.history("deck").await.unwrap().len(), 2);
        assert!(
            store
                .restore("deck", "2000-01-01T00-00-00Z")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_deck_rename_moves_the_history_directory() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("old", &sample_deck()).await.unwrap();
        store.save("old", &sample_deck()).await.unwrap();
        assert!(store.rename("old", "new").await.unwrap());
        assert!(!directory.path().join("deck-history/old").exists());
        assert_eq!(store.history("new").await.unwrap().len(), 1);
        assert!(store.load("old").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn deck_listing_skips_malformed_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = DeckStore::new(directory.path().to_path_buf());
        store.save("good", &sample_deck()).await.unwrap();
        std::fs::write(directory.path().join("broken.json"), "not json").unwrap();
        // A design file is not a deck: it is skipped too.
        std::fs::write(
            directory.path().join("design.json"),
            include_str!("../../../fixtures/sample-design.json"),
        )
        .unwrap();
        let summaries = store.list().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "good");
        assert_eq!(summaries[0].slide_count, 3);
        assert_eq!(summaries[0].pending_count, 0);
    }

    #[tokio::test]
    async fn user_authorship_is_recorded_per_slide_path() {
        let directory = tempfile::tempdir().unwrap();
        let store = DeckStore::new(directory.path().to_path_buf());
        let first = sample_deck();
        store.save("deck", &first).await.unwrap();
        store
            .record_authors("deck", None, &first, false)
            .await
            .unwrap();
        let mut second = first.clone();
        second.slides[0].html = "<h1>Mine</h1>".to_owned();
        store.save("deck", &second).await.unwrap();
        store
            .record_authors("deck", Some(&first), &second, true)
            .await
            .unwrap();
        let paths = store.user_paths("deck").await.unwrap();
        assert!(paths.contains("slides/0/html"));
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn deck_ids_follow_the_design_id_rule() {
        assert!(is_valid_deck_id("q3-review"));
        assert!(!is_valid_deck_id("render"));
        assert!(!is_valid_deck_id("Bad_Id"));
    }

    #[test]
    fn pending_slides_carry_the_placeholder_class() {
        let mut slide = sample_deck().slides.remove(0);
        assert!(!is_pending_slide(&slide));
        slide.html = "<div class='swift-design-pending'></div>".to_owned();
        assert!(is_pending_slide(&slide));
    }
}
