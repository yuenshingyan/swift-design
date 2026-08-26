//! Source-material uploads: files agents turn into design content.
//!
//! Files live in one directory with sanitized kebab-case names. Screens
//! reference them as `/uploads/{name}`, so `full_image` screens can use
//! uploaded images directly.

use std::path::PathBuf;

use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::api_error;
use crate::events::ChangeNotifier;

/// Uploads above this size are rejected. Big enough for screen images
/// and PDF source material.
const UPLOAD_BODY_LIMIT_BYTES: usize = 50 * 1024 * 1024;

/// Filesystem-backed upload storage: one sanitized file name per upload.
#[derive(Clone)]
pub struct UploadStore {
    directory: PathBuf,
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
        Self { directory }
    }

    /// Saves `bytes` under a sanitized version of `file_name` and
    /// returns the stored name. Returns `None` when no safe name is
    /// left after sanitizing.
    pub async fn save(&self, file_name: &str, bytes: &[u8]) -> anyhow::Result<Option<String>> {
        let Some(name) = sanitize_file_name(file_name) else {
            return Ok(None);
        };
        tokio::fs::create_dir_all(&self.directory).await?;
        tokio::fs::write(self.directory.join(&name), bytes).await?;
        Ok(Some(name))
    }

    /// Lists every upload, sorted by name.
    pub async fn list(&self) -> anyhow::Result<Vec<UploadSummary>> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut summaries = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let metadata = entry.metadata().await?;
            if metadata.is_file() {
                summaries.push(UploadSummary {
                    content_type: content_type_of(&name),
                    is_image: is_image_name(&name),
                    name,
                    size_bytes: metadata.len(),
                });
            }
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
        match tokio::fs::remove_file(self.directory.join(name)).await {
            Ok(()) => Ok(true),
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

/// Turns an arbitrary client file name into a safe kebab-case name:
/// lowercase stem with runs of other characters collapsed to one
/// hyphen, plus a lowercase alphanumeric extension. `None` when nothing
/// safe remains. The result never contains `/` or `..`, so it is safe
/// as a file name.
fn sanitize_file_name(file_name: &str) -> Option<String> {
    let path = std::path::Path::new(file_name);
    let stem = path.file_stem()?.to_str()?;
    let mut sanitized_stem = String::new();
    let mut previous_was_hyphen = true;
    for character in stem.chars() {
        if character.is_ascii_alphanumeric() {
            sanitized_stem.push(character.to_ascii_lowercase());
            previous_was_hyphen = false;
        } else if !previous_was_hyphen {
            sanitized_stem.push('-');
            previous_was_hyphen = true;
        }
    }
    let sanitized_stem = sanitized_stem.trim_end_matches('-');
    if sanitized_stem.is_empty() {
        return None;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 10
                && extension
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        });
    match extension {
        Some(extension) => Some(format!("{sanitized_stem}.{extension}")),
        None => Some(sanitized_stem.to_owned()),
    }
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

/// Stores every file part of a multipart request.
async fn upload(
    State(store): State<UploadStore>,
    State(notifier): State<ChangeNotifier>,
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
        match store.save(&file_name, &bytes).await {
            Ok(Some(name)) => {
                tracing::info!(%name, size_bytes = bytes.len(), "upload stored");
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

/// Lists every upload.
async fn list_uploads(State(store): State<UploadStore>) -> Response {
    match store.list().await {
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
        store.save("a.png", b"x").await.unwrap();
        let listing = store.list().await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].content_type, "image/png");
        assert!(listing[0].is_image);
        assert!(store.delete("a.png").await.unwrap());
        assert!(!store.delete("a.png").await.unwrap());
        assert!(store.read("a.png").await.unwrap().is_none());
    }

    #[test]
    fn sanitizes_file_names_to_kebab_case() {
        assert_eq!(
            sanitize_file_name("Chart Final.PNG"),
            Some("chart-final.png".to_owned())
        );
        assert_eq!(
            sanitize_file_name("../../etc/passwd"),
            Some("passwd".to_owned())
        );
        assert_eq!(
            sanitize_file_name("Q3 (draft) v2.pdf"),
            Some("q3-draft-v2.pdf".to_owned())
        );
        assert_eq!(sanitize_file_name("???"), None);
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
