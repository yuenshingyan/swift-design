//! Design export: one self-contained file the user can download.
//!
//! `GET /designs/{id}/export` renders the design like the render route,
//! but first inlines every image element and screen background image
//! that points at `/uploads/{name}` as a `data:` URI, then fetches the
//! Google Fonts stylesheet and every font file it names and inlines
//! them too, so the exported file opens offline. Images referenced
//! inside text fragments are not rewritten. When the font fetch fails,
//! the export keeps the online `<link>`. `GET /designs/{id}/export.pdf`
//! prints the same page with the user's Chrome, one screen per page.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use design_model::Design;

use crate::api_error;
use crate::designs::{DesignStore, is_valid_design_id};
use crate::render::{self, RenderOptions};
use crate::screenshots;
use crate::uploads::{UploadStore, content_type_of, is_stored_name};

/// The `/designs/{id}/export` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/designs/{id}/export", get(export_design))
        .route("/designs/{id}/export.pdf", get(export_design_pdf))
}

/// The `Content-Disposition` value that names the download `{id}.{extension}`.
fn attachment_disposition(id: &str, extension: &str) -> String {
    format!("attachment; filename=\"{id}.{extension}\"")
}

/// The User-Agent the font fetch sends. Google Fonts returns `woff2`
/// only for a browser User-Agent.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Longest time to wait for one font stylesheet or font file.
const FONT_FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Start of the stylesheet `<link>` that `screen_css::google_fonts_link`
/// writes into the page head.
const FONT_LINK_PREFIX: &str = "<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/";

/// The two preconnect hints that go with the stylesheet link. An
/// offline page has no use for them.
const PRECONNECT_LINKS: [&str; 2] = [
    "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n",
    "<link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n",
];

/// Start of one font file reference inside the Google Fonts stylesheet.
const FONT_URL_PREFIX: &str = "url(https://fonts.gstatic.com/";

/// The CSP sources the render route allows for online fonts, and what
/// they become once every font is inline.
const CSP_FONT_SOURCES: [(&str, &str); 2] = [
    (
        "style-src 'unsafe-inline' https://fonts.googleapis.com;",
        "style-src 'unsafe-inline';",
    ),
    (
        "font-src 'self' https://fonts.gstatic.com;",
        "font-src 'self' data:;",
    ),
];

/// Where the Google Fonts `<link>` sits in the page, and its URL.
#[derive(Debug, PartialEq, Eq)]
struct FontLink {
    /// Byte offset of `<link`.
    start: usize,
    /// Byte offset right after the closing `>`.
    end: usize,
    /// The stylesheet URL.
    url: String,
}

/// Finds the Google Fonts stylesheet `<link>` in `html`.
fn google_fonts_link(html: &str) -> Option<FontLink> {
    let start = html.find(FONT_LINK_PREFIX)?;
    let url_start = start + "<link rel=\"stylesheet\" href=\"".len();
    let url_length = html[url_start..].find('"')?;
    let end = url_start + url_length + html[url_start + url_length..].find('>')? + 1;
    Some(FontLink {
        start,
        end,
        url: html[url_start..url_start + url_length].to_owned(),
    })
}

/// Replaces the Google Fonts `<link>` in `html` with a `<style>` element
/// that holds the stylesheet with every font file as a `data:` URI, and
/// drops the preconnect hints and the online font sources from the CSP.
/// `fetch` returns the bytes behind one URL. Any fetch failure is
/// logged and leaves `html` as it was, so the export still succeeds.
async fn inline_google_fonts<F, Fut>(html: String, fetch: F) -> String
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<u8>>>,
{
    let Some(link) = google_fonts_link(&html) else {
        return html;
    };
    match offline_font_css(&link.url, &fetch).await {
        Ok(css) => replace_font_link(&html, &link, &css),
        Err(error) => {
            tracing::warn!(
                url = %link.url,
                %error,
                "font fetch failed: the export keeps the online font link"
            );
            html
        }
    }
}

/// Fetches the Google Fonts stylesheet at `url` and inlines every font
/// file it names.
async fn offline_font_css<F, Fut>(url: &str, fetch: &F) -> anyhow::Result<String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<u8>>>,
{
    let css = String::from_utf8(fetch(url.to_owned()).await?)?;
    inline_font_urls(&css, fetch).await
}

