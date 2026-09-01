//! Design export: one self-contained file the user can download.
//!
//! `GET /designs/{id}/export` renders the design like the render route,
//! but first inlines every image element and screen background image
//! that points at `/uploads/{name}` as a `data:` URI, then fetches the
//! Google Fonts stylesheet and every font file it names and inlines
//! them too, so the exported file opens offline. Images referenced
//! inside text fragments are not rewritten. When the font fetch fails,
//! the export keeps the online `<link>`. A design has no PDF export: a
//! demo is clicked, and a print loses its flows and widgets.
//!
//! Decks export the same way under `/decks/{id}/export`, and
//! `GET /decks/{id}/export.pdf` prints the deck with the user's Chrome,
//! one slide per page. `GET /decks/{id}/export.pptx` builds a
//! PowerPoint file from the measured slides; see `pptx.rs`.
//!
//! Documents export under `/documents/{id}/export`, and
//! `GET /documents/{id}/export.pdf` prints the document with the user's
//! Chrome, one page per sheet of its paper. `GET
//! /documents/{id}/export.docx` builds a Word file from the pages' HTML;
//! see `docx.rs`. It needs no Chrome.
//!
//! Socials export under `/socials/{id}/export`, and
//! `GET /socials/{id}/export.pdf` prints the social with the user's
//! Chrome, one frame per sheet: the file a LinkedIn carousel takes.
//! `GET /socials/{id}/export.zip` packs one PNG per frame, the files an
//! Instagram carousel takes.
//!
//! Prints export under `/prints/{id}/export`, and
//! `GET /prints/{id}/export.pdf` prints the print with the user's
//! Chrome, one sheet per PDF page: the file a print shop takes.
//! `GET /prints/{id}/export.zip` packs one PNG per sheet.
//!
//! Mailings export under `/mailings/{id}/export`, and
//! `GET /mailings/{id}/export.pdf` prints the mailing with the user's
//! Chrome, one email per PDF page. `GET /mailings/{id}/export.zip`
//! packs one PNG per email. `GET /mailings/{id}/export.email.zip`
//! packs one email-client HTML file per email, built by `email_html`
//! with no Chrome, plus a subjects file.
//!
//! Campaigns export under `/campaigns/{id}/export`, and
//! `GET /campaigns/{id}/export.pdf` prints the campaign with the
//! user's Chrome, one ad per PDF page. `GET /campaigns/{id}/export.zip`
//! packs one PNG per ad: the files an ad platform takes.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use design_model::{Campaign, DECK_VIEWPORT, Deck, Design, Document, Mailing, Print, Social};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::api_error;
use crate::campaign_render;
use crate::campaigns::{CampaignStore, is_valid_campaign_id};
use crate::deck_render;
use crate::decks::{DeckStore, is_valid_deck_id};
use crate::designs::{DesignStore, is_valid_design_id};
use crate::document_render;
use crate::documents::{DocumentStore, is_valid_document_id};
use crate::docx;
use crate::email_html;
use crate::mailing_render;
use crate::mailings::{MailingStore, is_valid_mailing_id};
use crate::pptx;
use crate::print_render;
use crate::prints::{PrintStore, is_valid_print_id};
use crate::render;
use crate::screenshots;
use crate::settings::SettingsStore;
use crate::social_render;
use crate::socials::{SocialStore, is_valid_social_id};
use crate::uploads::{UploadStore, content_type_of, is_stored_name};

/// The content type of a `.pptx` download.
const PPTX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";

