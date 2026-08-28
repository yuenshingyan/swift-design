//! Source-material uploads: files agents turn into design content.
//!
//! Files live in one directory with sanitized kebab-case names. Screens
//! reference them as `/uploads/{name}`, so `full_image` screens can use
//! uploaded images directly. The path stays flat because stored designs,
//! the agent instructions, and the export inliner all name a file that
//! way.
//!
//! Each file belongs to one scope: the session it was attached to, or
//! `_draft` for a file attached on the landing page before a session
//! exists. `owners.json` next to the files records that. A run reads
//! only its own session's files, so a file attached to one project never
//! reaches another project's prompt.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api_error;
use crate::events::ChangeNotifier;

/// Uploads above this size are rejected. Big enough for screen images
/// and PDF source material.
const UPLOAD_BODY_LIMIT_BYTES: usize = 50 * 1024 * 1024;

/// The scope of a file attached before its session exists. The landing
/// page uploads here, and creating a session adopts the lot.
pub const DRAFT_SCOPE: &str = "_draft";

/// The file that records which scope owns each upload.
const OWNERS_FILE: &str = "owners.json";

/// Filesystem-backed upload storage: one sanitized file name per upload,
/// plus the scope that owns it.
#[derive(Clone)]
pub struct UploadStore {
    directory: PathBuf,
    /// Serializes the read-modify-write of `owners.json`.
    owners_lock: Arc<tokio::sync::Mutex<()>>,
}

/// One row in the `GET /uploads` listing.
#[derive(Debug, Serialize)]
pub struct UploadSummary {
    /// Sanitized file name, used in `/uploads/{name}`.
    pub name: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Content type from the extension, like `image/png`.
    pub content_type: &'static str,
    /// True for image content types: screens can use the file as
    /// `<img src='/uploads/{name}'>`.
    pub is_image: bool,
}

impl UploadStore {
    /// Creates a store over `directory`. The directory may not exist yet;
    /// it is created on the first save.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            owners_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// The scope of every owned file. A file with no entry belongs to no
    /// scope: it was stored before the scopes existed, or written by an
    /// agent straight into the directory.
    async fn owners(&self) -> HashMap<String, String> {
        let path = self.directory.join(OWNERS_FILE);
        let Ok(text) = tokio::fs::read_to_string(&path).await else {
            return HashMap::new();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    /// Writes the owner table. Atomic, because the studio lists the
    /// uploads while a run writes them.
    async fn write_owners(&self, owners: &HashMap<String, String>) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.directory).await?;
        let text = serde_json::to_string_pretty(owners)?;
        crate::files::write_atomically(&self.directory.join(OWNERS_FILE), &text).await
    }

    /// Saves `bytes` for `scope` under a sanitized version of
    /// `file_name` and returns the stored name. Returns `None` when no
    /// safe name is left after sanitizing.
    ///
    /// A name another scope already holds gets a number, so one
    /// project's `readme.md` never overwrites another's.
    pub async fn save(
        &self,
        scope: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> anyhow::Result<Option<String>> {
        let Some(wanted) = sanitize_file_name(file_name) else {
            return Ok(None);
        };
        let _guard = self.owners_lock.lock().await;
        let mut owners = self.owners().await;
        let name = free_name(&wanted, scope, &owners);
        tokio::fs::create_dir_all(&self.directory).await?;
        tokio::fs::write(self.directory.join(&name), bytes).await?;
        owners.insert(name.clone(), scope.to_owned());
        self.write_owners(&owners).await?;
        Ok(Some(name))
    }

    /// Gives `scope` every draft and unowned file. Returns how many
    /// moved.
    ///
    /// The landing page attaches files before the session exists, so
    /// creating the session takes what the composer showed.
    pub async fn adopt(&self, scope: &str) -> anyhow::Result<usize> {
        let _guard = self.owners_lock.lock().await;
        let mut owners = self.owners().await;
        let mut moved = 0;
        for name in self.stored_names().await {
            let owner = owners.get(&name).map(String::as_str);
            if owner.is_none() || owner == Some(DRAFT_SCOPE) {
                owners.insert(name, scope.to_owned());
                moved += 1;
            }
        }
        if moved > 0 {
            self.write_owners(&owners).await?;
        }
        Ok(moved)
    }

    /// Deletes every file of `scope`. Called when its session goes.
    pub async fn delete_scope(&self, scope: &str) -> anyhow::Result<usize> {
        let _guard = self.owners_lock.lock().await;
        let mut owners = self.owners().await;
        let mine: Vec<String> = owners
            .iter()
            .filter(|(_, owner)| owner.as_str() == scope)
            .map(|(name, _)| name.clone())
            .collect();
        for name in &mine {
            let _ = tokio::fs::remove_file(self.directory.join(name)).await;
            owners.remove(name);
        }
        if !mine.is_empty() {
            self.write_owners(&owners).await?;
        }
        Ok(mine.len())
    }

    /// Every stored file name, the owner table aside.
    async fn stored_names(&self) -> Vec<String> {
        let Ok(mut entries) = tokio::fs::read_dir(&self.directory).await else {
            return Vec::new();
        };
        let mut names = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name == OWNERS_FILE || name.starts_with('.') {
                continue;
            }
            if entry.metadata().await.is_ok_and(|data| data.is_file()) {
                names.push(name);
            }
        }
        names
    }

