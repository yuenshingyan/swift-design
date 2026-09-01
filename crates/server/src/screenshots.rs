//! Screen, slide, page, frame, sheet, and email screenshots: how a
//! model sees its own work.
//!
//! The server renders one screen, slide, page, or frame to HTML and
//! asks an installed Chrome or Chromium to draw it as a PNG. No browser
//! crate is shipped: the binary is the user's own. `GET
//! /designs/{id}/screens/{n}.png`, `GET /decks/{id}/slides/{n}.png`,
//! `GET /documents/{id}/pages/{n}.png`, and `GET
//! /socials/{id}/frames/{n}.png` serve a screenshot to external agents,
//! and the polish pass sends the images to vision-capable models.
//! Without Chrome, everything falls back to text.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use design_model::{
    Campaign, DECK_VIEWPORT, Deck, Design, Document, Mailing, Print, Social, Viewport,
};

use crate::api_error;
use crate::campaign_render;
use crate::deck_render;
use crate::decks::{DeckStore, is_valid_deck_id};
use crate::designs::{DesignStore, is_valid_design_id};
use crate::document_render;
use crate::documents::{DocumentStore, is_valid_document_id};
use crate::mailing_render;
use crate::mailings::{MailingStore, is_valid_mailing_id};
use crate::print_render;
use crate::prints::{PrintStore, is_valid_print_id};
use crate::render::{RenderOptions, render_design_with};
use crate::settings::SettingsStore;
use crate::social_render;
use crate::socials::{SocialStore, is_valid_social_id};

/// Environment variable that names the Chrome binary to use.
pub const CHROME_ENVIRONMENT_VARIABLE: &str = "SWIFT_DESIGN_CHROME";

/// Longest time to wait for one screenshot.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(40);

/// Longest time to wait for one PDF: a whole design with its fonts and
/// images.
const PDF_TIMEOUT: Duration = Duration::from_secs(90);

/// Virtual time Chrome waits for fonts and images before it prints. A
/// PDF carries every screen, so it gets more than a screenshot.
const PDF_VIRTUAL_TIME_BUDGET_MS: u32 = 5000;

/// The Chrome window size for `viewport`: one pixel per logical px.
fn window_size(viewport: Viewport) -> String {
    format!("{},{}", viewport.width, viewport.height)
}

/// Most screens the polish pass photographs per round.
pub const POLISH_IMAGE_LIMIT: usize = 12;

/// Known Chrome locations on macOS and Linux, checked in order.
const KNOWN_PATHS: [&str; 8] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
];