/// The `/designs/{id}/export`, `/decks/{id}/export`,
/// `/documents/{id}/export`, `/socials/{id}/export`,
/// `/prints/{id}/export`, and `/mailings/{id}/export` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/designs/{id}/export", get(export_design))
        .route("/decks/{id}/export", get(export_deck))
        .route("/decks/{id}/export.pdf", get(export_deck_pdf))
        .route("/decks/{id}/export.pptx", get(export_deck_pptx))
        .route("/documents/{id}/export", get(export_document))
        .route("/documents/{id}/export.pdf", get(export_document_pdf))
        .route("/documents/{id}/export.docx", get(export_document_docx))
        .route("/socials/{id}/export", get(export_social))
        .route("/socials/{id}/export.pdf", get(export_social_pdf))
        .route("/socials/{id}/export.zip", get(export_social_zip))
        .route("/prints/{id}/export", get(export_print))
        .route("/prints/{id}/export.pdf", get(export_print_pdf))
        .route("/prints/{id}/export.zip", get(export_print_zip))
        .route("/mailings/{id}/export", get(export_mailing))
        .route("/mailings/{id}/export.pdf", get(export_mailing_pdf))
        .route("/mailings/{id}/export.zip", get(export_mailing_zip))
        .route(
            "/mailings/{id}/export.email.zip",
            get(export_mailing_email_zip),
        )
        .route("/campaigns/{id}/export", get(export_campaign))
        .route("/campaigns/{id}/export.pdf", get(export_campaign_pdf))
        .route("/campaigns/{id}/export.zip", get(export_campaign_zip))
}

/// The `Content-Disposition` value that names the download `{id}.{extension}`.
fn attachment_disposition(id: &str, extension: &str) -> String {
    format!("attachment; filename=\"{id}.{extension}\"")
}