    /// Lists the uploads of `scope`, sorted by name. The draft scope
    /// also lists files nothing owns, so a file left by an older
    /// version stays reachable and deletable.
    pub async fn list(&self, scope: &str) -> anyhow::Result<Vec<UploadSummary>> {
        let owners = self.owners().await;
        let mut summaries = Vec::new();
        for name in self.stored_names().await {
            if !is_in_scope(&name, scope, &owners) {
                continue;
            }
            let metadata = tokio::fs::metadata(self.directory.join(&name)).await?;
            summaries.push(UploadSummary {
                content_type: content_type_of(&name),
                is_image: is_image_name(&name),
                name,
                size_bytes: metadata.len(),
            });
        }
        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(summaries)
    }

    /// Reads one upload. `Ok(None)` means no file with that name exists.
    pub async fn read(&self, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match tokio::fs::read(self.directory.join(name)).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Removes one upload. `Ok(false)` means no file with that name
    /// exists.
    pub async fn delete(&self, name: &str) -> anyhow::Result<bool> {
        let _guard = self.owners_lock.lock().await;
        match tokio::fs::remove_file(self.directory.join(name)).await {
            Ok(()) => {
                let mut owners = self.owners().await;
                if owners.remove(name).is_some() {
                    self.write_owners(&owners).await?;
                }
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

/// The `/uploads` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/uploads", get(list_uploads).post(upload))
        .route("/uploads/{name}", get(get_upload).delete(delete_upload))
        .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT_BYTES))
}

/// The longest stem a stored name keeps. A deep folder path loses its
/// front, so the leaf name always survives.
const STEM_LIMIT: usize = 100;

/// Turns an arbitrary client file name into a safe kebab-case name:
/// the folders and the stem in lowercase, with runs of other characters
/// collapsed to one hyphen, plus a lowercase alphanumeric extension.
/// `None` when nothing safe remains. The result never contains `/` or
/// `..`, so it is safe as a file name.
///
/// The folders stay in the name because a pasted folder sends every
/// file under one relative path. Two files named `readme.md` in two
/// folders must not overwrite one another.
fn sanitize_file_name(file_name: &str) -> Option<String> {
    let path = std::path::Path::new(file_name);
    let raw_extension = path.extension().and_then(|extension| extension.to_str());
    let extension = raw_extension
        .map(str::to_ascii_lowercase)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 10
                && extension
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        });
    let stem_source = match (raw_extension, &extension) {
        // The extension is the tail of the name, so cutting its length
        // plus the dot leaves the folders and the stem.
        (Some(raw), Some(_)) => &file_name[..file_name.len() - raw.len() - 1],
        _ => file_name,
    };
    let stem = fold_to_kebab_case(stem_source);
    let stem = stem.get(stem.len().saturating_sub(STEM_LIMIT)..)?;
    let stem = stem.trim_matches('-');
    if stem.is_empty() {
        return None;
    }
    match extension {
        Some(extension) => Some(format!("{stem}.{extension}")),
        None => Some(stem.to_owned()),
    }
}

/// Lowercases the ASCII letters and digits of `text` and collapses every
/// run of other characters, `/` included, to one hyphen.
fn fold_to_kebab_case(text: &str) -> String {
    let mut folded = String::new();
    let mut previous_was_hyphen = true;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            folded.push(character.to_ascii_lowercase());
            previous_was_hyphen = false;
        } else if !previous_was_hyphen {
            folded.push('-');
            previous_was_hyphen = true;
        }
    }
    folded
}

/// True when `name` belongs to `scope`. The draft scope also holds
/// every file nothing owns.
fn is_in_scope(name: &str, scope: &str, owners: &HashMap<String, String>) -> bool {
    match owners.get(name) {
        Some(owner) => owner == scope,
        None => scope == DRAFT_SCOPE,
    }
}

/// `wanted`, or `wanted-2`, `wanted-3` and so on when another scope
/// already holds that name. A scope overwrites its own file.
fn free_name(wanted: &str, scope: &str, owners: &HashMap<String, String>) -> String {
    if owners.get(wanted).is_none_or(|owner| owner == scope) {
        return wanted.to_owned();
    }
    let (stem, extension) = match wanted.rsplit_once('.') {
        Some((stem, extension)) => (stem, format!(".{extension}")),
        None => (wanted, String::new()),
    };
    for number in 2..1000 {
        let candidate = format!("{stem}-{number}{extension}");
        if owners.get(&candidate).is_none_or(|owner| owner == scope) {
            return candidate;
        }
    }
    wanted.to_owned()
}

