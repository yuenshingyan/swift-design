use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::task::JoinSet;

use super::*;

/// Optional one-based screen selection for a demo PNG ZIP.
#[derive(Default, Deserialize)]
pub(super) struct DesignZipQuery {
    screens: Option<String>,
}

/// Parses a bounded selection before any screenshots are taken.
pub(super) fn selected_screen_indexes(
    selection: Option<&str>,
    count: usize,
) -> Result<Vec<usize>, String> {
    let Some(selection) = selection else {
        return Ok((0..count).collect());
    };
    if selection.trim().is_empty() {
        return Err("Cannot export selected screens: select at least one screen.".into());
    }
    if selection.len() > 4096 {
        return Err(
            "Cannot export selected screens: the selection is too long; use ranges.".into(),
        );
    }
    let mut indexes = BTreeSet::new();
    for part in selection.split(',') {
        let part = part.trim();
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let number = |value: &str| {
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            value.parse::<usize>().ok()
        };
        let (Some(first), Some(last)) = (number(first), number(last)) else {
            return Err(
                "Cannot export selected screens: use numbers or ranges such as 1,3-5.".into(),
            );
        };
        if first == 0 || first > last || last > count {
            return Err(format!(
                "Cannot export selected screens: choose ascending ranges from 1 to {count}."
            ));
        }
        indexes.extend((first - 1)..last);
    }
    Ok(indexes.into_iter().collect())
}

