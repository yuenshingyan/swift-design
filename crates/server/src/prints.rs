//! Print storage and the `/prints` CRUD routes.
//!
//! Prints live as `<id>.json` files in one directory, next to but
//! apart from the designs, the decks, the documents, and the socials.
//! Agents may write files into that directory directly; every request
//! re-reads the filesystem, so nothing goes stale. Every save first
//! copies the current file into the `HistoryStore`, and
//! `/prints/{id}/history` lists and restores those snapshots. This
//! module is the print twin of `socials.rs`.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::{Orientation, Print, PrintSize, Sheet};
use serde::{Deserialize, Serialize};

use crate::designs::{PENDING_SCREEN_CLASS, is_valid_design_id};
use crate::events::ChangeNotifier;
use crate::history::{HistoryStore, Snapshot, is_valid_stamp};
use crate::{api_error, print_render, provenance};

/// The CSS class on a sheet that holds the place of one the model has
/// not written yet. The same marker as for screens.
pub const PENDING_SHEET_CLASS: &str = PENDING_SCREEN_CLASS;

/// True when a sheet only holds the place of one still to be written.
pub fn is_pending_sheet(sheet: &Sheet) -> bool {
    sheet.html.contains(PENDING_SHEET_CLASS)
}

/// Filesystem-backed print storage: one `<id>.json` file per print.
#[derive(Clone)]
pub struct PrintStore {
    directory: PathBuf,
    /// Where each save keeps the previous file. `None` keeps no history.
    history: Option<HistoryStore>,
    /// Ids of malformed files already reported, so the listing warns
    /// once per file and not on every request.
    reported_malformed: Arc<Mutex<HashSet<String>>>,
}

/// One row in the `GET /prints` listing.
#[derive(Clone, Debug, Serialize)]
pub struct PrintSummary {
    /// File stem of the print file, used in `/prints/{id}` routes.
    pub id: String,
    /// Print title.
    pub title: String,
    /// Theme name, shown next to the id on chooser cards.
    pub theme: String,
    /// The paper size the sheets are laid out on.
    pub size: PrintSize,
    /// How the sheets are turned.
    pub orientation: Orientation,
    /// Number of sheets.
    pub sheet_count: usize,
    /// Number of titles in the planned outline. More than `sheet_count`
    /// for a preview print; 0 when the print has no outline.
    pub outline_count: usize,
    /// Number of placeholder sheets a running or stopped continuation
    /// left in the print. 0 for a finished print.
    pub pending_count: usize,
}

impl PrintStore {
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