/// True for names `sanitize_file_name` can produce. Blocks traversal in
/// the serving route.
pub(crate) fn is_stored_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains("..")
        && name.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '.'
        })
}

/// True when the file name has an image extension.
pub(crate) fn is_image_name(name: &str) -> bool {
    content_type_of(name).starts_with("image/")
}

/// Content type for a file name, from its extension. Shared with the
/// static-file routes, so it also covers UI bundle types.
pub(crate) fn content_type_of(name: &str) -> &'static str {
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "json" => "application/json",
        "html" => "text/html; charset=utf-8",
        "css" => "text/css",
        "js" => "text/javascript",
        "wasm" => "application/wasm",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// The scope of a listing or an upload: `?session={id}`. Absent means
/// the draft scope, which is what the landing page uses.
#[derive(Debug, Deserialize)]
struct ScopeQuery {
    #[serde(default)]
    session: Option<String>,
}

impl ScopeQuery {
    /// The scope this request names.
    fn scope(&self) -> &str {
        match self.session.as_deref() {
            Some(session) if !session.trim().is_empty() => session,
            _ => DRAFT_SCOPE,
        }
    }
}

/// Stores every file part of a multipart request under its scope.
async fn upload(
    State(store): State<UploadStore>,
    State(notifier): State<ChangeNotifier>,
    Query(query): Query<ScopeQuery>,
    mut multipart: Multipart,
) -> Response {
    let mut stored = Vec::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return api_error::error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("malformed multipart request: {error}"),
                    Vec::new(),
                );
            }
        };
        let Some(file_name) = field.file_name().map(ToOwned::to_owned) else {
            continue;
        };
        let bytes = match field.bytes().await {
            Ok(bytes) => bytes,
            Err(error) => {
                return api_error::error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("failed to read file part `{file_name}`: {error}"),
                    Vec::new(),
                );
            }
        };
        match store.save(query.scope(), &file_name, &bytes).await {
            Ok(Some(name)) => {
                tracing::info!(%name, scope = %query.scope(), size_bytes = bytes.len(), "upload stored");
                stored.push(serde_json::json!({
                    "name": name,
                    "url": format!("/uploads/{name}"),
                    "size_bytes": bytes.len(),
                    "content_type": content_type_of(&name),
                    "is_image": is_image_name(&name),
                }));
            }
            Ok(None) => {
                return api_error::error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("file name `{file_name}` has no usable characters"),
                    Vec::new(),
                );
            }
            Err(error) => return api_error::internal_error(&error),
        }
    }
    if stored.is_empty() {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            "no file parts in request: send multipart fields with file names",
            Vec::new(),
        );
    }
    notifier.notify();
    Json(stored).into_response()
}