/// Keeps original screen numbers in ZIP entry names, even for a sparse subset.
pub(super) fn pack_design_images(id: &str, images: &[(usize, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, image) in images {
        writer.start_file(format!("{id}-screen-{}.png", index + 1), options)?;
        writer.write_all(image)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Loads a stored design for an export: 400, 404, or 422 as a response
/// when the id, the file, or the design is not usable.
pub(super) async fn load_design_for_export(
    designs: &DesignStore,
    id: &str,
) -> Result<Design, Response> {
    if !is_valid_design_id(id) {
        return Err(api_error::invalid_design_id(id));
    }
    let design = match designs.load(id).await {
        Ok(Some(design)) => design,
        Ok(None) => return Err(api_error::design_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = design.validate();
    if !errors.is_empty() {
        return Err(api_error::validation_failed(&errors));
    }
    Ok(design)
}

/// Renders a stored design with uploaded images and theme fonts inlined
/// and returns it as a file download.
pub(super) async fn export_design(
    State(designs): State<DesignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
) -> Response {
    let mut design = match load_design_for_export(&designs, &id).await {
        Ok(design) => design,
        Err(response) => return response,
    };
    if let Err(error) = inline_uploaded_images(&mut design, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(render::render_design(&design, false), fetch_as_browser).await;
    tracing::info!(%id, size_bytes = html.len(), "design exported");
    file_download(&id, "html", "text/html; charset=utf-8", html.into_bytes())
}

/// Packs selected saved screens into a PNG ZIP, keeping their original numbers.
/// The optional `screens` query accepts one-based numbers and ranges; omission exports all.
/// Returns 503 when Chrome is unavailable.
pub(super) async fn export_design_zip(
    State(designs): State<DesignStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
    Query(query): Query<DesignZipQuery>,
) -> Response {
    let design = match load_design_for_export(&designs, &id).await {
        Ok(design) => design,
        Err(response) => return response,
    };
    let indexes = match selected_screen_indexes(query.screens.as_deref(), design.screens.len()) {
        Ok(indexes) => indexes,
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_REQUEST, &message, Vec::new());
        }
    };
    let base_url = format!("http://{}", settings.address());
    build_design_zip_response(
        &id,
        &design,
        &base_url,
        screenshots::find_chrome(),
        Some(&indexes),
    )
    .await
}

/// Screenshots only the selected screens (all when omitted) and packs the PNGs.
/// `chrome` is a parameter
/// so the no-Chrome path is testable.
pub(super) async fn build_design_zip_response(
    id: &str,
    design: &Design,
    base_url: &str,
    chrome: Option<PathBuf>,
    indexes: Option<&[usize]>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let indexes = indexes
        .map(Vec::from)
        .unwrap_or_else(|| (0..design.screens.len()).collect());
    let mut images = Vec::with_capacity(indexes.len());
    let mut remaining = indexes.into_iter();
    let design = Arc::new(design.clone());
    let base_url: Arc<str> = Arc::from(base_url);
    let mut captures = JoinSet::new();
    loop {
        // Bound browser memory while avoiding a separate serial browser wait per page.
        while captures.len() < 3 {
            let Some(index) = remaining.next() else { break };
            let design = Arc::clone(&design);
            let base_url = Arc::clone(&base_url);
            captures.spawn(async move {
                (
                    index,
                    screenshots::screenshot_screen(&design, index, &base_url).await,
                )
            });
        }
        match captures.join_next().await {
            Some(Ok((index, Ok(bytes)))) => images.push((index, bytes)),
            Some(Ok((_, Err(error)))) => return api_error::internal_error(&error),
            Some(Err(error)) => return api_error::internal_error(&error.into()),
            None => break,
        }
    }
    images.sort_by_key(|(index, _)| *index);
    match pack_design_images(id, &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), screen_count = images.len(), "design exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Loads a stored deck for an export: 400, 404, or 422 as a response
/// when the id, the file, or the deck is not usable.
pub(super) async fn load_deck_for_export(decks: &DeckStore, id: &str) -> Result<Deck, Response> {
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
pub(super) async fn export_deck(
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
pub(super) async fn export_deck_pdf(
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

/// Packs one PNG per slide of a stored deck into a zip and returns it
/// as a file download. 503 when no Chrome is installed.
pub(super) async fn export_deck_zip(
    State(decks): State<DeckStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let deck = match load_deck_for_export(&decks, &id).await {
        Ok(deck) => deck,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_deck_zip_response(&id, &deck, &base_url, screenshots::find_chrome()).await
}

/// Screenshots every slide and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
pub(super) async fn build_deck_zip_response(
    id: &str,
    deck: &Deck,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(deck.slides.len());
    for index in 0..deck.slides.len() {
        match screenshots::screenshot_slide(deck, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_unit_images(id, "slide", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), slide_count = images.len(), "deck exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Inlines uploaded images, renders the deck print page, and prints it
/// with `chrome`. `chrome` is a parameter so the no-Chrome path is
/// testable.
pub(super) async fn build_deck_pdf_response(
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
pub(super) async fn export_deck_pptx(
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
pub(super) async fn build_pptx_response(
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
pub(super) async fn load_document_for_export(
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
pub(super) async fn export_document(
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
pub(super) async fn export_document_pdf(
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

/// Packs one PNG per page of a stored document into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
pub(super) async fn export_document_zip(
    State(documents): State<DocumentStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let document = match load_document_for_export(&documents, &id).await {
        Ok(document) => document,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_document_zip_response(&id, &document, &base_url, screenshots::find_chrome()).await
}

/// Screenshots every page and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
pub(super) async fn build_document_zip_response(
    id: &str,
    document: &Document,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(document.pages.len());
    for index in 0..document.pages.len() {
        match screenshots::screenshot_page(document, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_unit_images(id, "page", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), page_count = images.len(), "document exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Builds a Word file from a stored document and returns it as a file
/// download. Needs no Chrome.
pub(super) async fn export_document_docx(
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
pub(super) async fn build_document_pdf_response(
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
pub(super) async fn load_social_for_export(
    socials: &SocialStore,
    id: &str,
) -> Result<Social, Response> {
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

/// Query of `GET /socials/{id}/export` and `export.pdf`.
#[derive(Debug, Deserialize)]
pub(super) struct FrameQuery {
    /// Export only this one-based frame. Without it, every frame.
    #[serde(default)]
    frame: Option<usize>,
}

/// Converts a one-based frame number to a zero-based index. `Err` is
/// the 404 that names the valid range.
pub(super) fn only_frame_index(
    id: &str,
    social: &Social,
    number: Option<usize>,
) -> Result<Option<usize>, Box<Response>> {
    match number {
        Some(number) if number >= 1 && number <= social.frames.len() => Ok(Some(number - 1)),
        Some(number) => Err(Box::new(api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "social `{id}` has no frame {number}: use 1 to {}",
                social.frames.len()
            ),
            Vec::new(),
        ))),
        None => Ok(None),
    }
}

/// Renders a stored social, or one frame of it, with uploaded images
/// and theme fonts inlined and returns it as a file download.
pub(super) async fn export_social(
    State(socials): State<SocialStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<FrameQuery>,
) -> Response {
    let mut social = match load_social_for_export(&socials, &id).await {
        Ok(social) => social,
        Err(response) => return response,
    };
    let only_frame = match only_frame_index(&id, &social, query.frame) {
        Ok(only_frame) => only_frame,
        Err(response) => return *response,
    };
    if let Err(error) = inline_uploaded_frame_images(&mut social, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        social_render::render_social_with(
            &social,
            social_render::RenderOptions {
                only_frame,
                ..social_render::RenderOptions::default()
            },
        ),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "social exported");
    let scope = ExportScope {
        id: &id,
        only: only_frame,
    };
    file_download(
        &scope.stem("frame"),
        "html",
        "text/html; charset=utf-8",
        html.into_bytes(),
    )
}

/// Prints a stored social, or one frame of it, to a PDF with the
/// user's Chrome, one frame per sheet, and returns it as a file
/// download. 503 when no Chrome is installed.
pub(super) async fn export_social_pdf(
    State(socials): State<SocialStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<FrameQuery>,
) -> Response {
    let social = match load_social_for_export(&socials, &id).await {
        Ok(social) => social,
        Err(response) => return response,
    };
    let only = match only_frame_index(&id, &social, query.frame) {
        Ok(only) => only,
        Err(response) => return *response,
    };
    let scope = ExportScope { id: &id, only };
    build_social_pdf_response(scope, social, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per frame of a stored social into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
pub(super) async fn export_social_zip(
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
pub(super) async fn build_social_pdf_response(
    scope: ExportScope<'_>,
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
            only_frame: scope.only,
            ..social_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, social.viewport()).await {
        Ok(bytes) => {
            tracing::info!(id = %scope.id, size_bytes = bytes.len(), "social exported as pdf");
            file_download(&scope.stem("frame"), "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every frame and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
pub(super) async fn build_social_zip_response(
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
    match pack_unit_images(id, "frame", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), frame_count = images.len(), "social exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}
