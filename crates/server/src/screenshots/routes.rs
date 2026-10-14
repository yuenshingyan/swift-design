use super::*;

/// The `/designs/{id}/screens/{n}.png`, `/decks/{id}/slides/{n}.png`,
/// `/documents/{id}/pages/{n}.png`, `/socials/{id}/frames/{n}.png`,
/// `/prints/{id}/sheets/{n}.png`, `/mailings/{id}/emails/{n}.png`,
/// `/campaigns/{id}/ads/{n}.png`, and `/artworks/{id}/covers/{n}.png`
/// route table.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/designs/{id}/screens/{file}", get(get_screen_image))
        .route("/decks/{id}/slides/{file}", get(get_slide_image))
        .route("/documents/{id}/pages/{file}", get(get_page_image))
        .route("/socials/{id}/frames/{file}", get(get_frame_image))
        .route("/prints/{id}/sheets/{file}", get(get_sheet_image))
        .route("/mailings/{id}/emails/{file}", get(get_email_image))
        .route("/campaigns/{id}/ads/{file}", get(get_ad_image))
        .route("/artworks/{id}/covers/{file}", get(get_cover_image))
}

/// The 1-based number in a `{n}.png` file name. `None` for any other
/// name.
pub(crate) fn image_number(file: &str) -> Option<usize> {
    file.strip_suffix(".png")
        .and_then(|stem| stem.parse::<usize>().ok())
        .filter(|number| *number >= 1)
}

/// The 1-based number in a `{n}.html` file name. `None` for any other
/// name.
pub(crate) fn email_client_number(file: &str) -> Option<usize> {
    file.strip_suffix(".html")
        .and_then(|stem| stem.parse::<usize>().ok())
        .filter(|number| *number >= 1)
}

/// The 404 for an image file name that is not `{n}.png`. `unit` is
/// `screen` or `slide`.
pub(crate) fn bad_image_name(unit: &str, file: &str) -> Response {
    api_error::error_response(
        StatusCode::NOT_FOUND,
        &format!("no {unit} image `{file}`: use {{n}}.png with n from 1"),
        Vec::new(),
    )
}

/// Serves a PNG of one screen. `file` is `{n}.png` with a 1-based `n`.
pub(crate) async fn get_screen_image(
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
pub(crate) async fn get_slide_image(
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
pub(crate) async fn get_page_image(
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
pub(crate) async fn get_frame_image(
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
pub(crate) async fn get_sheet_image(
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
pub(crate) async fn get_email_image(
    State(mailings): State<MailingStore>,
    State(uploads): State<UploadStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_mailing_id(&id) {
        return api_error::invalid_mailing_id(&id);
    }
    // `{n}.html` on the same route serves the email-client HTML the
    // email zip packs: the copy button fetches it.
    if let Some(number) = email_client_number(&file) {
        return email_client_html(&mailings, &uploads, &id, number).await;
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

/// Serves a PNG of one ad. `file` is `{n}.png` with a 1-based `n`.
pub(crate) async fn get_ad_image(
    State(campaigns): State<CampaignStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_campaign_id(&id) {
        return api_error::invalid_campaign_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("ad", &file);
    };
    let campaign = match campaigns.load(&id).await {
        Ok(Some(campaign)) => campaign,
        Ok(None) => return api_error::campaign_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > campaign.ads.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "campaign `{id}` has no ad {number}: use 1 to {}",
                campaign.ads.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("ad images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_ad(&campaign, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Serves a PNG of one cover. `file` is `{n}.png` with a 1-based `n`.
pub(crate) async fn get_cover_image(
    State(artworks): State<ArtworkStore>,
    State(settings): State<SettingsStore>,
    Path((id, file)): Path<(String, String)>,
) -> Response {
    if !is_valid_artwork_id(&id) {
        return api_error::invalid_artwork_id(&id);
    }
    let Some(number) = image_number(&file) else {
        return bad_image_name("cover", &file);
    };
    let artwork = match artworks.load(&id).await {
        Ok(Some(artwork)) => artwork,
        Ok(None) => return api_error::artwork_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    if number > artwork.covers.len() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "artwork `{id}` has no cover {number}: use 1 to {}",
                artwork.covers.len()
            ),
            Vec::new(),
        );
    }
    if find_chrome().is_none() {
        return chrome_missing_response("cover images");
    }
    let base_url = format!("http://{}", settings.address());
    match screenshot_cover(&artwork, number - 1, &base_url).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "image/png")], bytes).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}