/// Replaces every `url(https://fonts.gstatic.com/...)` in `css` with a
/// `data:font/woff2;base64,...` URI. The rest of the CSS is unchanged.
/// The first fetch failure fails the whole rewrite.
async fn inline_font_urls<F, Fut>(css: &str, fetch: &F) -> anyhow::Result<String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<u8>>>,
{
    let mut result = String::with_capacity(css.len());
    let mut cursor = 0;
    while let Some(found) = css[cursor..].find(FONT_URL_PREFIX) {
        let url_start = cursor + found + "url(".len();
        let Some(url_length) = css[url_start..].find(')') else {
            break;
        };
        let url = &css[url_start..url_start + url_length];
        let bytes = fetch(url.to_owned()).await?;
        result.push_str(&css[cursor..url_start]);
        result.push_str("data:font/woff2;base64,");
        result.push_str(&BASE64.encode(&bytes));
        cursor = url_start + url_length;
    }
    result.push_str(&css[cursor..]);
    Ok(result)
}

/// Swaps `link` for a `<style>` element that holds `css`, removes the
/// preconnect hints, and drops the online font hosts from the CSP.
fn replace_font_link(html: &str, link: &FontLink, css: &str) -> String {
    let mut result = String::with_capacity(html.len() + css.len());
    result.push_str(&html[..link.start]);
    result.push_str("<style>\n");
    result.push_str(css);
    result.push_str("\n</style>");
    result.push_str(&html[link.end..]);
    for preconnect in PRECONNECT_LINKS {
        result = result.replace(preconnect, "");
    }
    for (online, offline) in CSP_FONT_SOURCES {
        result = result.replace(online, offline);
    }
    result
}

/// Fetches one URL as a browser would: Google Fonts answers with
/// `woff2` only for a browser User-Agent.
#[cfg(not(test))]
async fn fetch_as_browser(url: String) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(FONT_FETCH_TIMEOUT)
        .build()?;
    let response = client
        .get(&url)
        .header("user-agent", BROWSER_USER_AGENT)
        .send()
        .await?
        .error_for_status()?;
    Ok(response.bytes().await?.to_vec())
}

/// The test build never reaches the network: every fetch fails at once,
/// so the export keeps its online font link.
#[cfg(test)]
async fn fetch_as_browser(url: String) -> anyhow::Result<Vec<u8>> {
    let _ = (FONT_FETCH_TIMEOUT, BROWSER_USER_AGENT);
    anyhow::bail!("no network in tests: {url}")
}