/// Lists the uploads of one scope.
async fn list_uploads(
    State(store): State<UploadStore>,
    Query(query): Query<ScopeQuery>,
) -> Response {
    match store.list(query.scope()).await {
        Ok(summaries) => Json(summaries).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves one upload with a content type from its extension.
async fn get_upload(State(store): State<UploadStore>, Path(name): Path<String>) -> Response {
    if !is_stored_name(&name) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid upload name `{name}`"),
            Vec::new(),
        );
    }
    match store.read(&name).await {
        Ok(Some(bytes)) => {
            ([(header::CONTENT_TYPE, content_type_of(&name))], bytes).into_response()
        }
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("no upload named `{name}`"),
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Removes one upload and bumps the events revision.
async fn delete_upload(
    State(store): State<UploadStore>,
    State(notifier): State<ChangeNotifier>,
    Path(name): Path<String>,
) -> Response {
    if !is_stored_name(&name) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid upload name `{name}`"),
            Vec::new(),
        );
    }
    match store.delete(&name).await {
        Ok(true) => {
            tracing::info!(%name, "upload deleted");
            notifier.notify();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("no upload named `{name}`: run `GET /uploads` for the stored list"),
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::uploads::{
        UploadStore, content_type_of, is_image_name, is_stored_name, sanitize_file_name,
    };

    #[test]
    fn image_flag_follows_the_content_type() {
        assert!(is_image_name("chart.png"));
        assert!(is_image_name("photo.jpg"));
        assert!(is_image_name("logo.svg"));
        assert!(!is_image_name("notes.pdf"));
        assert!(!is_image_name("brief.md"));
        assert!(!is_image_name("no-extension"));
    }

    #[tokio::test]
    async fn deleting_an_upload_reports_whether_it_existed() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("talk", "a.png", b"x").await.unwrap();
        let listing = store.list("talk").await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].content_type, "image/png");
        assert!(listing[0].is_image);
        assert!(store.delete("a.png").await.unwrap());
        assert!(!store.delete("a.png").await.unwrap());
        assert!(store.read("a.png").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_session_sees_only_its_own_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("talk", "notes.md", b"one").await.unwrap();
        store.save("pitch", "other.md", b"two").await.unwrap();
        let names = |listing: Vec<super::UploadSummary>| {
            listing
                .into_iter()
                .map(|summary| summary.name)
                .collect::<Vec<String>>()
        };
        assert_eq!(names(store.list("talk").await.unwrap()), ["notes.md"]);
        assert_eq!(names(store.list("pitch").await.unwrap()), ["other.md"]);
        // The draft scope holds what no session has taken yet.
        assert!(store.list(super::DRAFT_SCOPE).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn one_session_never_overwrites_another_file() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let first = store.save("talk", "readme.md", b"one").await.unwrap();
        let second = store.save("pitch", "readme.md", b"two").await.unwrap();
        assert_eq!(first.as_deref(), Some("readme.md"));
        assert_eq!(second.as_deref(), Some("readme-2.md"));
        assert_eq!(store.read("readme.md").await.unwrap().unwrap(), b"one");
        // The same session writing the same name replaces its own file.
        let again = store.save("talk", "readme.md", b"three").await.unwrap();
        assert_eq!(again.as_deref(), Some("readme.md"));
        assert_eq!(store.read("readme.md").await.unwrap().unwrap(), b"three");
    }

    #[tokio::test]
    async fn a_new_session_takes_the_draft_files_and_the_unowned_ones() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store
            .save(super::DRAFT_SCOPE, "brief.md", b"one")
            .await
            .unwrap();
        // A file from before the scopes existed, written straight into
        // the directory.
        tokio::fs::write(directory.path().join("legacy.md"), b"old")
            .await
            .unwrap();
        assert_eq!(store.adopt("talk").await.unwrap(), 2);
        let mut names: Vec<String> = store
            .list("talk")
            .await
            .unwrap()
            .into_iter()
            .map(|summary| summary.name)
            .collect();
        names.sort();
        assert_eq!(names, ["brief.md", "legacy.md"]);
        assert!(store.list(super::DRAFT_SCOPE).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deleting_a_scope_takes_its_files_and_leaves_the_rest() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("talk", "a.md", b"one").await.unwrap();
        store.save("pitch", "b.md", b"two").await.unwrap();
        assert_eq!(store.delete_scope("talk").await.unwrap(), 1);
        assert!(store.read("a.md").await.unwrap().is_none());
        assert!(store.read("b.md").await.unwrap().is_some());
    }

    #[test]
    fn sanitizes_file_names_to_kebab_case() {
        assert_eq!(
            sanitize_file_name("Chart Final.PNG"),
            Some("chart-final.png".to_owned())
        );
        assert_eq!(
            sanitize_file_name("../../etc/passwd"),
            Some("etc-passwd".to_owned())
        );
        assert_eq!(
            sanitize_file_name("Q3 (draft) v2.pdf"),
            Some("q3-draft-v2.pdf".to_owned())
        );
        assert_eq!(sanitize_file_name("???"), None);
    }

    #[test]
    fn a_folder_path_keeps_its_folders_in_the_name() {
        assert_eq!(
            sanitize_file_name("Brand Kit/logos/mark.SVG"),
            Some("brand-kit-logos-mark.svg".to_owned())
        );
        // Two leaves with one name stay two files.
        assert_ne!(
            sanitize_file_name("kit/api/readme.md"),
            sanitize_file_name("kit/ui/readme.md")
        );
    }

    #[test]
    fn a_deep_path_keeps_its_leaf_and_drops_its_front() {
        let deep = format!("{}/report.pdf", vec!["folder"; 40].join("/"));
        let name = sanitize_file_name(&deep).unwrap();
        assert!(name.ends_with("folder-report.pdf"), "{name}");
        assert!(name.len() <= 104, "{name}");
        assert!(!name.starts_with('-'), "{name}");
        assert!(is_stored_name(&name), "{name}");
    }

    #[test]
    fn stored_name_check_blocks_traversal() {
        assert!(is_stored_name("chart-final.png"));
        assert!(!is_stored_name("../secret"));
        assert!(!is_stored_name(".hidden"));
        assert!(!is_stored_name("UPPER.png"));
        assert!(!is_stored_name(""));
    }

    #[test]
    fn content_types_cover_common_extensions() {
        assert_eq!(content_type_of("a.png"), "image/png");
        assert_eq!(content_type_of("a.pdf"), "application/pdf");
        assert_eq!(content_type_of("a.unknown"), "application/octet-stream");
    }
}
