//! Design storage and the `/designs` CRUD routes.
//!
//! Designs live as `<id>.json` files in one directory. Agents may write
//! files into that directory directly; every request re-reads the
//! filesystem, so nothing goes stale. Every save first copies the
//! current file into the `HistoryStore`, and `/designs/{id}/history`
//! lists and restores those snapshots.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::{Design, Screen};
use serde::{Deserialize, Serialize};

use crate::events::ChangeNotifier;
use crate::history::{HistoryStore, Snapshot, is_valid_stamp};
use crate::{api_error, provenance, render};

/// The CSS class on a screen that holds the place of one the model has
/// not written yet.
///
/// A continuation saves these while its chunks run, so the canvas shows
/// the gaps fill in. A finished design has none: the run replaces them,
/// and `continue_design` drops any that a stopped run left behind.
pub const PENDING_SCREEN_CLASS: &str = "swift-design-pending";

/// True when a screen only holds the place of one still to be written.
pub fn is_pending_screen(screen: &Screen) -> bool {
    screen.html.contains(PENDING_SCREEN_CLASS)
}

/// Filesystem-backed design storage: one `<id>.json` file per design.
#[derive(Clone)]
pub struct DesignStore {
    directory: PathBuf,
    /// Where each save keeps the previous file. `None` keeps no history.
    history: Option<HistoryStore>,
    /// Ids of malformed files already reported, so the listing warns
    /// once per file and not on every request.
    reported_malformed: Arc<Mutex<HashSet<String>>>,
}

/// One row in the `GET /designs` listing.
#[derive(Debug, Serialize)]
pub struct DesignSummary {
    /// File stem of the design file, used in `/designs/{id}` routes.
    pub id: String,
    /// Design title.
    pub title: String,
    /// Theme name, shown next to the id on chooser cards.
    pub theme: String,
    /// The px canvas of every screen, for preview frames.
    pub viewport: design_model::Viewport,
    /// Number of screens.
    pub screen_count: usize,
    /// Number of titles in the planned outline. More than `screen_count`
    /// for a preview design; 0 when the design has no outline.
    pub outline_count: usize,
    /// Number of placeholder screens a running or stopped continuation
    /// left in the design. 0 for a finished design.
    pub pending_count: usize,
}

impl DesignStore {
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

    /// Logs a malformed design file the first time it is seen.
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
                "skipping malformed design file: it does not match the current design schema; delete or regenerate it"
            );
        }
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn authors_path_of(&self, id: &str) -> PathBuf {
        self.directory.join(".authors").join(format!("{id}.json"))
    }

    /// Field paths the user changed in this design. Missing sidecar files
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
    /// candidate replaces a design wholesale.
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
        previous: Option<&Design>,
        current: &Design,
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

    /// Lists every parseable design, sorted by id. Malformed files are
    /// logged and skipped so one bad file cannot break the listing.
    pub async fn list(&self) -> anyhow::Result<Vec<DesignSummary>> {
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
            match serde_json::from_str::<Design>(&raw) {
                Ok(design) => summaries.push(DesignSummary {
                    id: id.to_owned(),
                    title: design.title,
                    theme: design.theme.name,
                    viewport: design.viewport,
                    screen_count: design.screens.len(),
                    pending_count: design
                        .screens
                        .iter()
                        .filter(|screen| is_pending_screen(screen))
                        .count(),
                    outline_count: design.outline.len(),
                }),
                Err(error) => {
                    self.report_malformed(id, &error);
                }
            }
        }
        summaries.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(summaries)
    }

    /// Loads one design. `Ok(None)` means no file with that id exists.
    pub async fn load(&self, id: &str) -> anyhow::Result<Option<Design>> {
        match tokio::fs::read_to_string(self.path_of(id)).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes one design as pretty-printed JSON, creating the directory
    /// when needed. The previous file goes to the history store first.
    pub async fn save(&self, id: &str, design: &Design) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.directory).await?;
        self.snapshot_current(id).await;
        let json = serde_json::to_string_pretty(design)?;
        tokio::fs::write(self.path_of(id), json + "\n").await?;
        Ok(())
    }

    /// Copies the current design file into the history store. A design with
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
                tracing::warn!(%id, %error, "could not read the design file for its history snapshot");
                return;
            }
        };
        match history.snapshot(id, &content).await {
            Ok(stamp) => {
                tracing::debug!(%id, %stamp, size_bytes = content.len(), "design snapshot kept")
            }
            Err(error) => tracing::warn!(%id, %error, "could not keep the design history snapshot"),
        }
    }

    /// The snapshots of one design, newest first. Empty without a history
    /// store.
    pub async fn history(&self, id: &str) -> anyhow::Result<Vec<Snapshot>> {
        match &self.history {
            Some(history) => history.list(id).await,
            None => Ok(Vec::new()),
        }
    }

    /// Writes the snapshot `stamp` back as the current design. `save`
    /// keeps the current design as a new snapshot first. `Ok(None)` means
    /// no snapshot with that stamp exists.
    pub async fn restore(&self, id: &str, stamp: &str) -> anyhow::Result<Option<Design>> {
        let Some(history) = &self.history else {
            return Ok(None);
        };
        let Some(content) = history.read(id, stamp).await? else {
            return Ok(None);
        };
        let design: Design = serde_json::from_slice(&content)?;
        self.save(id, &design).await?;
        Ok(Some(design))
    }

    /// Deletes one design and its authorship sidecar. Returns false when
    /// Moves a design and its authorship sidecar to a new id. Returns
    /// false when no design with the old id exists.
    pub async fn rename(&self, old_id: &str, new_id: &str) -> anyhow::Result<bool> {
        let Some(design) = self.load(old_id).await? else {
            return Ok(false);
        };
        let user_paths = self.user_paths(old_id).await?;
        self.save(new_id, &design).await?;
        self.save_user_paths(new_id, &user_paths).await?;
        self.delete(old_id).await?;
        if let Some(history) = &self.history
            && let Err(error) = history.rename(old_id, new_id).await
        {
            tracing::warn!(old_id, new_id, %error, "could not move the design history");
        }
        Ok(true)
    }

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

