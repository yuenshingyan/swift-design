use super::*;

/// Makes a template from a website or the user's files: the model
/// reads the material and answers with a theme and a style note.
pub(super) async fn extract_template(
    State(store): State<TemplateStore>,
    State(uploads): State<UploadStore>,
    State(settings): State<SettingsStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<ExtractRequest>,
) -> Response {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > NAME_LIMIT {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("template name must be 1 to {NAME_LIMIT} characters"),
            Vec::new(),
        );
    }
    let url = request
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());
    if url.is_none() == request.uploads.is_empty() {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            "name exactly one source: `url` or `uploads`",
            Vec::new(),
        );
    }
    // The model the runner would use: the studio settings, else the
    // environment.
    let configuration = match settings.read().await {
        Ok(Some(stored)) => configuration_from_settings(&stored),
        Ok(None) => None,
        Err(error) => return api_error::internal_error(&error),
    }
    .or_else(configured_model);
    let Some(configuration) = configuration else {
        return api_error::error_response(
            StatusCode::CONFLICT,
            "choose a model in the studio settings before extracting a template",
            Vec::new(),
        );
    };
    let material = match url {
        Some(url) => material_from_url(url).await,
        None => {
            let scope = request.scope.as_deref().unwrap_or(DRAFT_SCOPE);
            material_from_uploads(&uploads, scope, &request.uploads).await
        }
    };
    let material = match material {
        Ok(material) => material,
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_REQUEST, &message, Vec::new());
        }
    };
    let http = match ModelClient::build_http_client() {
        Ok(http) => http,
        Err(message) => {
            return api_error::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &message,
                Vec::new(),
            );
        }
    };
    let client = ModelClient::new(configuration, Some(settings));
    let style = match extract_style(&client, &http, &material).await {
        Ok(style) => style,
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_GATEWAY, &message, Vec::new());
        }
    };
    let template = Template {
        id: template_id(name),
        name: name.to_owned(),
        saved_at: rfc3339_now(),
        source_design: material.source.clone(),
        theme: style.theme,
        viewport: Viewport::default(),
        screens: Vec::new(),
        note: Some(style.note).filter(|note| !note.trim().is_empty()),
        is_default: false,
    };
    match store.save(&template).await {
        Ok(()) => {
            tracing::info!(template_id = %template.id, "template extracted");
            notifier.notify();
            (StatusCode::CREATED, Json(summarize(&template))).into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Marks a template as one a new session starts with, or clears the
/// mark.
pub(super) async fn set_default_template(
    State(store): State<TemplateStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<DefaultRequest>,
) -> Response {
    if !is_valid_template_id(&id) {
        return invalid_template_id(&id);
    }
    let mut template = match store.load(&id).await {
        Ok(Some(template)) => template,
        Ok(None) => return template_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    template.is_default = request.is_default;
    match store.save(&template).await {
        Ok(()) => {
            tracing::info!(template_id = %id, is_default = request.is_default, "template default set");
            notifier.notify();
            Json(summarize(&template)).into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Query of `GET /templates/{id}/render`.
#[derive(Debug, Deserialize)]
pub(super) struct RenderQuery {
    /// Render only this one-based example screen. Used by the thumbnails
    /// in the template picker.
    #[serde(default)]
    screen: Option<usize>,
}

/// Renders a template's example screens as a design page, so the picker can
/// show the real look in an iframe. The template name is the design title;
/// the screens are the ones saved as layout examples.
pub(super) async fn render_template(
    State(store): State<TemplateStore>,
    Path(id): Path<String>,
    Query(query): Query<RenderQuery>,
) -> Response {
    if !is_valid_template_id(&id) {
        return invalid_template_id(&id);
    }
    let template = match store.load(&id).await {
        Ok(Some(template)) => template,
        Ok(None) => return template_not_found(&id),
        Err(error) => return api_error::internal_error(&error),
    };
    // A template extracted from brand material has no example screens:
    // the picker shows a swatch of its theme instead.
    let screens = if template.screens.is_empty() {
        vec![swatch_screen(&template)]
    } else {
        template.screens.clone()
    };
    let design = Design {
        title: template.name.clone(),
        theme: template.theme.clone(),
        viewport: template.viewport,
        app_window: None,
        screens,
        outline: Vec::new(),
        transition: None,
    };
    let errors = design.validate();
    if !errors.is_empty() {
        return api_error::validation_failed(&errors);
    }
    let only_screen = match query.screen {
        Some(number) if number >= 1 && number <= design.screens.len() => Some(number - 1),
        Some(number) => {
            return api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!(
                    "template `{id}` has no screen {number}: use 1 to {}",
                    design.screens.len()
                ),
                Vec::new(),
            );
        }
        None => None,
    };
    let options = RenderOptions {
        only_screen,
        ..RenderOptions::default()
    };
    Html(render_design_with(&design, options)).into_response()
}

/// The error for an id no template file matches.
pub(super) fn template_not_found(id: &str) -> Response {
    api_error::error_response(
        StatusCode::NOT_FOUND,
        &format!("no template `{id}`: run `GET /templates` for the saved list"),
        Vec::new(),
    )
}

/// Returns every saved template, newest first.
pub(super) async fn list_templates(State(store): State<TemplateStore>) -> Response {
    match store.list().await {
        Ok(templates) => Json(templates.iter().map(summarize).collect::<Vec<_>>()).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Returns one template with its theme and example screens.
pub(super) async fn get_template(
    State(store): State<TemplateStore>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_template_id(&id) {
        return invalid_template_id(&id);
    }
    match store.load(&id).await {
        Ok(Some(template)) => Json(template).into_response(),
        Ok(None) => template_not_found(&id),
        Err(error) => api_error::internal_error(&error),
    }
}

/// The style of a stored design: its theme, canvas, and first written
/// screens. A placeholder holds no layout, so it teaches the model
/// nothing.
pub(super) async fn design_style(designs: &DesignStore, id: &str) -> Result<SourceStyle, Response> {
    let design = match designs.load(id).await {
        Ok(Some(design)) => design,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no design `{id}`: run `GET /designs` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    Ok(SourceStyle {
        theme: design.theme,
        viewport: design.viewport,
        screens: design
            .screens
            .iter()
            .filter(|screen| !is_pending_screen(screen))
            .take(TEMPLATE_SCREEN_LIMIT)
            .cloned()
            .collect(),
    })
}

/// The style of a stored deck: its theme, the deck canvas, and its
/// first written slides as screens.
pub(super) async fn deck_style(decks: &DeckStore, id: &str) -> Result<SourceStyle, Response> {
    let deck = match decks.load(id).await {
        Ok(Some(deck)) => deck,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no deck `{id}`: run `GET /decks` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    Ok(SourceStyle {
        theme: deck.theme,
        viewport: DECK_VIEWPORT,
        screens: deck
            .slides
            .iter()
            .filter(|slide| !is_pending_slide(slide))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|slide| Screen {
                name: String::new(),
                html: slide.html.clone(),
                css: slide.css.clone(),
                notes: slide.notes.clone(),
            })
            .collect(),
    })
}

/// The style of a stored document: its theme, the paper canvas, and
/// its first written pages as screens.
pub(super) async fn document_style(
    documents: &DocumentStore,
    id: &str,
) -> Result<SourceStyle, Response> {
    let document = match documents.load(id).await {
        Ok(Some(document)) => document,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no document `{id}`: run `GET /documents` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    Ok(SourceStyle {
        theme: document.theme,
        viewport: document.paper.viewport(),
        screens: document
            .pages
            .iter()
            .filter(|page| !is_pending_page(page))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|page| Screen {
                name: String::new(),
                html: page.html.clone(),
                css: page.css.clone(),
                notes: page.notes.clone(),
            })
            .collect(),
    })
}

/// The style of a stored social: its theme, the format canvas, and
/// its first written frames as screens.
pub(super) async fn social_style(socials: &SocialStore, id: &str) -> Result<SourceStyle, Response> {
    let social = match socials.load(id).await {
        Ok(Some(social)) => social,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no social `{id}`: run `GET /socials` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    Ok(SourceStyle {
        theme: social.theme,
        viewport: social.format.viewport(),
        screens: social
            .frames
            .iter()
            .filter(|frame| !is_pending_frame(frame))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|frame| Screen {
                name: String::new(),
                html: frame.html.clone(),
                css: frame.css.clone(),
                notes: frame.notes.clone(),
            })
            .collect(),
    })
}

/// The style of a stored print: its theme, the sheet canvas, and its
/// first written sheets as screens.
pub(super) async fn print_style(prints: &PrintStore, id: &str) -> Result<SourceStyle, Response> {
    let print = match prints.load(id).await {
        Ok(Some(print)) => print,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no print `{id}`: run `GET /prints` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let viewport = print.viewport();
    Ok(SourceStyle {
        theme: print.theme,
        viewport,
        screens: print
            .sheets
            .iter()
            .filter(|sheet| !is_pending_sheet(sheet))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|sheet| Screen {
                name: String::new(),
                html: sheet.html.clone(),
                css: sheet.css.clone(),
                notes: sheet.notes.clone(),
            })
            .collect(),
    })
}

/// The style of a stored mailing: its theme, the email canvas, and
/// its first written emails as screens.
pub(super) async fn mailing_style(
    mailings: &MailingStore,
    id: &str,
) -> Result<SourceStyle, Response> {
    let mailing = match mailings.load(id).await {
        Ok(Some(mailing)) => mailing,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no mailing `{id}`: run `GET /mailings` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let viewport = mailing.viewport();
    Ok(SourceStyle {
        theme: mailing.theme,
        viewport,
        screens: mailing
            .emails
            .iter()
            .filter(|email| !is_pending_email(email))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|email| Screen {
                name: String::new(),
                html: email.html.clone(),
                css: email.css.clone(),
                notes: email.notes.clone(),
            })
            .collect(),
    })
}

/// The style of a stored campaign: its theme, the ad canvas, and its
/// first written ads as screens.
pub(super) async fn campaign_style(
    campaigns: &CampaignStore,
    id: &str,
) -> Result<SourceStyle, Response> {
    let campaign = match campaigns.load(id).await {
        Ok(Some(campaign)) => campaign,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no campaign `{id}`: run `GET /campaigns` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let viewport = campaign.viewport();
    Ok(SourceStyle {
        theme: campaign.theme,
        viewport,
        screens: campaign
            .ads
            .iter()
            .filter(|ad| !is_pending_ad(ad))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|ad| Screen {
                name: String::new(),
                html: ad.html.clone(),
                css: ad.css.clone(),
                notes: ad.notes.clone(),
            })
            .collect(),
    })
}

/// The style of a stored artwork: its theme, the cover canvas, and
/// its first written covers as screens.
pub(super) async fn artwork_style(
    artworks: &ArtworkStore,
    id: &str,
) -> Result<SourceStyle, Response> {
    let artwork = match artworks.load(id).await {
        Ok(Some(artwork)) => artwork,
        Ok(None) => {
            return Err(api_error::error_response(
                StatusCode::NOT_FOUND,
                &format!("no artwork `{id}`: run `GET /artworks` for the list"),
                Vec::new(),
            ));
        }
        Err(error) => return Err(api_error::internal_error(&error)),
    };
    let viewport = artwork.viewport();
    Ok(SourceStyle {
        theme: artwork.theme,
        viewport,
        screens: artwork
            .covers
            .iter()
            .filter(|cover| !is_pending_cover(cover))
            .take(TEMPLATE_SCREEN_LIMIT)
            .map(|cover| Screen {
                name: String::new(),
                html: cover.html.clone(),
                css: cover.css.clone(),
                notes: cover.notes.clone(),
            })
            .collect(),
    })
}

/// Saves the style of one design, deck, document, social, print,
/// mailing, campaign, or artwork as a template.
pub(super) async fn save_template(
    State(store): State<TemplateStore>,
    State(stores): State<ArtifactStores>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<SaveRequest>,
) -> Response {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > NAME_LIMIT {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("template name must be 1 to {NAME_LIMIT} characters"),
            Vec::new(),
        );
    }
    let source = match template_source(&request) {
        Ok(source) => source,
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_REQUEST, &message, Vec::new());
        }
    };
    let (source_id, style) = match &source {
        TemplateSource::Design(id) => (id.clone(), design_style(&stores.designs, id).await),
        TemplateSource::Deck(id) => (id.clone(), deck_style(&stores.decks, id).await),
        TemplateSource::Document(id) => (id.clone(), document_style(&stores.documents, id).await),
        TemplateSource::Social(id) => (id.clone(), social_style(&stores.socials, id).await),
        TemplateSource::Print(id) => (id.clone(), print_style(&stores.prints, id).await),
        TemplateSource::Mailing(id) => (id.clone(), mailing_style(&stores.mailings, id).await),
        TemplateSource::Campaign(id) => (id.clone(), campaign_style(&stores.campaigns, id).await),
        TemplateSource::Artwork(id) => (id.clone(), artwork_style(&stores.artworks, id).await),
    };
    let style = match style {
        Ok(style) => style,
        Err(response) => return response,
    };
    if style.screens.is_empty() {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "`{source_id}` has no written screens, slides, pages, frames, sheets, \
                 emails, ads, or covers to save as a template"
            ),
            Vec::new(),
        );
    }
    let template = Template {
        id: template_id(name),
        name: name.to_owned(),
        saved_at: rfc3339_now(),
        source_design: source_id,
        theme: style.theme,
        viewport: style.viewport,
        screens: style.screens,
        note: None,
        is_default: false,
    };
    match store.save(&template).await {
        Ok(()) => {
            tracing::info!(
                template_id = %template.id,
                screen_count = template.screens.len(),
                "template saved"
            );
            notifier.notify();
            (StatusCode::CREATED, Json(summarize(&template))).into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Removes one template.
pub(super) async fn delete_template(
    State(store): State<TemplateStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if !is_valid_template_id(&id) {
        return invalid_template_id(&id);
    }
    match store.delete(&id).await {
        Ok(true) => {
            notifier.notify();
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => api_error::error_response(
            StatusCode::NOT_FOUND,
            &format!("no template `{id}`: run `GET /templates` for the saved list"),
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// The listing row for one template.
pub(super) fn summarize(template: &Template) -> TemplateSummary {
    TemplateSummary {
        id: template.id.clone(),
        name: template.name.clone(),
        saved_at: template.saved_at.clone(),
        theme: template.theme.name.clone(),
        screen_count: template.screens.len(),
        is_default: template.is_default,
    }
}

/// The id for a template name: lowercase, words joined by `-`, with the
/// save time appended so two templates with one name never collide.
pub(super) fn template_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug: Vec<&str> = slug.split('-').filter(|part| !part.is_empty()).collect();
    let slug = slug.join("-");
    let slug = if slug.is_empty() {
        "template".to_owned()
    } else {
        slug.chars().take(40).collect()
    };
    format!("{slug}-{}", unix_now_seconds())
}