/// A file download named `{id}.{extension}` with `content_type`.
fn file_download(id: &str, extension: &str, content_type: &str, bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (
                header::CONTENT_DISPOSITION,
                attachment_disposition(id, extension),
            ),
        ],
        bytes,
    )
        .into_response()
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
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Loads a stored deck for an export: 400, 404, or 422 as a response
/// when the id, the file, or the deck is not usable.
async fn load_deck_for_export(decks: &DeckStore, id: &str) -> Result<Deck, Response> {
    if !is_valid_deck_id(id) {
        return Err(api_error::invalid_deck_id(id));
    }
    let deck = match decks.load(id).await {
        Ok(Some(deck)) => deck,
        Ok(None) => return Err(api_error::deck_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = deck.validate();
    if !errors.is_empty() {
        return Err(api_error::deck_validation_failed(&errors));
    }
    Ok(deck)
}

/// Renders a stored deck with uploaded images and theme fonts inlined
/// and returns it as a file download.
async fn export_deck(
    State(decks): State<DeckStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut deck = match load_deck_for_export(&decks, &id).await {
        Ok(deck) => deck,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_slide_images(&mut deck, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(deck_render::render_deck(&deck, false), fetch_as_browser).await;
    tracing::info!(%id, size_bytes = html.len(), "deck exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Prints a stored deck to a PDF with the user's Chrome and returns it
/// as a file download. 503 when no Chrome is installed.
async fn export_deck_pdf(
    State(decks): State<DeckStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let deck = match load_deck_for_export(&decks, &id).await {
        Ok(deck) => deck,
        Err(response) => return response,
    };
    build_deck_pdf_response(&id, deck, &uploads, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the deck print page, and prints it
/// with `chrome`. `chrome` is a parameter so the no-Chrome path is
/// testable.
async fn build_deck_pdf_response(
    id: &str,
    mut deck: Deck,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_slide_images(&mut deck, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = deck_render::render_deck_with(
        &deck,
        deck_render::RenderOptions {
            is_print: true,
            ..deck_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, DECK_VIEWPORT).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "deck exported as pdf");
            file_download(id, "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Builds a PowerPoint file from a stored deck with the user's Chrome
/// and returns it as a file download. 503 when no Chrome is installed.
async fn export_deck_pptx(
    State(decks): State<DeckStore>,
    State(uploads): State<UploadStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let deck = match load_deck_for_export(&decks, &id).await {
        Ok(deck) => deck,
        Err(response) => return response,
    };
    let sources = pptx::ExportSources {
        uploads: &uploads,
        base_url: format!("http://{}", settings.address()),
    };
    build_pptx_response(&id, &deck, &sources, screenshots::find_chrome()).await
}

/// Measures and packs the deck. `chrome` is a parameter so the
/// no-Chrome path is testable.
async fn build_pptx_response(
    id: &str,
    deck: &Deck,
    sources: &pptx::ExportSources<'_>,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PPTX exports");
    }
    match pptx::export_deck(deck, sources).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "deck exported as pptx");
            file_download(id, "pptx", PPTX_CONTENT_TYPE, bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Loads a stored document for an export: 400, 404, or 422 as a
/// response when the id, the file, or the document is not usable.
async fn load_document_for_export(
    documents: &DocumentStore,
    id: &str,
) -> Result<Document, Response> {
    if !is_valid_document_id(id) {
        return Err(api_error::invalid_document_id(id));
    }
    let document = match documents.load(id).await {
        Ok(Some(document)) => document,
        Ok(None) => return Err(api_error::document_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = document.validate();
    if !errors.is_empty() {
        return Err(api_error::document_validation_failed(&errors));
    }
    Ok(document)
}

/// Renders a stored document with uploaded images and theme fonts
/// inlined and returns it as a file download.
async fn export_document(
    State(documents): State<DocumentStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut document = match load_document_for_export(&documents, &id).await {
        Ok(document) => document,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_page_images(&mut document, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        document_render::render_document(&document, false),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "document exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Prints a stored document to a PDF with the user's Chrome and returns
/// it as a file download. 503 when no Chrome is installed.
async fn export_document_pdf(
    State(documents): State<DocumentStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let document = match load_document_for_export(&documents, &id).await {
        Ok(document) => document,
        Err(response) => return response,
    };
    build_document_pdf_response(&id, document, &uploads, screenshots::find_chrome()).await
}

/// Builds a Word file from a stored document and returns it as a file
/// download. Needs no Chrome.
async fn export_document_docx(
    State(documents): State<DocumentStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let document = match load_document_for_export(&documents, &id).await {
        Ok(document) => document,
        Err(response) => return response,
    };
    match docx::export_document(&document, &uploads).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "document exported as docx");
            file_download(&id, "docx", docx::DOCX_CONTENT_TYPE, bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Inlines uploaded images, renders the document print page, and
/// prints it with `chrome`. `chrome` is a parameter so the no-Chrome
/// path is testable.
async fn build_document_pdf_response(
    id: &str,
    mut document: Document,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_page_images(&mut document, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = document_render::render_document_with(
        &document,
        document_render::RenderOptions {
            is_print: true,
            ..document_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, document.viewport()).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "document exported as pdf");
            file_download(id, "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Loads a stored social for an export: 400, 404, or 422 as a response
/// when the id, the file, or the social is not usable.
async fn load_social_for_export(socials: &SocialStore, id: &str) -> Result<Social, Response> {
    if !is_valid_social_id(id) {
        return Err(api_error::invalid_social_id(id));
    }
    let social = match socials.load(id).await {
        Ok(Some(social)) => social,
        Ok(None) => return Err(api_error::social_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = social.validate();
    if !errors.is_empty() {
        return Err(api_error::social_validation_failed(&errors));
    }
    Ok(social)
}

/// Renders a stored social with uploaded images and theme fonts inlined
/// and returns it as a file download.
async fn export_social(
    State(socials): State<SocialStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut social = match load_social_for_export(&socials, &id).await {
        Ok(social) => social,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_frame_images(&mut social, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        social_render::render_social(&social, false),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "social exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Prints a stored social to a PDF with the user's Chrome, one frame
/// per sheet, and returns it as a file download. 503 when no Chrome is
/// installed.
async fn export_social_pdf(
    State(socials): State<SocialStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let social = match load_social_for_export(&socials, &id).await {
        Ok(social) => social,
        Err(response) => return response,
    };
    build_social_pdf_response(&id, social, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per frame of a stored social into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
async fn export_social_zip(
    State(socials): State<SocialStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let social = match load_social_for_export(&socials, &id).await {
        Ok(social) => social,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_social_zip_response(&id, &social, &base_url, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the social print page, and prints
/// it with `chrome`. `chrome` is a parameter so the no-Chrome path is
/// testable.
async fn build_social_pdf_response(
    id: &str,
    mut social: Social,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_frame_images(&mut social, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = social_render::render_social_with(
        &social,
        social_render::RenderOptions {
            is_print: true,
            ..social_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, social.viewport()).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "social exported as pdf");
            file_download(id, "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every frame and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
async fn build_social_zip_response(
    id: &str,
    social: &Social,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(social.frames.len());
    for index in 0..social.frames.len() {
        match screenshots::screenshot_frame(social, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_frame_images(id, &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), frame_count = images.len(), "social exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// One zip with `{id}-frame-{n}.png` per image, 1-based, in order. A
/// PNG is compressed already, so the entries are stored as they are.
fn pack_frame_images(id: &str, images: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, image) in images.iter().enumerate() {
        writer.start_file(format!("{id}-frame-{}.png", index + 1), options)?;
        writer.write_all(image)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Loads a stored print for an export: 400, 404, or 422 as a response
/// when the id, the file, or the print is not usable.
async fn load_print_for_export(prints: &PrintStore, id: &str) -> Result<Print, Response> {
    if !is_valid_print_id(id) {
        return Err(api_error::invalid_print_id(id));
    }
    let print = match prints.load(id).await {
        Ok(Some(print)) => print,
        Ok(None) => return Err(api_error::print_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = print.validate();
    if !errors.is_empty() {
        return Err(api_error::print_validation_failed(&errors));
    }
    Ok(print)
}

/// Renders a stored print with uploaded images and theme fonts inlined
/// and returns it as a file download.
async fn export_print(
    State(prints): State<PrintStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut print = match load_print_for_export(&prints, &id).await {
        Ok(print) => print,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_sheet_images(&mut print, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html =
        inline_google_fonts(print_render::render_print(&print, false), fetch_as_browser).await;
    tracing::info!(%id, size_bytes = html.len(), "print exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Prints a stored print to a PDF with the user's Chrome, one sheet
/// per PDF page, and returns it as a file download. 503 when no Chrome
/// is installed.
async fn export_print_pdf(
    State(prints): State<PrintStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let print = match load_print_for_export(&prints, &id).await {
        Ok(print) => print,
        Err(response) => return response,
    };
    build_print_pdf_response(&id, print, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per sheet of a stored print into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
async fn export_print_zip(
    State(prints): State<PrintStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let print = match load_print_for_export(&prints, &id).await {
        Ok(print) => print,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_print_zip_response(&id, &print, &base_url, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the print's print page, and prints
/// it with `chrome`. `chrome` is a parameter so the no-Chrome path is
/// testable.
async fn build_print_pdf_response(
    id: &str,
    mut print: Print,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_sheet_images(&mut print, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = print_render::render_print_with(
        &print,
        print_render::RenderOptions {
            is_print: true,
            ..print_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, print.viewport()).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "print exported as pdf");
            file_download(id, "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every sheet and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
async fn build_print_zip_response(
    id: &str,
    print: &Print,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(print.sheets.len());
    for index in 0..print.sheets.len() {
        match screenshots::screenshot_sheet(print, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_sheet_images(id, &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), sheet_count = images.len(), "print exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// One zip with `{id}-sheet-{n}.png` per image, 1-based, in order. A
/// PNG is compressed already, so the entries are stored as they are.
fn pack_sheet_images(id: &str, images: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, image) in images.iter().enumerate() {
        writer.start_file(format!("{id}-sheet-{}.png", index + 1), options)?;
        writer.write_all(image)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// The print twin of `inline_uploaded_images`: rewrites sheet html and
/// css.
async fn inline_uploaded_sheet_images(
    print: &mut Print,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = print
        .sheets
        .iter()
        .flat_map(|sheet| [Some(sheet.html.as_str()), sheet.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for sheet in &mut print.sheets {
        sheet.html = replace_upload_references(&sheet.html, &data_uris);
        if let Some(css) = &sheet.css {
            sheet.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// Loads a stored campaign for an export: 400, 404, or 422 as a
/// response when the id, the file, or the campaign is not usable.
async fn load_campaign_for_export(
    campaigns: &CampaignStore,
    id: &str,
) -> Result<Campaign, Response> {
    if !is_valid_campaign_id(id) {
        return Err(api_error::invalid_campaign_id(id));
    }
    let campaign = match campaigns.load(id).await {
        Ok(Some(campaign)) => campaign,
        Ok(None) => return Err(api_error::campaign_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = campaign.validate();
    if !errors.is_empty() {
        return Err(api_error::campaign_validation_failed(&errors));
    }
    Ok(campaign)
}

/// Renders a stored campaign with uploaded images and theme fonts
/// inlined and returns it as a file download.
async fn export_campaign(
    State(campaigns): State<CampaignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut campaign = match load_campaign_for_export(&campaigns, &id).await {
        Ok(campaign) => campaign,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_ad_images(&mut campaign, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        campaign_render::render_campaign(&campaign, false),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "campaign exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Prints a stored campaign to a PDF with the user's Chrome, one ad
/// per PDF page, and returns it as a file download. 503 when no Chrome
/// is installed.
async fn export_campaign_pdf(
    State(campaigns): State<CampaignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let campaign = match load_campaign_for_export(&campaigns, &id).await {
        Ok(campaign) => campaign,
        Err(response) => return response,
    };
    build_campaign_pdf_response(&id, campaign, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per ad of a stored campaign into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
async fn export_campaign_zip(
    State(campaigns): State<CampaignStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let campaign = match load_campaign_for_export(&campaigns, &id).await {
        Ok(campaign) => campaign,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_campaign_zip_response(&id, &campaign, &base_url, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the campaign's print page, and
/// prints it with `chrome`. `chrome` is a parameter so the no-Chrome
/// path is testable.
async fn build_campaign_pdf_response(
    id: &str,
    mut campaign: Campaign,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_ad_images(&mut campaign, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = campaign_render::render_campaign_with(
        &campaign,
        campaign_render::RenderOptions {
            is_print: true,
            ..campaign_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, campaign.viewport()).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "campaign exported as pdf");
            file_download(id, "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every ad and packs the PNGs. `chrome` is a parameter so
/// the no-Chrome path is testable.
async fn build_campaign_zip_response(
    id: &str,
    campaign: &Campaign,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(campaign.ads.len());
    for index in 0..campaign.ads.len() {
        match screenshots::screenshot_ad(campaign, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_ad_images(id, &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), ad_count = images.len(), "campaign exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// One zip with `{id}-ad-{n}.png` per image, 1-based, in order. A PNG
/// is compressed already, so the entries are stored as they are.
fn pack_ad_images(id: &str, images: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, image) in images.iter().enumerate() {
        writer.start_file(format!("{id}-ad-{}.png", index + 1), options)?;
        writer.write_all(image)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// The campaign twin of `inline_uploaded_images`: rewrites ad html and
/// css.
async fn inline_uploaded_ad_images(
    campaign: &mut Campaign,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = campaign
        .ads
        .iter()
        .flat_map(|ad| [Some(ad.html.as_str()), ad.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for ad in &mut campaign.ads {
        ad.html = replace_upload_references(&ad.html, &data_uris);
        if let Some(css) = &ad.css {
            ad.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// Loads a stored mailing for an export: 400, 404, or 422 as a
/// response when the id, the file, or the mailing is not usable.
async fn load_mailing_for_export(mailings: &MailingStore, id: &str) -> Result<Mailing, Response> {
    if !is_valid_mailing_id(id) {
        return Err(api_error::invalid_mailing_id(id));
    }
    let mailing = match mailings.load(id).await {
        Ok(Some(mailing)) => mailing,
        Ok(None) => return Err(api_error::mailing_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = mailing.validate();
    if !errors.is_empty() {
        return Err(api_error::mailing_validation_failed(&errors));
    }
    Ok(mailing)
}

/// Renders a stored mailing with uploaded images and theme fonts
/// inlined and returns it as a file download.
async fn export_mailing(
    State(mailings): State<MailingStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut mailing = match load_mailing_for_export(&mailings, &id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_email_images(&mut mailing, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        mailing_render::render_mailing(&mailing, false),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "mailing exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Prints a stored mailing to a PDF with the user's Chrome, one email
/// per PDF page, and returns it as a file download. 503 when no
/// Chrome is installed.
async fn export_mailing_pdf(
    State(mailings): State<MailingStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mailing = match load_mailing_for_export(&mailings, &id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    build_mailing_pdf_response(&id, mailing, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per email of a stored mailing into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
async fn export_mailing_zip(
    State(mailings): State<MailingStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let mailing = match load_mailing_for_export(&mailings, &id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_mailing_zip_response(&id, &mailing, &base_url, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the mailing's print page, and
/// prints it with `chrome`. `chrome` is a parameter so the no-Chrome
/// path is testable.
async fn build_mailing_pdf_response(
    id: &str,
    mut mailing: Mailing,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_email_images(&mut mailing, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = mailing_render::render_mailing_with(
        &mailing,
        mailing_render::RenderOptions {
            is_print: true,
            ..mailing_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, mailing.viewport()).await {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), "mailing exported as pdf");
            file_download(id, "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every email and packs the PNGs. `chrome` is a
/// parameter so the no-Chrome path is testable.
async fn build_mailing_zip_response(
    id: &str,
    mailing: &Mailing,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(mailing.emails.len());
    for index in 0..mailing.emails.len() {
        match screenshots::screenshot_email(mailing, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_email_images(id, &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), email_count = images.len(), "mailing exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Builds one email-client HTML file per email and packs them into a
/// zip with the subjects file. Needs no Chrome.
async fn export_mailing_email_zip(
    State(mailings): State<MailingStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut mailing = match load_mailing_for_export(&mailings, &id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_email_images(&mut mailing, &uploads).await {
        return api_error::internal_error(&error);
    }
    let files = email_html::export_mailing_emails(&mailing);
    match pack_email_html_files(&id, &files) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), email_count = files.len(), "mailing exported as email html zip");
            file_download(&id, "email.zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves email `number` (1-based) of a stored mailing as one
/// email-client HTML page: the same file the email zip packs. The
/// copy button and direct links use it.
pub(crate) async fn email_client_html(
    mailings: &MailingStore,
    uploads: &UploadStore,
    id: &str,
    number: usize,
) -> Response {
    let mut mailing = match load_mailing_for_export(mailings, id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    if number == 0 || number > mailing.emails.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "mailing `{id}` has no email {number}: use 1 to {}",
                mailing.emails.len()
            ),
            Vec::new(),
        );
    }
    if let Err(error) = inline_uploaded_email_images(&mut mailing, uploads).await {
        return api_error::internal_error(&error);
    }
    let mut files = email_html::export_mailing_emails(&mailing);
    let file = files.remove(number - 1);
    Html(file.html).into_response()
}

/// One zip with `{id}-email-{n}.html` per email, 1-based, in order,
/// plus `{id}-subjects.txt` with the subject and preheader lines.
fn pack_email_html_files(id: &str, files: &[email_html::EmailHtmlFile]) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    let mut subjects = String::new();
    for (index, file) in files.iter().enumerate() {
        writer.start_file(format!("{id}-email-{}.html", index + 1), options)?;
        writer.write_all(file.html.as_bytes())?;
        subjects.push_str(&format!(
            "Email {}
Subject: {}
Preheader: {}

",
            index + 1,
            file.subject.as_deref().unwrap_or(""),
            file.preheader.as_deref().unwrap_or("")
        ));
    }
    writer.start_file(format!("{id}-subjects.txt"), options)?;
    writer.write_all(subjects.as_bytes())?;
    Ok(writer.finish()?.into_inner())
}

/// One zip with `{id}-email-{n}.png` per image, 1-based, in order. A
/// PNG is compressed already, so the entries are stored as they are.
fn pack_email_images(id: &str, images: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, image) in images.iter().enumerate() {
        writer.start_file(format!("{id}-email-{}.png", index + 1), options)?;
        writer.write_all(image)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// The mailing twin of `inline_uploaded_images`: rewrites email html
/// and css.
async fn inline_uploaded_email_images(
    mailing: &mut Mailing,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = mailing
        .emails
        .iter()
        .flat_map(|email| [Some(email.html.as_str()), email.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for email in &mut mailing.emails {
        email.html = replace_upload_references(&email.html, &data_uris);
        if let Some(css) = &email.css {
            email.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// Replaces every `/uploads/{name}` reference in screen html and css
/// with a `data:` URI: `<img src>`, `href`, inline `style` backgrounds,
/// and `url()` in the screen CSS. Names that are missing or unsafe stay
/// as written, so the export still succeeds.
async fn inline_uploaded_images(design: &mut Design, uploads: &UploadStore) -> anyhow::Result<()> {
    let texts: Vec<&str> = design
        .screens
        .iter()
        .flat_map(|screen| [Some(screen.html.as_str()), screen.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
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

/// The deck twin of `inline_uploaded_images`: rewrites slide html and
/// css.
async fn inline_uploaded_slide_images(
    deck: &mut Deck,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = deck
        .slides
        .iter()
        .flat_map(|slide| [Some(slide.html.as_str()), slide.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for slide in &mut deck.slides {
        slide.html = replace_upload_references(&slide.html, &data_uris);
        if let Some(css) = &slide.css {
            slide.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// The document twin of `inline_uploaded_images`: rewrites page html
/// and css.
async fn inline_uploaded_page_images(
    document: &mut Document,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = document
        .pages
        .iter()
        .flat_map(|page| [Some(page.html.as_str()), page.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for page in &mut document.pages {
        page.html = replace_upload_references(&page.html, &data_uris);
        if let Some(css) = &page.css {
            page.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// The social twin of `inline_uploaded_images`: rewrites frame html and
/// css.
async fn inline_uploaded_frame_images(
    social: &mut Social,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = social
        .frames
        .iter()
        .flat_map(|frame| [Some(frame.html.as_str()), frame.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for frame in &mut social.frames {
        frame.html = replace_upload_references(&frame.html, &data_uris);
        if let Some(css) = &frame.css {
            frame.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// One `data:` URI per stored upload that `texts` reference. Names that
/// are unsafe or missing get no entry.
async fn collect_data_uris(
    texts: &[&str],
    uploads: &UploadStore,
) -> anyhow::Result<HashMap<String, String>> {
    let mut names: Vec<String> = Vec::new();
    for text in texts {
        collect_upload_names(text, &mut names);
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
    Ok(data_uris)
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
        FONT_LINK_PREFIX, FontLink, attachment_disposition, base64_encode, build_deck_pdf_response,
        build_document_pdf_response, build_pptx_response, build_social_pdf_response,
        build_social_zip_response, google_fonts_link, inline_font_urls, inline_google_fonts,
        inline_uploaded_frame_images, inline_uploaded_images, inline_uploaded_page_images,
        inline_uploaded_slide_images, pack_frame_images, upload_references,
    };
    use crate::pptx::ExportSources;
    use crate::render;
    use crate::test_support::{sample_deck, sample_document, sample_social};
    use crate::uploads::UploadStore;

    #[tokio::test]
    async fn social_exports_without_chrome_are_503() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let response = build_social_pdf_response("launch", sample_social(), &store, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let response =
            build_social_zip_response("launch", &sample_social(), "http://127.0.0.1:3000", None)
                .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn frame_images_pack_as_numbered_pngs() {
        let bytes = pack_frame_images("launch", &[b"PNG1".to_vec(), b"PNG2".to_vec()]).unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 2);
        assert_eq!(archive.by_index(0).unwrap().name(), "launch-frame-1.png");
        assert_eq!(archive.by_index(1).unwrap().name(), "launch-frame-2.png");
        let mut content = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_index(1).unwrap(), &mut content).unwrap();
        assert_eq!(content, b"PNG2");
    }

    #[tokio::test]
    async fn inlines_uploaded_images_in_frames() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("launch", "logo.png", b"PNGDATA").await.unwrap();
        let mut social = sample_social();
        social.frames[0].html = "<img src='/uploads/logo.png'>".to_owned();
        social.frames[0].css = Some(".a { background: url('/uploads/logo.png'); }".to_owned());
        inline_uploaded_frame_images(&mut social, &store)
            .await
            .unwrap();
        let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
        assert!(social.frames[0].html.contains(&expected));
        assert!(social.frames[0].css.as_deref().unwrap().contains(&expected));
        assert!(!social.frames[0].html.contains("/uploads/"));
    }

    #[tokio::test]
    async fn document_pdf_export_without_chrome_is_503() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let response = build_document_pdf_response("report", sample_document(), &store, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn inlines_uploaded_images_in_pages() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("report", "chart.png", b"PNGDATA").await.unwrap();
        let mut document = sample_document();
        document.pages[0].html = "<img src='/uploads/chart.png'>".to_owned();
        document.pages[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
        inline_uploaded_page_images(&mut document, &store)
            .await
            .unwrap();
        let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
        assert!(document.pages[0].html.contains(&expected));
        assert!(
            document.pages[0]
                .css
                .as_deref()
                .unwrap()
                .contains(&expected)
        );
        assert!(!document.pages[0].html.contains("/uploads/"));
    }

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
        assert_eq!(
            attachment_disposition("overview", "pptx"),
            "attachment; filename=\"overview.pptx\""
        );
    }

    #[tokio::test]
    async fn deck_pdf_export_without_chrome_is_503() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let response = build_deck_pdf_response("overview", sample_deck(), &store, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn pptx_export_without_chrome_is_503() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        let sources = ExportSources {
            uploads: &store,
            base_url: "http://127.0.0.1:3000".to_owned(),
        };
        let response = build_pptx_response("overview", &sample_deck(), &sources, None).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.starts_with("PPTX exports need Chrome"));
        assert!(message.contains("SWIFT_DESIGN_CHROME"));
    }

    #[tokio::test]
    async fn inlines_uploaded_images_in_slides() {
        let directory = tempfile::tempdir().unwrap();
        let store = UploadStore::new(directory.path().to_path_buf());
        store.save("talk", "chart.png", b"PNGDATA").await.unwrap();
        let mut deck = sample_deck();
        deck.slides[0].html = "<img src='/uploads/chart.png'>".to_owned();
        deck.slides[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
        inline_uploaded_slide_images(&mut deck, &store)
            .await
            .unwrap();
        let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
        assert!(deck.slides[0].html.contains(&expected));
        assert!(deck.slides[0].css.as_deref().unwrap().contains(&expected));
        assert!(!deck.slides[0].html.contains("/uploads/"));
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
        store.save("talk", "chart.png", b"PNGDATA").await.unwrap();

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
