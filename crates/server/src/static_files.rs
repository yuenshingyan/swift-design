//! Serves the built editor UI (the `ui` crate's WASM bundle).

use std::path::{Path as FilePath, PathBuf};

use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

use crate::api_error;
use crate::uploads::content_type_of;

/// Shown at `/` until the UI bundle exists.
const UI_NOT_BUILT_PAGE: &str = "<!doctype html>\n<html lang=\"en\"><head>\
<meta charset=\"utf-8\"><title>Swift Design</title><style>\
body { margin: 0; min-height: 100vh; display: flex; align-items: center;\
 justify-content: center; background: #F7F6F3; color: #15181C;\
 font-family: Inter, system-ui, sans-serif; }\
main { max-width: 34rem; padding: 2rem; }\
h1 { margin: 0 0 1rem; font-size: 2rem; letter-spacing: -0.03em; font-weight: 600; }\
p { margin: 0 0 1rem; line-height: 1.55; color: #4E545B; }\
pre, code { font-family: 'JetBrains Mono', ui-monospace, monospace; }\
pre { background: #14171B; color: #C9CDD2; border-radius: 8px; padding: 1.1rem 1.3rem; }\
</style></head><body><main>\
<h1>Swift Design</h1>\
<p>The editor UI is not built yet. Build it with:</p>\
<pre>cd crates/ui &amp;&amp; dx build --release</pre>\
<p>The API works without it; agents start at <code>GET /instructions</code>.</p>\
</main></body></html>\n";

/// Directory that holds the built UI bundle.
#[derive(Clone)]
pub struct UiDirectory(pub PathBuf);

/// Fallback handler: serves UI files, with `index.html` for `/` and for
/// client-side routes (paths without a file extension).
pub async fn serve_ui(State(UiDirectory(directory)): State<UiDirectory>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if !is_safe_relative_path(path) {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid path `{path}`"),
            Vec::new(),
        );
    }
    if path.is_empty() {
        return serve_index(&directory).await;
    }
    let file = path;
    match tokio::fs::read(directory.join(file)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type_of(file))], bytes).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if FilePath::new(file).extension().is_none() {
                serve_index(&directory).await
            } else {
                api_error::error_response(
                    StatusCode::NOT_FOUND,
                    &format!("no file `{file}`"),
                    Vec::new(),
                )
            }
        }
        Err(error) => api_error::internal_error(&error.into()),
    }
}

/// Serves `index.html`, or the not-built page when it does not exist.
/// The page is sent with `Cache-Control: no-cache` so a new build's
/// hashed asset names are picked up on the next load.
async fn serve_index(directory: &FilePath) -> Response {
    match tokio::fs::read_to_string(directory.join("index.html")).await {
        Ok(index) => ([(header::CACHE_CONTROL, "no-cache")], Html(index)).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Html(UI_NOT_BUILT_PAGE.to_owned()).into_response()
        }
        Err(error) => api_error::internal_error(&error.into()),
    }
}

/// True when every path segment is a plain visible file name. Blocks
/// `..`, hidden files, and characters outside the bundle's naming.
/// A bare `.` segment is allowed: the dx bundle links assets as
/// `/./assets/…`.
fn is_safe_relative_path(path: &str) -> bool {
    path.split('/').all(|segment| {
        segment == "."
            || (!segment.starts_with('.')
                && segment.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                }))
    })
}

#[cfg(test)]
mod tests {
    use crate::static_files::is_safe_relative_path;

    #[test]
    fn safe_path_check_blocks_traversal_and_hidden_files() {
        assert!(is_safe_relative_path(""));
        assert!(is_safe_relative_path("index.html"));
        assert!(is_safe_relative_path("assets/ui_bg.wasm"));
        assert!(is_safe_relative_path("./assets/ui_bg.wasm"));
        assert!(!is_safe_relative_path("../Cargo.toml"));
        assert!(!is_safe_relative_path("assets/../secret"));
        assert!(!is_safe_relative_path(".env"));
        assert!(!is_safe_relative_path("a b"));
    }
}
