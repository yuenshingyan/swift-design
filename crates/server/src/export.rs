//! Design export: one self-contained file the user can download.
//!
//! `GET /designs/{id}/export` renders the design like the render route,
//! but first inlines every image element and screen background image
//! that points at `/uploads/{name}` as a `data:` URI, then fetches the
//! Google Fonts stylesheet and every font file it names and inlines
//! them too, so the exported file opens offline. Images referenced
//! inside text fragments are not rewritten. When the font fetch fails,
//! the export keeps the online `<link>`. `GET /designs/{id}/export.zip`
//! packs one PNG per screen. A design has no PDF export: a demo is
//! clicked, and a print loses its flows and widgets.
//!
//! Decks export the same way under `/decks/{id}/export`, and
//! `GET /decks/{id}/export.pdf` prints the deck with the user's Chrome,
//! one slide per page. `GET /decks/{id}/export.zip` packs one PNG per
//! slide. `GET /decks/{id}/export.pptx` builds a PowerPoint file from
//! the measured slides; see `pptx.rs`.
//!
//! Documents export under `/documents/{id}/export`, and
//! `GET /documents/{id}/export.pdf` prints the document with the user's
//! Chrome, one page per sheet of its paper. `GET
//! /documents/{id}/export.zip` packs one PNG per page. `GET
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
//!
//! Artworks export under `/artworks/{id}/export`, and
//! `GET /artworks/{id}/export.pdf` prints the artwork with the user's
//! Chrome, one cover per PDF page. `GET /artworks/{id}/export.zip`
//! packs one PNG per cover: the files the platforms take.
//!
//! The HTML and PDF exports of the five multi-unit kinds take an
//! optional one-based unit query (`?frame=N`, `?sheet=N`, `?email=N`,
//! `?ad=N`, `?cover=N`) that exports that unit alone. The download is
//! then named after the unit, like the zip entries.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use design_model::{
    Artwork, Campaign, DECK_VIEWPORT, Deck, Design, Document, Mailing, Print, Social,
};
use serde::Deserialize;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::AppState;
use crate::api_error;
use crate::artwork_render;
use crate::artworks::{ArtworkStore, is_valid_artwork_id};
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
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/designs/{id}/export", get(export_design))
        .route("/designs/{id}/export.zip", get(export_design_zip))
        .route("/decks/{id}/export", get(export_deck))
        .route("/decks/{id}/export.pdf", get(export_deck_pdf))
        .route("/decks/{id}/export.zip", get(export_deck_zip))
        .route("/decks/{id}/export.pptx", get(export_deck_pptx))
        .route("/documents/{id}/export", get(export_document))
        .route("/documents/{id}/export.pdf", get(export_document_pdf))
        .route("/documents/{id}/export.zip", get(export_document_zip))
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
        .route("/artworks/{id}/export", get(export_artwork))
        .route("/artworks/{id}/export.pdf", get(export_artwork_pdf))
        .route("/artworks/{id}/export.zip", get(export_artwork_zip))
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

/// Which part of an artifact an export covers: the whole artifact, or
/// one zero-based unit.
struct ExportScope<'a> {
    /// The artifact id.
    id: &'a str,
    /// The zero-based unit to export alone. `None` exports every unit.
    only: Option<usize>,
}

impl ExportScope<'_> {
    /// The download file stem: `{id}` for the whole artifact,
    /// `{id}-{unit}-{n}` for one unit, matching the zip entry names.
    fn stem(&self, unit: &str) -> String {
        match self.only {
            Some(index) => format!("{}-{unit}-{}", self.id, index + 1),
            None => self.id.to_owned(),
        }
    }
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

mod core;
use core::*;

mod multi;
use multi::*;

mod mailing;
pub(crate) use mailing::email_client_html;
use mailing::*;

mod utils;
pub(crate) use utils::base64_encode;
use utils::*;

#[cfg(test)]
mod tests;
