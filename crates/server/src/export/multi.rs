use super::*;

/// Loads a stored print for an export: 400, 404, or 422 as a response
/// when the id, the file, or the print is not usable.
pub(super) async fn load_print_for_export(
    prints: &PrintStore,
    id: &str,
) -> Result<Print, Response> {
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

/// Query of `GET /prints/{id}/export` and `export.pdf`.
#[derive(Debug, Deserialize)]
pub(super) struct SheetQuery {
    /// Export only this one-based sheet. Without it, every sheet.
    #[serde(default)]
    sheet: Option<usize>,
}

/// Converts a one-based sheet number to a zero-based index. `Err` is
/// the 404 that names the valid range.
pub(super) fn only_sheet_index(
    id: &str,
    print: &Print,
    number: Option<usize>,
) -> Result<Option<usize>, Box<Response>> {
    match number {
        Some(number) if number >= 1 && number <= print.sheets.len() => Ok(Some(number - 1)),
        Some(number) => Err(Box::new(api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "print `{id}` has no sheet {number}: use 1 to {}",
                print.sheets.len()
            ),
            Vec::new(),
        ))),
        None => Ok(None),
    }
}

/// Renders a stored print, or one sheet of it, with uploaded images
/// and theme fonts inlined and returns it as a file download.
pub(super) async fn export_print(
    State(prints): State<PrintStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<SheetQuery>,
) -> Response {
    let mut print = match load_print_for_export(&prints, &id).await {
        Ok(print) => print,
        Err(response) => return response,
    };
    let only_sheet = match only_sheet_index(&id, &print, query.sheet) {
        Ok(only_sheet) => only_sheet,
        Err(response) => return *response,
    };
    if let Err(error) = inline_uploaded_sheet_images(&mut print, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        print_render::render_print_with(
            &print,
            print_render::RenderOptions {
                only_sheet,
                ..print_render::RenderOptions::default()
            },
        ),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "print exported");
    let scope = ExportScope {
        id: &id,
        only: only_sheet,
    };
    file_download(
        &scope.stem("sheet"),
        "html",
        "text/html; charset=utf-8",
        html.into_bytes(),
    )
}