/// Command names looked up on `PATH` after the known locations.
const PATH_NAMES: [&str; 5] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The Chrome binary to use: `SWIFT_DESIGN_CHROME`, a known location, or
/// a name on `PATH`. `None` when nothing is installed.
pub fn find_chrome() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(CHROME_ENVIRONMENT_VARIABLE) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    for path in KNOWN_PATHS {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let search_path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&search_path) {
        for name in PATH_NAMES {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// True when the model can read images, judged from its name.
pub fn supports_vision(model: &str) -> bool {
    let name = model.to_ascii_lowercase();
    [
        "gpt-4o",
        "gpt-4.1",
        "gpt-5",
        "o3",
        "o4",
        "claude",
        "gemini",
        "llava",
        "vision",
        "pixtral",
        "qwen2.5-vl",
        "qwen-vl",
        "gemma3",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

/// Renders screen `index` (zero-based) of `design` to a PNG. `base_url`
/// resolves relative image paths like `/uploads/…`.
pub async fn screenshot_screen(
    design: &Design,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = render_design_with(
        design,
        RenderOptions {
            only_screen: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..RenderOptions::default()
        },
    );
    screenshot_html(&chrome, &with_base_href(&html, base_url), design.viewport).await
}

/// Renders the whole design with the layout audit script and returns the
/// DOM after the audit ran, as Chrome dumps it. The polish pass reads
/// the findings out of it.
pub async fn dump_design_dom(design: &Design, base_url: &str) -> anyhow::Result<String> {
    let html = render_design_with(
        design,
        RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, design.viewport).await
}

/// Renders slide `index` (zero-based) of `deck` to a PNG. `base_url`
/// resolves relative image paths like `/uploads/…`.
pub async fn screenshot_slide(
    deck: &Deck,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = deck_render::render_deck_with(
        deck,
        deck_render::RenderOptions {
            only_slide: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..deck_render::RenderOptions::default()
        },
    );
    screenshot_html(&chrome, &with_base_href(&html, base_url), DECK_VIEWPORT).await
}

/// Renders the whole deck with the layout audit script and returns the
/// DOM after the audit ran, as Chrome dumps it.
pub async fn dump_deck_dom(deck: &Deck, base_url: &str) -> anyhow::Result<String> {
    let html = deck_render::render_deck_with(
        deck,
        deck_render::RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..deck_render::RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, DECK_VIEWPORT).await
}

/// Renders page `index` (zero-based) of `document` to a PNG. `base_url`
/// resolves relative image paths like `/uploads/…`.
pub async fn screenshot_page(
    document: &Document,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = document_render::render_document_with(
        document,
        document_render::RenderOptions {
            only_page: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..document_render::RenderOptions::default()
        },
    );
    screenshot_html(
        &chrome,
        &with_base_href(&html, base_url),
        document.viewport(),
    )
    .await
}

/// Renders the whole document with the layout audit script and returns
/// the DOM after the audit ran, as Chrome dumps it.
pub async fn dump_document_dom(document: &Document, base_url: &str) -> anyhow::Result<String> {
    let html = document_render::render_document_with(
        document,
        document_render::RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..document_render::RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, document.viewport()).await
}

/// Renders frame `index` (zero-based) of `social` to a PNG. `base_url`
/// resolves relative image paths like `/uploads/…`.
pub async fn screenshot_frame(
    social: &Social,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = social_render::render_social_with(
        social,
        social_render::RenderOptions {
            only_frame: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..social_render::RenderOptions::default()
        },
    );
    screenshot_html(&chrome, &with_base_href(&html, base_url), social.viewport()).await
}

/// Renders the whole social with the layout audit script and returns
/// the DOM after the audit ran, as Chrome dumps it.
pub async fn dump_social_dom(social: &Social, base_url: &str) -> anyhow::Result<String> {
    let html = social_render::render_social_with(
        social,
        social_render::RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..social_render::RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, social.viewport()).await
}

/// Renders sheet `index` (zero-based) of `print` to a PNG. `base_url`
/// resolves relative image paths like `/uploads/…`.
pub async fn screenshot_sheet(
    print: &Print,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = print_render::render_print_with(
        print,
        print_render::RenderOptions {
            only_sheet: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..print_render::RenderOptions::default()
        },
    );
    screenshot_html(&chrome, &with_base_href(&html, base_url), print.viewport()).await
}

/// Renders the whole print with the layout audit script and returns
/// the DOM after the audit ran, as Chrome dumps it.
pub async fn dump_print_dom(print: &Print, base_url: &str) -> anyhow::Result<String> {
    let html = print_render::render_print_with(
        print,
        print_render::RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..print_render::RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, print.viewport()).await
}

/// Renders email `index` (zero-based) of `mailing` to a PNG.
/// `base_url` resolves relative image paths like `/uploads/…`.
pub async fn screenshot_email(
    mailing: &Mailing,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = mailing_render::render_mailing_with(
        mailing,
        mailing_render::RenderOptions {
            only_email: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..mailing_render::RenderOptions::default()
        },
    );
    screenshot_html(
        &chrome,
        &with_base_href(&html, base_url),
        mailing.viewport(),
    )
    .await
}

/// Renders the whole mailing with the layout audit script and returns
/// the DOM after the audit ran, as Chrome dumps it.
pub async fn dump_mailing_dom(mailing: &Mailing, base_url: &str) -> anyhow::Result<String> {
    let html = mailing_render::render_mailing_with(
        mailing,
        mailing_render::RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..mailing_render::RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, mailing.viewport()).await
}

/// Renders ad `index` (zero-based) of `campaign` to a PNG.
/// `base_url` resolves relative image paths like `/uploads/…`.
pub async fn screenshot_ad(
    campaign: &Campaign,
    index: usize,
    base_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let html = campaign_render::render_campaign_with(
        campaign,
        campaign_render::RenderOptions {
            only_ad: Some(index),
            asset_origin: Some(base_url.to_owned()),
            ..campaign_render::RenderOptions::default()
        },
    );
    screenshot_html(
        &chrome,
        &with_base_href(&html, base_url),
        campaign.viewport(),
    )
    .await
}

/// Renders the whole campaign with the layout audit script and returns
/// the DOM after the audit ran, as Chrome dumps it.
pub async fn dump_campaign_dom(campaign: &Campaign, base_url: &str) -> anyhow::Result<String> {
    let html = campaign_render::render_campaign_with(
        campaign,
        campaign_render::RenderOptions {
            is_auditing: true,
            asset_origin: Some(base_url.to_owned()),
            ..campaign_render::RenderOptions::default()
        },
    );
    dump_rendered_dom(&html, base_url, campaign.viewport()).await
}

/// Loads a rendered page in Chrome and returns the DOM after its
/// scripts ran. `base_url` resolves relative image paths; `viewport`
/// sizes the window.
pub async fn dump_rendered_dom(
    html: &str,
    base_url: &str,
    viewport: Viewport,
) -> anyhow::Result<String> {
    let chrome = find_chrome().ok_or_else(|| {
        anyhow::anyhow!(
            "no Chrome or Chromium found: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        )
    })?;
    let (html_path, _) = scratch_paths("-audit", "dom").await?;
    tokio::fs::write(&html_path, with_base_href(html, base_url)).await?;
    let result = run_chrome_dump(&chrome, &file_url(&html_path), viewport).await;
    let _ = tokio::fs::remove_file(&html_path).await;
    result
}

/// Opens `url` in Chrome and returns the DOM after scripts ran. The
/// URL is a web address; the caller checks it first.
pub(crate) async fn dump_url(
    chrome: &std::path::Path,
    url: &str,
    viewport: Viewport,
) -> anyhow::Result<String> {
    run_chrome_dump(chrome, url, viewport).await
}

/// Opens `url` in Chrome and returns a PNG of the first viewport.
pub(crate) async fn screenshot_url(
    chrome: &std::path::Path,
    url: &str,
    viewport: Viewport,
) -> anyhow::Result<Vec<u8>> {
    let (_, png_path) = scratch_paths("-capture", "png").await?;
    run_chrome(chrome, url, &png_path, viewport).await?;
    let bytes = tokio::fs::read(&png_path).await?;
    let _ = tokio::fs::remove_file(&png_path).await;
    Ok(bytes)
}

/// The `file://` URL of a scratch file.
fn file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

/// Creates the scratch directory and returns a fresh pair of paths in
/// it: `{stamp}{suffix}.html` and `{stamp}.{extension}`. Nothing is
/// written yet.
async fn scratch_paths(suffix: &str, extension: &str) -> anyhow::Result<(PathBuf, PathBuf)> {
    let directory = std::env::temp_dir().join("swift-design-screenshots");
    tokio::fs::create_dir_all(&directory).await?;
    let stamp = format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    Ok((
        directory.join(format!("{stamp}{suffix}.html")),
        directory.join(format!("{stamp}.{extension}")),
    ))
}

/// A headless Chrome command with the flags every run shares. The
/// caller adds the output flag and the URL.
fn chrome_command(
    chrome: &std::path::Path,
    headless_flag: &str,
    virtual_time_budget_ms: u32,
    viewport: Viewport,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(chrome);
    command
        .arg(headless_flag)
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--virtual-time-budget={virtual_time_budget_ms}"))
        .arg(format!("--window-size={}", window_size(viewport)))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    command
}

/// Runs Chrome headless with `--dump-dom` and returns the serialized
/// page after scripts ran.
async fn run_chrome_dump(
    chrome: &std::path::Path,
    url: &str,
    viewport: Viewport,
) -> anyhow::Result<String> {
    for headless_flag in ["--headless=new", "--headless"] {
        let command = chrome_command(chrome, headless_flag, 2500, viewport)
            .arg("--dump-dom")
            .arg(url)
            .stdout(std::process::Stdio::piped())
            .output();
        let output = tokio::time::timeout(SCREENSHOT_TIMEOUT, command)
            .await
            .map_err(|_| {
                anyhow::anyhow!("Chrome did not finish within {SCREENSHOT_TIMEOUT:?}")
            })??;
        if output.status.success() && !output.stdout.is_empty() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }
    anyhow::bail!("Chrome did not dump the page {url}")
}

/// Writes `html` to a temp file, prints it to a PDF with Chrome, and
/// reads the PDF back. The page must carry its own `@page` rules; theme
/// fonts load from Google Fonts when the machine is online, and fall
/// back to the system stack otherwise.
pub async fn print_html_to_pdf(
    chrome: &std::path::Path,
    html: &str,
    viewport: Viewport,
) -> anyhow::Result<Vec<u8>> {
    let (html_path, pdf_path) = scratch_paths("-print", "pdf").await?;
    tokio::fs::write(&html_path, html).await?;
    let result = run_chrome_print(chrome, &html_path, &pdf_path, viewport).await;
    let _ = tokio::fs::remove_file(&html_path).await;
    result?;
    let bytes = tokio::fs::read(&pdf_path).await?;
    let _ = tokio::fs::remove_file(&pdf_path).await;
    Ok(bytes)
}

/// Runs Chrome headless with `--print-to-pdf`. Tries the new headless
/// mode first, then the old flag for older builds.
async fn run_chrome_print(
    chrome: &std::path::Path,
    html_path: &std::path::Path,
    pdf_path: &std::path::Path,
    viewport: Viewport,
) -> anyhow::Result<()> {
    let url = format!("file://{}", html_path.display());
    let print_flag = format!("--print-to-pdf={}", pdf_path.display());
    for headless_flag in ["--headless=new", "--headless"] {
        let command = chrome_command(chrome, headless_flag, PDF_VIRTUAL_TIME_BUDGET_MS, viewport)
            .arg("--no-pdf-header-footer")
            .arg(&print_flag)
            .arg(&url)
            .stdout(std::process::Stdio::null())
            .output();
        let output = tokio::time::timeout(PDF_TIMEOUT, command)
            .await
            .map_err(|_| anyhow::anyhow!("Chrome did not finish within {PDF_TIMEOUT:?}"))??;
        if output.status.success() && pdf_path.is_file() {
            return Ok(());
        }
    }
    anyhow::bail!("Chrome did not write a PDF for {}", html_path.display())
}

/// The 503 response for a Chrome-backed feature on a machine without
/// Chrome. `feature` is plural, like `screen images`.
pub fn chrome_missing_response(feature: &str) -> Response {
    api_error::error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        &format!(
            "{feature} need Chrome or Chromium on the server machine: install one or set {CHROME_ENVIRONMENT_VARIABLE}"
        ),
        Vec::new(),
    )
}

/// Inserts `<base href>` after `<head>`, so relative paths resolve
/// against the server when the page loads from a file.
pub fn with_base_href(html: &str, base_url: &str) -> String {
    let base = format!("<base href=\"{}/\">", base_url.trim_end_matches('/'));
    match html.find("<head>") {
        Some(position) => {
            let split = position + "<head>".len();
            format!("{}\n{base}{}", &html[..split], &html[split..])
        }
        None => format!("{base}{html}"),
    }
}

/// Writes `html` to a temp file, screenshots it with Chrome, and reads
/// the PNG back.
async fn screenshot_html(
    chrome: &std::path::Path,
    html: &str,
    viewport: Viewport,
) -> anyhow::Result<Vec<u8>> {
    let (html_path, png_path) = scratch_paths("", "png").await?;
    tokio::fs::write(&html_path, html).await?;
    let result = run_chrome(chrome, &file_url(&html_path), &png_path, viewport).await;
    let _ = tokio::fs::remove_file(&html_path).await;
    result?;
    let bytes = tokio::fs::read(&png_path).await?;
    let _ = tokio::fs::remove_file(&png_path).await;
    Ok(bytes)
}

/// Runs Chrome headless once. Tries the new headless mode first, then
/// the old flag for older builds.
async fn run_chrome(
    chrome: &std::path::Path,
    url: &str,
    png_path: &std::path::Path,
    viewport: Viewport,
) -> anyhow::Result<()> {
    let screenshot_flag = format!("--screenshot={}", png_path.display());
    for headless_flag in ["--headless=new", "--headless"] {
        let command = chrome_command(chrome, headless_flag, 1200, viewport)
            .arg(&screenshot_flag)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .output();
        let output = tokio::time::timeout(SCREENSHOT_TIMEOUT, command)
            .await
            .map_err(|_| {
                anyhow::anyhow!("Chrome did not finish within {SCREENSHOT_TIMEOUT:?}")
            })??;
        if output.status.success() && png_path.is_file() {
            return Ok(());
        }
    }
    anyhow::bail!("Chrome did not write a screenshot for {url}")
}

/// The `/designs/{id}/screens/{n}.png`, `/decks/{id}/slides/{n}.png`,
/// `/documents/{id}/pages/{n}.png`, `/socials/{id}/frames/{n}.png`,
/// `/prints/{id}/sheets/{n}.png`, and `/mailings/{id}/emails/{n}.png`
/// route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/designs/{id}/screens/{file}", get(get_screen_image))
        .route("/decks/{id}/slides/{file}", get(get_slide_image))
        .route("/documents/{id}/pages/{file}", get(get_page_image))
        .route("/socials/{id}/frames/{file}", get(get_frame_image))
        .route("/prints/{id}/sheets/{file}", get(get_sheet_image))
        .route("/mailings/{id}/emails/{file}", get(get_email_image))
}

/// The 1-based number in a `{n}.png` file name. `None` for any other
/// name.
fn image_number(file: &str) -> Option<usize> {
    file.strip_suffix(".png")
        .and_then(|stem| stem.parse::<usize>().ok())
        .filter(|number| *number >= 1)
}

/// The 1-based number in a `{n}.html` file name. `None` for any other
/// name.
fn email_client_number(file: &str) -> Option<usize> {
    file.strip_suffix(".html")
        .and_then(|stem| stem.parse::<usize>().ok())
        .filter(|number| *number >= 1)
}

/// The 404 for an image file name that is not `{n}.png`. `unit` is
/// `screen` or `slide`.
fn bad_image_name(unit: &str, file: &str) -> Response {
    api_error::error_response(
        StatusCode::NOT_FOUND,
        &format!("no {unit} image `{file}`: use {{n}}.png with n from 1"),
        Vec::new(),
    )
}

/// Serves a PNG of one screen. `file` is `{n}.png` with a 1-based `n`.
async fn get_screen_image(
    State(designs): State<DesignStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_design_id(&id) {
        return api_error::invalid_design_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("screen", &file);
    };
    let design = match designs.load(&id).await {
        Ok(Some(design)) => design,
        Ok(None) => return api_error::design_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > design.screens.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "design `{id}` has no screen {number}: use 1 to {}",
                design.screens.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("screen images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_screen(&design, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves a PNG of one slide. `file` is `{n}.png` with a 1-based `n`.
async fn get_slide_image(
    State(decks): State<DeckStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_deck_id(&id) {
        return api_error::invalid_deck_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("slide", &file);
    };
    let deck = match decks.load(&id).await {
        Ok(Some(deck)) => deck,
        Ok(None) => return api_error::deck_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > deck.slides.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "deck `{id}` has no slide {number}: use 1 to {}",
                deck.slides.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("slide images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_slide(&deck, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves a PNG of one page. `file` is `{n}.png` with a 1-based `n`.
async fn get_page_image(
    State(documents): State<DocumentStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_document_id(&id) {
        return api_error::invalid_document_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("page", &file);
    };
    let document = match documents.load(&id).await {
        Ok(Some(document)) => document,
        Ok(None) => return api_error::document_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > document.pages.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "document `{id}` has no page {number}: use 1 to {}",
                document.pages.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("page images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_page(&document, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves a PNG of one frame. `file` is `{n}.png` with a 1-based `n`.
async fn get_frame_image(
    State(socials): State<SocialStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_social_id(&id) {
        return api_error::invalid_social_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("frame", &file);
    };
    let social = match socials.load(&id).await {
        Ok(Some(social)) => social,
        Ok(None) => return api_error::social_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > social.frames.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "social `{id}` has no frame {number}: use 1 to {}",
                social.frames.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("frame images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_frame(&social, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves a PNG of one sheet. `file` is `{n}.png` with a 1-based `n`.
async fn get_sheet_image(
    State(prints): State<PrintStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_print_id(&id) {
        return api_error::invalid_print_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("sheet", &file);
    };
    let print = match prints.load(&id).await {
        Ok(Some(print)) => print,
        Ok(None) => return api_error::print_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > print.sheets.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "print `{id}` has no sheet {number}: use 1 to {}",
                print.sheets.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("sheet images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_sheet(&print, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves a PNG of one email. `file` is `{n}.png` with a 1-based `n`.
async fn get_email_image(
    State(mailings): State<MailingStore>,
    State(uploads): State<crate::uploads::UploadStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_mailing_id(&id) {
        return api_error::invalid_mailing_id(&id);
    }
    // `{n}.html` on the same route serves the email-client HTML the
    // email zip packs: the copy button fetches it.
    if let Some(number) = email_client_number(&file) {
        return crate::export::email_client_html(&mailings, &uploads, &id, number).await;
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("email", &file);
    };
    let mailing = match mailings.load(&id).await {
        Ok(Some(mailing)) => mailing,
        Ok(None) => return api_error::mailing_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > mailing.emails.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "mailing `{id}` has no email {number}: use 1 to {}",
                mailing.emails.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("email images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_email(&mailing, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_numbers_come_from_png_names() {
        assert_eq!(image_number("1.png"), Some(1));
        assert_eq!(image_number("12.png"), Some(12));
        assert_eq!(image_number("0.png"), None);
        assert_eq!(image_number("1.jpg"), None);
        assert_eq!(image_number("cover.png"), None);
    }

    #[test]
    fn window_sizes_follow_the_viewport() {
        assert_eq!(window_size(Viewport::default()), "1440,900");
        assert_eq!(
            window_size(Viewport {
                width: 390,
                height: 844
            }),
            "390,844"
        );
    }

    #[test]
    fn vision_support_follows_the_model_name() {
        assert!(supports_vision("gpt-5-mini"));
        assert!(supports_vision("claude-sonnet-5"));
        assert!(supports_vision("google/gemini-2.5-flash"));
        assert!(!supports_vision("llama-3.3-70b-versatile"));
        assert!(!supports_vision("deepseek/deepseek-chat"));
    }

    #[test]
    fn base_href_lands_inside_head() {
        let html = "<!doctype html>\n<html><head>\n<title>x</title></head><body></body></html>";
        let result = with_base_href(html, "http://127.0.0.1:3000/");
        assert!(result.contains("<head>\n<base href=\"http://127.0.0.1:3000/\">"));
        assert!(with_base_href("<p>x</p>", "http://h").starts_with("<base href=\"http://h/\">"));
    }

    #[tokio::test]
    async fn scratch_paths_share_a_stem_and_carry_the_extension() {
        let (html_path, pdf_path) = scratch_paths("-print", "pdf")
            .await
            .unwrap_or_else(|error| {
                panic!("scratch paths: {error}");
            });
        assert!(html_path.to_string_lossy().ends_with("-print.html"));
        assert!(pdf_path.to_string_lossy().ends_with(".pdf"));
        assert_eq!(html_path.parent(), pdf_path.parent());
        let html_stem = html_path.to_string_lossy().replace("-print.html", "");
        let pdf_stem = pdf_path.to_string_lossy().replace(".pdf", "");
        assert_eq!(html_stem, pdf_stem);
    }

    #[tokio::test]
    async fn chrome_missing_response_uses_the_shared_shape() {
        let response = chrome_missing_response("PDF exports");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("body: {error}"));
        let body: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("json: {error}"));
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(message.starts_with("PDF exports need Chrome"));
        assert!(message.contains(CHROME_ENVIRONMENT_VARIABLE));
    }
}