    /// Logs a malformed print file the first time it is seen.
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
                "skipping malformed print file: it does not match the current print schema; delete or regenerate it"
            );
        }
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    fn authors_path_of(&self, id: &str) -> PathBuf {
        self.directory.join(".authors").join(format!("{id}.json"))
    }

    /// Field paths the user changed in this print. Missing sidecar
    /// files mean everything is agent-authored.
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
    /// candidate replaces a print wholesale.
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
        previous: Option<&Print>,
        current: &Print,
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

    /// Lists every parseable print, sorted by id. Malformed files are
    /// logged and skipped so one bad file cannot break the listing.
    pub async fn list(&self) -> anyhow::Result<Vec<PrintSummary>> {
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
            match serde_json::from_str::<Print>(&raw) {
                Ok(print) => summaries.push(summary_of(id, print)),
                Err(error) => self.report_malformed(id, &error),
            }
        }
        summaries.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(summaries)
    }

    /// Loads one print. `Ok(None)` means no file with that id exists.
    pub async fn load(&self, id: &str) -> anyhow::Result<Option<Print>> {
        match tokio::fs::read_to_string(self.path_of(id)).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes one print as pretty-printed JSON, creating the
    /// directory when needed. The previous file goes to the history
    /// store first.
    pub async fn save(&self, id: &str, print: &Print) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.directory).await?;
        self.snapshot_current(id).await;
        let json = serde_json::to_string_pretty(print)?;
        crate::files::write_atomically(&self.path_of(id), &(json + "\n")).await?;
        Ok(())
    }

    /// Copies the current print file into the history store. A print
    /// with no file yet has nothing to keep. A history failure is
    /// logged and does not stop the save.
    async fn snapshot_current(&self, id: &str) {
        let Some(history) = &self.history else {
            return;
        };
        let content = match tokio::fs::read(self.path_of(id)).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(%id, %error, "could not read the print file for its history snapshot");
                return;
            }
        };
        match history.snapshot(id, &content).await {
            Ok(stamp) => {
                tracing::debug!(%id, %stamp, size_bytes = content.len(), "print snapshot kept")
            }
            Err(error) => {
                tracing::warn!(%id, %error, "could not keep the print history snapshot")
            }
        }
    }

    /// The snapshots of one print, newest first. Empty without a
    /// history store.
    pub async fn history(&self, id: &str) -> anyhow::Result<Vec<Snapshot>> {
        match &self.history {
            Some(history) => history.list(id).await,
            None => Ok(Vec::new()),
        }
    }

    /// Writes the snapshot `stamp` back as the current print. `save`
    /// keeps the current print as a new snapshot first. `Ok(None)`
    /// means no snapshot with that stamp exists.
    pub async fn restore(&self, id: &str, stamp: &str) -> anyhow::Result<Option<Print>> {
        let Some(history) = &self.history else {
            return Ok(None);
        };
        let Some(content) = history.read(id, stamp).await? else {
            return Ok(None);
        };
        let print: Print = serde_json::from_slice(&content)?;
        self.save(id, &print).await?;
        Ok(Some(print))
    }

    /// Moves a print and its authorship sidecar to a new id. Returns
    /// false when no print with the old id exists.
    pub async fn rename(&self, old_id: &str, new_id: &str) -> anyhow::Result<bool> {
        let Some(print) = self.load(old_id).await? else {
            return Ok(false);
        };
        let user_paths = self.user_paths(old_id).await?;
        self.save(new_id, &print).await?;
        self.save_user_paths(new_id, &user_paths).await?;
        // The history moves before the delete, because a delete now
        // drops the history of the id it deletes.
        if let Some(history) = &self.history
            && let Err(error) = history.rename(old_id, new_id).await
        {
            tracing::warn!(old_id, new_id, %error, "could not move the print history");
        }
        self.delete(old_id).await?;
        Ok(true)
    }

    /// Deletes one print, its authorship sidecar, and its history.
    /// Returns false when no file with that id exists.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.clear_user_paths(id).await?;
        // The history goes with the file. A later print with the same
        // id is a different print and must start with no snapshots.
        if let Some(history) = &self.history
            && let Err(error) = history.delete(id).await
        {
            tracing::warn!(%id, %error, "could not delete the print history");
        }
        match tokio::fs::remove_file(self.path_of(id)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Every stored print id, malformed files included. A session
    /// delete must remove those too, so this reads the directory
    /// instead of the listing.
    async fn stored_ids(&self) -> anyhow::Result<Vec<String>> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut ids = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
                ids.push(id.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Deletes every print of one session: the session's own print and
    /// its candidates, with their sidecars and history. Returns how
    /// many files went.
    pub async fn delete_session(&self, session_id: &str) -> anyhow::Result<usize> {
        let mut deleted = 0;
        for id in self.stored_ids().await? {
            if crate::sessions::session_id_of_artifact(&id) != session_id {
                continue;
            }
            if self.delete(&id).await? {
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

/// The listing row for one parsed print.
fn summary_of(id: &str, print: Print) -> PrintSummary {
    PrintSummary {
        id: id.to_owned(),
        title: print.title,
        theme: print.theme.name,
        size: print.size,
        orientation: print.orientation,
        sheet_count: print.sheets.len(),
        pending_count: print
            .sheets
            .iter()
            .filter(|sheet| is_pending_sheet(sheet))
            .count(),
        outline_count: print.outline.len(),
    }
}

/// The `/prints` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/prints", get(list_prints))
        .route(
            "/prints/{id}",
            get(get_print).put(put_print).delete(delete_print),
        )
        .route("/prints/{id}/fork", post(fork_print))
        .route("/prints/{id}/render", get(render_stored_print))
        .route("/prints/{id}/authors", get(get_authors))
        .route("/prints/{id}/history", get(list_history))
        .route(
            "/prints/{id}/history/{stamp}/restore",
            post(restore_history),
        )
}

/// True for ids that are safe as file stems: the same rule as design
/// ids. `render` is reserved: `/prints/render` is the preview route.
pub(crate) fn is_valid_print_id(id: &str) -> bool {
    is_valid_design_id(id)
}

/// Lists every stored print.
async fn list_prints(State(store): State<PrintStore>) -> Response {
    match store.list().await {
        Ok(summaries) => Json(summaries).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Returns one stored print as JSON.
async fn get_print(State(store): State<PrintStore>, Path(id): Path<String>) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(print)) => Json(print).into_response(),
        Ok(None) => api_error::print_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Saves a print under the given id, or reports every validation
/// error. Saves carrying the `x-swift-design-author: user` header mark
/// their changed fields as user-authored; other saves count as agent
/// writes.
async fn put_print(
    State(store): State<PrintStore>,
    State(sessions): State<crate::sessions::SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(print): Json<Print>,
) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    let is_user = headers
        .get(provenance::AUTHOR_HEADER)
        .and_then(|value| value.to_str().ok())
        == Some("user");
    // Gating: prints are written only while the session is generating,
    // and the editor may also save while reviewing.
    if let Err(message) = crate::sessions::write_access(&sessions, &id, is_user).await {
        return api_error::error_response(StatusCode::CONFLICT, &message, Vec::new());
    }
    let errors = print.validate();
    if !errors.is_empty() {
        return api_error::print_validation_failed(&errors);
    }
    let previous = match store.load(&id).await {
        Ok(previous) => previous,
        Err(error) => return api_error::internal_error(&error),
    };
    if let Err(error) = store.save(&id, &print).await {
        return api_error::internal_error(&error);
    }
    if let Err(error) = store
        .record_authors(&id, previous.as_ref(), &print, is_user)
        .await
    {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!(%id, sheet_count = print.sheets.len(), is_user, "print saved");
    StatusCode::NO_CONTENT.into_response()
}

/// Lists the field paths the user changed in this print.
async fn get_authors(State(store): State<PrintStore>, Path(id): Path<String>) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    match store.user_paths(&id).await {
        Ok(paths) => Json(serde_json::json!({ "user_paths": paths })).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Lists the saved snapshots of one print, newest first.
async fn list_history(State(store): State<PrintStore>, Path(id): Path<String>) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return api_error::print_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    }
    match store.history(&id).await {
        Ok(snapshots) => Json(snapshots).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Writes one snapshot back as the current print. The print as it is
/// now becomes the newest snapshot.
async fn restore_history(
    State(store): State<PrintStore>,
    State(sessions): State<crate::sessions::SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path((id, stamp)): Path<(String, String)>,
) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    if let Err(message) = crate::sessions::write_access(&sessions, &id, true).await {
        return api_error::error_response(StatusCode::CONFLICT, &message, Vec::new());
    }
    if !is_valid_stamp(&stamp) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid history stamp `{stamp}`: use a stamp from GET /prints/{id}/history"),
            Vec::new(),
        );
    }
    match store.load(&id).await {
        Ok(Some(_)) => {}
        Ok(None) => return api_error::print_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    }
    match store.restore(&id, &stamp).await {
        Ok(Some(print)) => {
            notifier.notify();
            tracing::info!(%id, %stamp, sheet_count = print.sheets.len(), "print restored");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("unknown history stamp `{stamp}`: run GET /prints/{id}/history for the list"),
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Deletes one stored print.
async fn delete_print(
    State(store): State<PrintStore>,
    State(sessions): State<crate::sessions::SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    match store.delete(&id).await {
        Ok(true) => {
            sessions.forget_artifact(&id).await;
            notifier.notify();
            tracing::info!(%id, "print deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => api_error::print_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Copies one candidate to the next free number of its session, as a
/// new candidate, and answers its id. Refused while the session's run
/// writes, so the number cannot race the run.
async fn fork_print(
    State(store): State<PrintStore>,
    State(sessions): State<crate::sessions::SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    let base = crate::sessions::session_id_of_artifact(&id).to_owned();
    if let Ok(Some(session)) = sessions.read(&base).await
        && session.state == design_model::WorkflowState::Generating
    {
        return api_error::error_response(
            StatusCode::CONFLICT,
            &format!("session `{base}` is generating: fork when the run ends"),
            Vec::new(),
        );
    }
    let rows = match store.list().await {
        Ok(rows) => rows,
        Err(error) => return api_error::internal_error(&error),
    };
    let number =
        crate::candidates::next_candidate_number(&base, rows.iter().map(|row| row.id.as_str()));
    let new_id = crate::candidates::candidate_id(&base, number);
    if let Err(response) = crate::candidates::copy_print(&store, &id, &new_id).await {
        return response;
    }
    // The session's updated_at moves, so the listing sorts it first.
    let _ = sessions.update(&base, |_| {}).await;
    notifier.notify();
    tracing::info!(session_id = %base, from = %id, to = %new_id, "candidate forked");
    Json(serde_json::json!({ "id": new_id })).into_response()
}

/// Query of `GET /prints/{id}/render`.
#[derive(Debug, Deserialize)]
struct RenderQuery {
    /// With `editable=true`, the sheet reports in-place edits to the
    /// parent window. Used by the editor preview.
    #[serde(default)]
    editable: bool,
    /// Render only this one-based sheet. Used by thumbnails and the
    /// editor preview.
    #[serde(default)]
    sheet: Option<usize>,
}

/// Renders one stored print to HTML, or reports every validation
/// error. Agents that write print files directly use this route to
/// check them.
async fn render_stored_print(
    State(store): State<PrintStore>,
    Path(id): Path<String>,
    Query(query): Query<RenderQuery>,
) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    let print = match store.load(&id).await {
        Ok(Some(print)) => print,
        Ok(None) => return api_error::print_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    let errors = print.validate();
    if !errors.is_empty() {
        return api_error::print_validation_failed(&errors);
    }
    let only_sheet = match query.sheet {
        Some(number) if number >= 1 && number <= print.sheets.len() => Some(number - 1),
        Some(number) => {
            return api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!(
                    "print `{id}` has no sheet {number}: use 1 to {}",
                    print.sheets.len()
                ),
                Vec::new(),
            );
        }
        None => None,
    };
    let options = print_render::RenderOptions {
        is_editable: query.editable,
        only_sheet,
        ..print_render::RenderOptions::default()
    };
    Html(print_render::render_print_with(&print, options)).into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::Print;

    use crate::history::HistoryStore;
    use crate::prints::{PrintStore, is_pending_sheet, is_valid_print_id};

    fn sample_print() -> Print {
        serde_json::from_str(include_str!("../../../fixtures/sample-print.json")).unwrap()
    }

    fn store_with_history(directory: &std::path::Path) -> PrintStore {
        PrintStore::new(directory.join("prints"))
            .with_history(HistoryStore::new(directory.join("print-history")))
    }

    #[tokio::test]
    async fn the_first_print_save_writes_no_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("report", &sample_print()).await.unwrap();
        assert!(store.history("report").await.unwrap().is_empty());
        assert!(!directory.path().join("print-history/report").exists());
    }

    #[tokio::test]
    async fn a_print_restore_returns_the_print_to_the_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("report", &sample_print()).await.unwrap();
        let mut edited = sample_print();
        edited.title = "Edited".to_owned();
        store.save("report", &edited).await.unwrap();
        let stamp = store.history("report").await.unwrap()[0].stamp.clone();
        let restored = store.restore("report", &stamp).await.unwrap().unwrap();
        assert_eq!(restored.title, "Swift Design launch flyer");
        assert_eq!(store.history("report").await.unwrap().len(), 2);
        assert!(
            store
                .restore("report", "2000-01-01T00-00-00Z")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_print_delete_takes_the_history_with_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("report", &sample_print()).await.unwrap();
        store.save("report", &sample_print()).await.unwrap();
        assert_eq!(store.history("report").await.unwrap().len(), 1);
        assert!(store.delete("report").await.unwrap());
        assert!(store.history("report").await.unwrap().is_empty());
        assert!(!directory.path().join("print-history/report").exists());
    }

    #[tokio::test]
    async fn deleting_a_session_takes_its_print_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        for id in ["memo-candidate-1", "memo-candidate-2", "memorial"] {
            store.save(id, &sample_print()).await.unwrap();
            store.save(id, &sample_print()).await.unwrap();
        }
        assert_eq!(store.delete_session("memo").await.unwrap(), 2);
        let left: Vec<String> = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|summary| summary.id)
            .collect();
        assert_eq!(left, ["memorial"]);
        assert!(
            !directory
                .path()
                .join("print-history/memo-candidate-2")
                .exists()
        );
        assert!(directory.path().join("print-history/memorial").exists());
    }

    #[tokio::test]
    async fn a_print_rename_moves_the_history_directory() {
        let directory = tempfile::tempdir().unwrap();
        let store = store_with_history(directory.path());
        store.save("old", &sample_print()).await.unwrap();
        store.save("old", &sample_print()).await.unwrap();
        assert!(store.rename("old", "new").await.unwrap());
        assert!(!directory.path().join("print-history/old").exists());
        assert_eq!(store.history("new").await.unwrap().len(), 1);
        assert!(store.load("old").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn print_listing_skips_malformed_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = PrintStore::new(directory.path().to_path_buf());
        store.save("good", &sample_print()).await.unwrap();
        std::fs::write(directory.path().join("broken.json"), "not json").unwrap();
        // A deck file is not a print: it is skipped too.
        std::fs::write(
            directory.path().join("deck.json"),
            include_str!("../../../fixtures/sample-deck.json"),
        )
        .unwrap();
        let summaries = store.list().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "good");
        assert_eq!(summaries[0].sheet_count, 2);
        assert_eq!(summaries[0].pending_count, 0);
        assert_eq!(summaries[0].size, design_model::PrintSize::A4);
        assert_eq!(
            summaries[0].orientation,
            design_model::Orientation::Portrait
        );
    }

    #[tokio::test]
    async fn user_authorship_is_recorded_per_sheet_path() {
        let directory = tempfile::tempdir().unwrap();
        let store = PrintStore::new(directory.path().to_path_buf());
        let first = sample_print();
        store.save("report", &first).await.unwrap();
        store
            .record_authors("report", None, &first, false)
            .await
            .unwrap();
        let mut second = first.clone();
        second.sheets[0].html = "<h1>Mine</h1>".to_owned();
        store.save("report", &second).await.unwrap();
        store
            .record_authors("report", Some(&first), &second, true)
            .await
            .unwrap();
        let paths = store.user_paths("report").await.unwrap();
        assert!(paths.contains("sheets/0/html"));
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn print_ids_follow_the_design_id_rule() {
        assert!(is_valid_print_id("q3-report"));
        assert!(!is_valid_print_id("render"));
        assert!(!is_valid_print_id("Bad_Id"));
    }

    #[test]
    fn pending_sheets_carry_the_placeholder_class() {
        let mut sheet = sample_print().sheets.remove(0);
        assert!(!is_pending_sheet(&sheet));
        sheet.html = "<div class='swift-design-pending'></div>".to_owned();
        assert!(is_pending_sheet(&sheet));
    }
}
