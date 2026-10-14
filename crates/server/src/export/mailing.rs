use super::*;

/// Loads a stored mailing for an export: 400, 404, or 422 as a
/// response when the id, the file, or the mailing is not usable.
pub(super) async fn load_mailing_for_export(
    mailings: &MailingStore,
    id: &str,
) -> Result<Mailing, Response> {
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

/// Query of `GET /mailings/{id}/export` and `export.pdf`.
#[derive(Debug, Deserialize)]
pub(super) struct EmailQuery {
    /// Export only this one-based email. Without it, every email.
    #[serde(default)]
    email: Option<usize>,
}

/// Converts a one-based email number to a zero-based index. `Err` is
/// the 404 that names the valid range.
pub(super) fn only_email_index(
    id: &str,
    mailing: &Mailing,
    number: Option<usize>,
) -> Result<Option<usize>, Box<Response>> {
    match number {
        Some(number) if number >= 1 && number <= mailing.emails.len() => Ok(Some(number - 1)),
        Some(number) => Err(Box::new(api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "mailing `{id}` has no email {number}: use 1 to {}",
                mailing.emails.len()
            ),
            Vec::new(),
        ))),
        None => Ok(None),
    }
}

/// Renders a stored mailing, or one email of it, with uploaded images
/// and theme fonts inlined and returns it as a file download.
pub(super) async fn export_mailing(
    State(mailings): State<MailingStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<EmailQuery>,
) -> Response {
    let mut mailing = match load_mailing_for_export(&mailings, &id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    let only_email = match only_email_index(&id, &mailing, query.email) {
        Ok(only_email) => only_email,
        Err(response) => return *response,
    };
    if let Err(error) = inline_uploaded_email_images(&mut mailing, &uploads).await {
        return api_error::internal_error(&error);
    }
    let html = inline_google_fonts(
        mailing_render::render_mailing_with(
            &mailing,
            mailing_render::RenderOptions {
                only_email,
                ..mailing_render::RenderOptions::default()
            },
        ),
        fetch_as_browser,
    )
    .await;
    tracing::info!(%id, size_bytes = html.len(), "mailing exported");
    let scope = ExportScope {
        id: &id,
        only: only_email,
    };
    file_download(
        &scope.stem("email"),
        "html",
        "text/html; charset=utf-8",
        html.into_bytes(),
    )
}

/// Prints a stored mailing, or one email of it, to a PDF with the
/// user's Chrome, one email per PDF page, and returns it as a file
/// download. 503 when no Chrome is installed.
pub(super) async fn export_mailing_pdf(
    State(mailings): State<MailingStore>,
    State(uploads): State<UploadStore>,
    Path(id): Path<String>,
    Query(query): Query<EmailQuery>,
) -> Response {
    let mailing = match load_mailing_for_export(&mailings, &id).await {
        Ok(mailing) => mailing,
        Err(response) => return response,
    };
    let only = match only_email_index(&id, &mailing, query.email) {
        Ok(only) => only,
        Err(response) => return *response,
    };
    let scope = ExportScope { id: &id, only };
    build_mailing_pdf_response(scope, mailing, &uploads, screenshots::find_chrome()).await
}

/// Packs one PNG per email of a stored mailing into a zip and returns
/// it as a file download. 503 when no Chrome is installed.
pub(super) async fn export_mailing_zip(
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
pub(super) async fn build_mailing_pdf_response(
    scope: ExportScope<'_>,
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
            only_email: scope.only,
            ..mailing_render::RenderOptions::default()
        },
    );
    match screenshots::print_html_to_pdf(&chrome, &html, mailing.viewport()).await {
        Ok(bytes) => {
            tracing::info!(id = %scope.id, size_bytes = bytes.len(), "mailing exported as pdf");
            file_download(&scope.stem("email"), "pdf", "application/pdf", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Screenshots every email and packs the PNGs. `chrome` is a
/// parameter so the no-Chrome path is testable.
pub(super) async fn build_mailing_zip_response(
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
    match pack_unit_images(id, "email", &images) {
        Ok(bytes) => {
            tracing::info!(%id, size_bytes = bytes.len(), email_count = images.len(), "mailing exported as png zip");
            file_download(id, "zip", "application/zip", bytes)
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Builds one email-client HTML file per email and packs them into a
/// zip with the subjects file. Needs no Chrome.
pub(super) async fn export_mailing_email_zip(
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