/// The `/designs` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/designs", get(list_designs))
        .route(
            "/designs/{id}",
            get(get_design).put(put_design).delete(delete_design),
        )
        .route("/designs/{id}/render", get(render_stored_design))
        .route("/designs/{id}/authors", get(get_authors))
        .route("/designs/{id}/history", get(list_history))
        .route(
            "/designs/{id}/history/{stamp}/restore",
            post(restore_history),
        )
}

/// True for ids that are safe as file stems. The character allowlist
/// blocks path traversal. `render` is reserved: `/designs/render` is the
/// preview route.
pub(crate) fn is_valid_design_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "render"
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// Lists every stored design.
async fn list_designs(State(store): State<DesignStore>) -> Response {
    match store.list().await {
        Ok(summaries) => Json(summaries).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Returns one stored design as JSON.
async fn get_design(State(store): State<DesignStore>, Path(id): Path<String>) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(design)) => Json(design).into_response(),
        Ok(None) => api_error::design_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Saves a design under the given id, or reports every validation error.
/// Saves carrying the `x-swift-design-author: user` header mark their
/// changed fields as user-authored; other saves count as agent writes.
async fn put_design(
    State(store): State<DesignStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(design): Json<Design>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    let errors = design.validate();
    if !errors.is_empty() {
        return api_error::validation_failed(&errors);
    }
    let is_user = headers
        .get(provenance::AUTHOR_HEADER)
        .and_then(|value| value.to_str().ok())
        == Some("user");
    let previous = match store.load(&id).await {
        Ok(previous) => previous,
        Err(error) => return api_error::internal_error(&error),
    };
    if let Err(error) = store.save(&id, &design).await {
        return api_error::internal_error(&error);
    }
    if let Err(error) = store
        .record_authors(&id, previous.as_ref(), &design, is_user)
        .await
    {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!(%id, screen_count = design.screens.len(), is_user, "design saved");
    StatusCode::NO_CONTENT.into_response()
}

/// Lists the field paths the user changed in this design.
async fn get_authors(State(store): State<DesignStore>, Path(id): Path<String>) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    match store.user_paths(&id).await {
        Ok(paths) => Json(serde_json::json!({ "user_paths": paths })).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Lists the saved snapshots of one design, newest first.
async fn list_history(State(store): State<DesignStore>, Path(id): Path<String>) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return api_error::design_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    }
    match store.history(&id).await {
        Ok(snapshots) => Json(snapshots).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Writes one snapshot back as the current design. The design as it is now
/// becomes the newest snapshot.
async fn restore_history(
    State(store): State<DesignStore>,
    State(notifier): State<ChangeNotifier>,
    Path((id, stamp)): Path<(String, String)>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    if !is_valid_stamp(&stamp) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid history stamp `{stamp}`: use a stamp from GET /designs/{id}/history"),
            Vec::new(),
        );
    }
    match store.load(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return api_error::design_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    }
    match store.restore(&id, &stamp).await {
        Ok(Some(design)) => {
            notifier.notify();
            tracing::info!(%id, %stamp, screen_count = design.screens.len(), "design restored");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("unknown history stamp `{stamp}`: run GET /designs/{id}/history for the list"),
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Deletes one stored design.
async fn delete_design(
    State(store): State<DesignStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    match store.delete(&id).await {
        Ok(true) => {
            notifier.notify();
            tracing::info!(%id, "design deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => api_error::design_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Query of `GET /designs/{id}/render`.
#[derive(Debug, Deserialize)]
struct RenderQuery {
    /// With `editable=true`, the page reports in-place edits to the
    /// parent window. Used by the editor preview.
    #[serde(default)]
    editable: bool,
    /// Render only this one-based screen. Used by thumbnails and the
    /// editor preview.
    #[serde(default)]
    screen: Option<usize>,
}

/// Renders one stored design to HTML, or reports every validation error.
/// Agents that write design files directly use this route to check them.
async fn render_stored_design(
    State(store): State<DesignStore>,
    Path(id): Path<String>,
    Query(query): Query<RenderQuery>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(design)) => {
            let errors = design.validate();
            if !errors.is_empty() {
                return api_error::validation_failed(&errors);
            }
            let only_screen = match query.screen {
                Some(number) if number >= 1 && number <= design.screens.len() => Some(number - 1),
                Some(number) => {
                    return api_error::error_response(
                        StatusCode::NOT_FOUND,
                        &format!(
                            "design `{id}` has no screen {number}: use 1 to {}",
                            design.screens.len()
                        ),
                        Vec::new(),
                    );
                }
                None => None,
            };
            let options = render::RenderOptions {
                is_editable: query.editable,
                only_screen,
                ..render::RenderOptions::default()
            };
            Html(render::render_design_with(&design, options)).into_response()
        }
        Ok(None) => api_error::design_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::Design;

    use crate::designs::{DesignStore, is_valid_design_id};
    use crate::history::HistoryStore;

    fn sample_design() -> Design {
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap()
    }

    fn store_with_history(directory: &std::path::Path) -> DesignStore {
        DesignStore::new(directory.join("designs"))
            .with_history(HistoryStore::new(directory.join("history")))
    }

    #[tokio::test]
    async fn the_first_save_writes_no_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("design", &sample_design()).await.unwrap();
        assert!(store.history("design").await.unwrap().is_empty());
        assert!(!directory.path().join("history/design").exists());
    }

    #[tokio::test]
    async fn a_save_writes_a_snapshot_of_the_previous_content() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        let first = sample_design();
        store.save("design", &first).await.unwrap();
        let mut second = sample_design();
        second.title = "Second".to_owned();
        store.save("design", &second).await.unwrap();
        let rows = store.history("design").await.unwrap();
        assert_eq!(rows.len(), 1);
        let snapshot = std::fs::read_to_string(
            directory
                .path()
                .join(format!("history/design/{}.json", rows[0].stamp)),
        )
        .unwrap();
        let kept: Design = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(kept.title, first.title);
        assert_eq!(rows[0].size_bytes as usize, snapshot.len());
    }

    #[tokio::test]
    async fn a_restore_returns_the_design_to_the_snapshot_and_keeps_the_current_one() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("design", &sample_design()).await.unwrap();
        let mut edited = sample_design();
        edited.title = "Edited".to_owned();
        store.save("design", &edited).await.unwrap();
        let stamp = store.history("design").await.unwrap()[0].stamp.clone();
        let restored = store.restore("design", &stamp).await.unwrap().unwrap();
        assert_eq!(restored.title, "Swift Design Overview");
        assert_eq!(
            store.load("design").await.unwrap().unwrap().title,
            "Swift Design Overview"
        );
        let rows = store.history("design").await.unwrap();
        assert_eq!(rows.len(), 2);
        let newest = std::fs::read_to_string(
            directory
                .path()
                .join(format!("history/design/{}.json", rows[0].stamp)),
        )
        .unwrap();
        assert!(newest.contains("\"Edited\""));
        assert!(
            store
                .restore("design", "2000-01-01T00-00-00Z")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_rename_moves_the_history_directory() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("old", &sample_design()).await.unwrap();
        store.save("old", &sample_design()).await.unwrap();
        assert!(store.rename("old", "new").await.unwrap());
        assert!(!directory.path().join("history/old").exists());
        assert_eq!(store.history("new").await.unwrap().len(), 1);
        assert!(store.history("old").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_store_without_history_saves_and_restores_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let store = DesignStore::new(directory.path().to_path_buf());
        store.save("design", &sample_design()).await.unwrap();
        store.save("design", &sample_design()).await.unwrap();
        assert!(store.history("design").await.unwrap().is_empty());
        assert!(
            store
                .restore("design", "2000-01-01T00-00-00Z")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn accepts_kebab_case_ids_and_rejects_everything_else() {
        assert!(is_valid_design_id("overview"));
        assert!(is_valid_design_id("q3-sales-2026"));
        assert!(!is_valid_design_id(""));
        assert!(!is_valid_design_id("render"));
        assert!(!is_valid_design_id("Bad_Id"));
        assert!(!is_valid_design_id("../escape"));
        assert!(!is_valid_design_id(&"a".repeat(65)));
    }

    #[tokio::test]
    async fn listing_skips_malformed_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = DesignStore::new(directory.path().to_path_buf());
        let design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        store.save("good", &design).await.unwrap();
        std::fs::write(directory.path().join("broken.json"), "not json").unwrap();
        let summaries = store.list().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "good");
        assert_eq!(summaries[0].screen_count, 3);
    }

    #[tokio::test]
    async fn listing_an_absent_directory_returns_empty() {
        let store = DesignStore::new("/nonexistent/swift-design-test".into());
        assert!(store.list().await.unwrap().is_empty());
    }
}