/// Prints a stored print, or one sheet of it, to a PDF with the user's
/// Chrome, one sheet per PDF page, and returns it as a file download.
/// 503 when no Chrome is installed.
pub(super) async fn export_print_pdf(
    State(prints): State<PrintStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<SheetQuery>,
) -> Response {
    let print = match load_print_for_export(&prints, &id).await {
        Ok(print) => print,
        Err(response) => return response,
    };
    let only = match only_sheet_index(&id, &print, query.sheet) {
        Ok(only) => only,
        Err(response) => return *response,
    };
    let scope = ExportScope { id: &id, only };
    build_print_pdf_response(scope, print, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per sheet of a stored print into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
pub(super) async fn export_print_zip(
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
pub(super) async fn build_print_pdf_response(
    scope: ExportScope<'_>,
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
            only_sheet: scope.only,
            ..print_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, print.viewport()).await {
        Ok(bytes) => {
            tracing::info!(id = %scope.id, size_bytes = bytes.len(), "print exported as pdf");
            file_download(&scope.stem("sheet"), "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every sheet and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
pub(super) async fn build_print_zip_response(
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
    match pack_unit_images(id, "sheet", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), sheet_count = images.len(), "print exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// The print twin of `inline_uploaded_images`: rewrites sheet html and
/// css.
pub(super) async fn inline_uploaded_sheet_images(
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
pub(super) async fn load_campaign_for_export(
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

/// Query of `GET /campaigns/{id}/export` and `export.pdf`.
#[derive(Debug, Deserialize)]
pub(super) struct AdQuery {
    /// Export only this one-based ad. Without it, every ad.
    #[serde(default)]
    ad: Option<usize>,
}

/// Converts a one-based ad number to a zero-based index. `Err` is the
/// 404 that names the valid range.
pub(super) fn only_ad_index(
    id: &str,
    campaign: &Campaign,
    number: Option<usize>,
) -> Result<Option<usize>, Box<Response>> {
    match number {
        Some(number) if number >= 1 && number <= campaign.ads.len() => Ok(Some(number - 1)),
        Some(number) => Err(Box::new(api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "campaign `{id}` has no ad {number}: use 1 to {}",
                campaign.ads.len()
            ),
            Vec::new(),
        ))),
        None => Ok(None),
    }
}

/// Renders a stored campaign, or one ad of it, with uploaded images
/// and theme fonts inlined and returns it as a file download.
pub(super) async fn export_campaign(
    State(campaigns): State<CampaignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<AdQuery>,
) -> Response {
    let mut campaign = match load_campaign_for_export(&campaigns, &id).await {
        Ok(campaign) => campaign,
        Err(response) => return response,
    };
    let only_ad = match only_ad_index(&id, &campaign, query.ad) {
        Ok(only_ad) => only_ad,
        Err(response) => return *response,
    };
    if let Err(error) = inline_uploaded_ad_images(&mut campaign, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        campaign_render::render_campaign_with(
            &campaign,
            campaign_render::RenderOptions {
                only_ad,
                ..campaign_render::RenderOptions::default()
            },
        ),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "campaign exported");
    let scope = ExportScope {
        id: &id,
        only: only_ad,
    };
    file_download(
        &scope.stem("ad"),
        "html",
        "text/html; charset=utf-8",
        html.into_bytes(),
    )
}

/// Prints a stored campaign, or one ad of it, to a PDF with the user's
/// Chrome, one ad per PDF page, and returns it as a file download.
/// 503 when no Chrome is installed.
pub(super) async fn export_campaign_pdf(
    State(campaigns): State<CampaignStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<AdQuery>,
) -> Response {
    let campaign = match load_campaign_for_export(&campaigns, &id).await {
        Ok(campaign) => campaign,
        Err(response) => return response,
    };
    let only = match only_ad_index(&id, &campaign, query.ad) {
        Ok(only) => only,
        Err(response) => return *response,
    };
    let scope = ExportScope { id: &id, only };
    build_campaign_pdf_response(scope, campaign, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per ad of a stored campaign into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
pub(super) async fn export_campaign_zip(
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
pub(super) async fn build_campaign_pdf_response(
    scope: ExportScope<'_>,
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
            only_ad: scope.only,
            ..campaign_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, campaign.viewport()).await {
        Ok(bytes) => {
            tracing::info!(id = %scope.id, size_bytes = bytes.len(), "campaign exported as pdf");
            file_download(&scope.stem("ad"), "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every ad and packs the PNGs. `chrome` is a parameter so
/// the no-Chrome path is testable.
pub(super) async fn build_campaign_zip_response(
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
    match pack_unit_images(id, "ad", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), ad_count = images.len(), "campaign exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// The campaign twin of `inline_uploaded_images`: rewrites ad html and
/// css.
pub(super) async fn inline_uploaded_ad_images(
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

/// Loads a stored artwork for an export: 400, 404, or 422 as a
/// response when the id, the file, or the artwork is not usable.
pub(super) async fn load_artwork_for_export(
    artworks: &ArtworkStore,
    id: &str,
) -> Result<Artwork, Response> {
    if !is_valid_artwork_id(id) {
        return Err(api_error::invalid_artwork_id(id));
    }
    let artwork = match artworks.load(id).await {
        Ok(Some(artwork)) => artwork,
        Ok(None) => return Err(api_error::artwork_not_found(id)),
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let errors = artwork.validate();
    if !errors.is_empty() {
        return Err(api_error::artwork_validation_failed(&errors));
    }
    Ok(artwork)
}

/// Query of `GET /artworks/{id}/export` and `export.pdf`.
#[derive(Debug, Deserialize)]
pub(super) struct CoverQuery {
    /// Export only this one-based cover. Without it, every cover.
    #[serde(default)]
    cover: Option<usize>,
}

/// Converts a one-based cover number to a zero-based index. `Err` is
/// the 404 that names the valid range.
pub(super) fn only_cover_index(
    id: &str,
    artwork: &Artwork,
    number: Option<usize>,
) -> Result<Option<usize>, Box<Response>> {
    match number {
        Some(number) if number >= 1 && number <= artwork.covers.len() => Ok(Some(number - 1)),
        Some(number) => Err(Box::new(api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "artwork `{id}` has no cover {number}: use 1 to {}",
                artwork.covers.len()
            ),
            Vec::new(),
        ))),
        None => Ok(None),
    }
}

/// Renders a stored artwork, or one cover of it, with uploaded images
/// and theme fonts inlined and returns it as a file download.
pub(super) async fn export_artwork(
    State(artworks): State<ArtworkStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<CoverQuery>,
) -> Response {
    let mut artwork = match load_artwork_for_export(&artworks, &id).await {
        Ok(artwork) => artwork,
        Err(response) => return response,
    };
    let only_cover = match only_cover_index(&id, &artwork, query.cover) {
        Ok(only_cover) => only_cover,
        Err(response) => return *response,
    };
    if let Err(error) = inline_uploaded_cover_images(&mut artwork, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        artwork_render::render_artwork_with(
            &artwork,
            artwork_render::RenderOptions {
                only_cover,
                ..artwork_render::RenderOptions::default()
            },
        ),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "artwork exported");
    let scope = ExportScope {
        id: &id,
        only: only_cover,
    };
    file_download(
        &scope.stem("cover"),
        "html",
        "text/html; charset=utf-8",
        html.into_bytes(),
    )
}

/// Prints a stored artwork, or one cover of it, to a PDF with the
/// user's Chrome, one cover per PDF page, and returns it as a file
/// download. 503 when no Chrome is installed.
pub(super) async fn export_artwork_pdf(
    State(artworks): State<ArtworkStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<CoverQuery>,
) -> Response {
    let artwork = match load_artwork_for_export(&artworks, &id).await {
        Ok(artwork) => artwork,
        Err(response) => return response,
    };
    let only = match only_cover_index(&id, &artwork, query.cover) {
        Ok(only) => only,
        Err(response) => return *response,
    };
    let scope = ExportScope { id: &id, only };
    build_artwork_pdf_response(scope, artwork, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per cover of a stored artwork into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
pub(super) async fn export_artwork_zip(
    State(artworks): State<ArtworkStore>,
    State(settings): State<SettingsStore>,
    Path(id): Path<String>,
) -> Response {
    let artwork = match load_artwork_for_export(&artworks, &id).await {
        Ok(artwork) => artwork,
        Err(response) => return response,
    };
    let base_url = format!("http://{}", settings.address());
    build_artwork_zip_response(&id, &artwork, &base_url, screenshots::find_chrome()).await
}

/// Inlines uploaded images, renders the artwork's print page, and
/// prints it with `chrome`. `chrome` is a parameter so the no-Chrome
/// path is testable.
pub(super) async fn build_artwork_pdf_response(
    scope: ExportScope<'_>,
    mut artwork: Artwork,
    uploads: &UploadStore,
    chrome: Option<PathBuf>,
) -> Response {
    let Some(chrome) = chrome else {
        return screenshots::chrome_missing_response("PDF exports");
    };
    if let Err(error) = inline_uploaded_cover_images(&mut artwork, uploads).await {
        return api_error::internal_error(&error);
    }
    let html = artwork_render::render_artwork_with(
        &artwork,
        artwork_render::RenderOptions {
            is_print: true,
            only_cover: scope.only,
            ..artwork_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, artwork.viewport()).await {
        Ok(bytes) => {
            tracing::info!(id = %scope.id, size_bytes = bytes.len(), "artwork exported as pdf");
            file_download(&scope.stem("cover"), "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every cover and packs the PNGs. `chrome` is a parameter
/// so the no-Chrome path is testable.
pub(super) async fn build_artwork_zip_response(
    id: &str,
    artwork: &Artwork,
    base_url: &str,
    chrome: Option<PathBuf>,
) -> Response {
    if chrome.is_none() {
        return screenshots::chrome_missing_response("PNG exports");
    }
    let mut images = Vec::with_capacity(artwork.covers.len());
    for index in 0..artwork.covers.len() {
        match screenshots::screenshot_cover(artwork, index, base_url).await {
            Ok(bytes) => images.push(bytes),
            Err(error) => return api_error::internal_error(&error),
        }
    }
    match pack_unit_images(id, "cover", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), cover_count = images.len(), "artwork exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// The artwork twin of `inline_uploaded_images`: rewrites cover html
/// and css.
pub(super) async fn inline_uploaded_cover_images(
    artwork: &mut Artwork,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = artwork
        .covers
        .iter()
        .flat_map(|cover| [Some(cover.html.as_str()), cover.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for cover in &mut artwork.covers {
        cover.html = replace_upload_references(&cover.html, &data_uris);
        if let Some(css) = &cover.css {
            cover.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}