/// Prints a stored design to a PDF with the user's Chrome and returns it
/// as a file download. 503 when no Chrome is installed.
async fn export_design_pdf(
    State(designs): State<DesignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    let design = match designs.load(&id).await {
        Ok(Some(design)) => design,
        Ok(None) => return api_error::design_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    let errors = design.validate();
    if !errors.is_empty() {
        return api_error::validation_failed(&errors);
    }
    build_pdf_response(&id, design, &uploads, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the print page, and prints it with
/// `chrome`. `chrome` is a parameter so the no-Chrome path is testable.
async fn build_pdf_response(
    id: &str,
    mut design: Design,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_images(&mut design, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = render::render_design_with(
        &design,
        RenderOptions {
            is_print: true,
            ..RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, design.viewport).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "design exported as pdf");
            (
                [
                    (header::CONTENT_TYPE, "application/pdf".to_owned()),
                    (
                        header::CONTENT_DISPOSITION,
                        attachment_disposition(id, "pdf"),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Renders a stored design with uploaded images and theme fonts inlined
/// and returns it as a file download.
async fn export_design(
    State(designs): State<DesignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    let mut design = match designs.load(&id).await {
        Ok(Some(design)) => design,
        Ok(None) => return api_error::design_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    let errors = design.validate();
    if !errors.is_empty() {
        return api_error::validation_failed(&errors);
    }
    if let Err(error) = inline_uploaded_images(&mut design, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(render::render_design(&design, false), fetch_as_browser).await;
    tracing::info!(%id, size_bytes = html.len(), "design exported");
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                attachment_disposition(&id, "html"),
            ),
        ],
        html,
    )
        .into_response()
}

/// Replaces every `/uploads/{name}` reference in screen html and css
/// with a `data:` URI: `<img src>`, `href`, inline `style` backgrounds,
/// and `url()` in the screen CSS. Names that are missing or unsafe stay
/// as written, so the export still succeeds.
async fn inline_uploaded_images(design: &mut Design, uploads: &UploadStore) -> anyhow::Result<()> {
    let mut names: Vec<String> = Vec::new();
    for screen in &design.screens {
        collect_upload_names(&screen.html, &mut names);
        if let Some(css) = &screen.css {
            collect_upload_names(css, &mut names);
        }
    }
    let mut data_uris: HashMap<String, String> = HashMap::new();
    for name in names {
        if !is_stored_name(&name) {
            continue;
        }
        if let Some(bytes) = uploads.read(&name).await? {
            data_uris.insert(
                name.clone(),
                format!(
                    "data:{content_type};base64,{data}",
                    content_type = content_type_of(&name),
                    data = base64_encode(&bytes),
                ),
            );
        }
    }
    if data_uris.is_empty() {
        return Ok(());
    }
    for screen in &mut design.screens {
        screen.html = replace_upload_references(&screen.html, &data_uris);
        if let Some(css) = &screen.css {
            screen.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// Adds every upload name referenced in `text` to `names`, once.
fn collect_upload_names(text: &str, names: &mut Vec<String>) {
    for (_, name, _) in upload_references(text) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
}

/// Every `/uploads/{name}` reference in `text` as (start, name, end),
/// where `end` is the index right after the name.
fn upload_references(text: &str) -> Vec<(usize, String, usize)> {
    let mut references = Vec::new();
    let mut search_from = 0;
    while let Some(found) = text[search_from..].find("uploads/") {
        let start = search_from + found;
        let name_start = start + "uploads/".len();
        let name: String = text[name_start..]
            .chars()
            .take_while(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || *character == '-'
                    || *character == '.'
            })
            .collect();
        let reference_start = if start > 0 && text.as_bytes()[start - 1] == b'/' {
            start - 1
        } else {
            start
        };
        let end = name_start + name.len();
        if !name.is_empty() {
            references.push((reference_start, name, end));
        }
        search_from = end.max(start + 1);
    }
    references
}

/// Substitutes data URIs for the upload references that have one.
fn replace_upload_references(text: &str, data_uris: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, name, end) in upload_references(text) {
        if start < cursor {
            continue;
        }
        result.push_str(&text[cursor..start]);
        match data_uris.get(&name) {
            Some(data_uri) => result.push_str(data_uri),
            None => result.push_str(&text[start..end]),
        }
        cursor = end;
    }
    result.push_str(&text[cursor..]);
    result
}

/// Standard base64 with padding. Written here to keep the export free
/// of a dependency for one small encoding. Also used by the login
/// flow's PKCE values.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[(group >> 18) as usize & 0x3f] as char);
        encoded.push(ALPHABET[(group >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(group >> 6) as usize & 0x3f] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[group as usize & 0x3f] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::http::StatusCode;
    use design_model::Design;

    use crate::export::{
        FONT_LINK_PREFIX, FontLink, attachment_disposition, base64_encode, build_pdf_response,
        google_fonts_link, inline_font_urls, inline_google_fonts, inline_uploaded_images,
        upload_references,
    };
    use crate::render;
    use crate::uploads::UploadStore;

    const STYLESHEET_URL: &str =
        "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap";
    const FONT_CSS: &str = "@font-face { font-family: 'Inter'; font-style: normal; font-weight: 400; src: url(https://fonts.gstatic.com/s/inter/a.woff2) format('woff2'); }\n@font-face { font-family: 'Inter'; font-weight: 700; src: url(https://fonts.gstatic.com/s/inter/b.woff2) format('woff2'); }";

    fn sample_design() -> Design {
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap()
    }

    /// A fetcher with no network: the stylesheet URL returns `FONT_CSS`
    /// and every font URL returns its own file name as bytes.
    async fn fake_fetch(url: String) -> anyhow::Result<Vec<u8>> {
        if url.starts_with("https://fonts.googleapis.com/") {
            return Ok(FONT_CSS.as_bytes().to_vec());
        }
        let name = url.rsplit('/').next().unwrap_or_default();
        Ok(format!("FONT:{name}").into_bytes())
    }

    async fn failing_fetch(url: String) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("offline: {url}")
    }

    fn data_uri(name: &str) -> String {
        format!(
            "data:font/woff2;base64,{}",
            base64_encode(format!("FONT:{name}").as_bytes())
        )
    }

    #[test]
    fn google_fonts_link_is_found_with_its_bounds() {
        let html = format!(
            "<head>\n<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link rel=\"stylesheet\" href=\"{STYLESHEET_URL}\">\n<style>"
        );
        let link = google_fonts_link(&html).unwrap();
        assert_eq!(
            &html[link.start..link.end],
            format!("<link rel=\"stylesheet\" href=\"{STYLESHEET_URL}\">")
        );
        assert_eq!(
            link,
            FontLink {
                start: link.start,
                end: link.end,
                url: STYLESHEET_URL.to_owned()
            }
        );
        assert!(google_fonts_link("<head><style></style></head>").is_none());
    }

    #[tokio::test]
    async fn rewrite_replaces_one_font_url_with_a_data_uri() {
        let css =
            "@font-face { src: url(https://fonts.gstatic.com/s/inter/a.woff2) format('woff2'); }";
        let rewritten = inline_font_urls(css, &fake_fetch).await.unwrap();
        assert_eq!(
            rewritten,
            format!(
                "@font-face {{ src: url({}) format('woff2'); }}",
                data_uri("a.woff2")
            )
        );
    }

    #[tokio::test]
    async fn rewrite_replaces_every_font_url() {
        let rewritten = inline_font_urls(FONT_CSS, &fake_fetch).await.unwrap();
        assert!(rewritten.contains(&data_uri("a.woff2")));
        assert!(rewritten.contains(&data_uri("b.woff2")));
        assert!(!rewritten.contains("fonts.gstatic.com"));
        assert_eq!(rewritten.matches("data:font/woff2;base64,").count(), 2);
    }

    #[tokio::test]
    async fn rewrite_keeps_the_rest_of_the_css_unchanged() {
        let rewritten = inline_font_urls(FONT_CSS, &fake_fetch).await.unwrap();
        let restored = rewritten
            .replace(
                &data_uri("a.woff2"),
                "https://fonts.gstatic.com/s/inter/a.woff2",
            )
            .replace(
                &data_uri("b.woff2"),
                "https://fonts.gstatic.com/s/inter/b.woff2",
            );
        assert_eq!(restored, FONT_CSS);
        assert_eq!(
            inline_font_urls("p { color: red }", &fake_fetch)
                .await
                .unwrap(),
            "p { color: red }"
        );
    }

    #[tokio::test]
    async fn a_fetch_error_keeps_the_original_html_and_link() {
        let html = render::render_design(&sample_design(), false);
        assert!(html.contains(FONT_LINK_PREFIX));
        let result = inline_google_fonts(html.clone(), failing_fetch).await;
        assert_eq!(result, html);
    }

    #[tokio::test]
    async fn exported_html_holds_no_google_fonts_host_after_the_rewrite() {
        let html = render::render_design(&sample_design(), false);
        let result = inline_google_fonts(html, fake_fetch).await;
        assert!(!result.contains("fonts.googleapis.com"));
        assert!(result.contains("<style>\n@font-face"));
        assert!(!result.contains("rel=\"preconnect\""));
        assert!(result.contains("style-src 'unsafe-inline';"));
    }

    #[tokio::test]
    async fn exported_html_holds_no_gstatic_host_after_the_rewrite() {
        let html = render::render_design(&sample_design(), false);
        let result = inline_google_fonts(html, fake_fetch).await;
        assert!(!result.contains("fonts.gstatic.com"));
        assert!(result.contains(&data_uri("a.woff2")));
        assert!(result.contains("font-src 'self' data:;"));
    }

    #[test]
    fn attachment_disposition_names_the_design_file() {
        assert_eq!(
            attachment_disposition("overview", "pdf"),
            "attachment; filename=\"overview.pdf\""
        );
        assert_eq!(
            attachment_disposition("overview", "html"),
            "attachment; filename=\"overview.html\""
        );
    }

    #[tokio::test]
    async fn pdf_export_without_chrome_is_503() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let response = build_pdf_response("overview", sample_design(), &store, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("need Chrome or Chromium"));
        assert!(message.contains("SWIFT_DESIGN_CHROME"));
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[tokio::test]
    async fn inlines_uploaded_images_in_html_and_css() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("chart.png", b"PNGDATA").await.unwrap();

        let mut design = sample_design();
        design.screens[0].html = "<img src='/uploads/chart.png'><div style=\"background:url(/uploads/chart.png)\"></div>".to_owned();
        design.screens[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
        inline_uploaded_images(&mut design, &store).await.unwrap();
        let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
        assert_eq!(design.screens[0].html.matches(&expected).count(), 2);
        assert!(
            design.screens[0]
                .css
                .as_deref()
                .unwrap()
                .contains(&expected)
        );
        assert!(!design.screens[0].html.contains("/uploads/"));
    }

    #[tokio::test]
    async fn keeps_missing_and_external_images_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let mut design = sample_design();
        design.screens[0].html =
            "<img src='/uploads/absent.png'><a href='https://example.com/a.png'>x</a>".to_owned();
        inline_uploaded_images(&mut design, &store).await.unwrap();
        assert!(design.screens[0].html.contains("/uploads/absent.png"));
        assert!(design.screens[0].html.contains("https://example.com/a.png"));
    }

    #[test]
    fn upload_references_are_found_with_their_bounds() {
        let references = upload_references("a /uploads/x.png b uploads/y-1.jpg c /uploads/");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].1, "x.png");
        assert_eq!(
            &"a /uploads/x.png"[references[0].0..references[0].2],
            "/uploads/x.png"
        );
        assert_eq!(references[1].1, "y-1.jpg");
    }
}
