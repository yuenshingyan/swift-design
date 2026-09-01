//! Template storage and the `/templates` routes.
//!
//! A template is a design or a deck the user liked, kept as a style
//! reference. It holds the theme and the first few screens or slides,
//! stored as screens. A run that names a template puts both into the
//! candidate prompt, so the model writes new content in that look. A
//! template never carries content forward: the screens are examples of
//! layout and CSS, not text to reuse. One template store serves both
//! artifact kinds.
//!
//! Templates live as `<id>.json` files in one directory, like designs.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use design_model::{DECK_VIEWPORT, Design, Screen, Theme, Viewport};
use serde::{Deserialize, Serialize};

use crate::api_error;
use crate::artworks::{ArtworkStore, is_pending_cover};
use crate::campaigns::{CampaignStore, is_pending_ad};
use crate::decks::{DeckStore, is_pending_slide};
use crate::designs::{DesignStore, is_pending_screen};
use crate::documents::{DocumentStore, is_pending_page};
use crate::events::ChangeNotifier;
use crate::mailings::{MailingStore, is_pending_email};
use crate::prints::{PrintStore, is_pending_sheet};
use crate::socials::{SocialStore, is_pending_frame};

/// How many screens one template keeps as layout examples. The title
/// screen plus a few body screens show the style; more would only make the
/// prompt longer.
pub const TEMPLATE_SCREEN_LIMIT: usize = 4;

/// Longest template name accepted.
const NAME_LIMIT: usize = 80;

/// One saved template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Template {
    /// File stem of the template file, used in `/templates/{id}` routes.
    pub id: String,
    /// The name the user gave the template.
    pub name: String,
    /// When the template was saved, as an RFC 3339 UTC string.
    pub saved_at: String,
    /// The design or deck the template was saved from.
    pub source_design: String,
    /// The theme every design from this template starts with.
    pub theme: Theme,
    /// The px canvas the example screens were laid out on.
    #[serde(default)]
    pub viewport: design_model::Viewport,
    /// Screens kept as layout examples, in design order. Empty for a
    /// template extracted from brand material.
    pub screens: Vec<Screen>,
    /// How the style looks beyond the theme, from an extraction. The
    /// candidate prompt carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// True when a new session starts with this template picked.
    #[serde(default)]
    pub is_default: bool,
}

/// One row in the `GET /templates` listing.
#[derive(Debug, Serialize)]
pub struct TemplateSummary {
    /// Template id.
    pub id: String,
    /// Template name.
    pub name: String,
    /// When the template was saved, as an RFC 3339 UTC string.
    pub saved_at: String,
    /// Theme name, shown under the template name.
    pub theme: String,
    /// How many example screens the template holds.
    pub screen_count: usize,
    /// True when a new session starts with this template picked.
    pub is_default: bool,
}

/// Body of `POST /templates/extract`. One of `url` and `uploads` names
/// the material.
#[derive(Debug, Deserialize)]
struct ExtractRequest {
    /// The name to show in the template list.
    name: String,
    /// A website to capture.
    #[serde(default)]
    url: Option<String>,
    /// Upload names to read, in `scope`.
    #[serde(default)]
    uploads: Vec<String>,
    /// The session the uploads belong to. Absent means the draft scope
    /// of the landing page.
    #[serde(default)]
    scope: Option<String>,
}

/// Body of `PUT /templates/{id}/default`.
#[derive(Debug, Deserialize)]
struct DefaultRequest {
    is_default: bool,
}

/// Body of `POST /templates`. Exactly one of `design_id`, `deck_id`,
/// `document_id`, and `social_id` names the source.
#[derive(Debug, Deserialize)]
struct SaveRequest {
    /// The design to save the style of.
    #[serde(default)]
    design_id: Option<String>,
    /// The deck to save the style of.
    #[serde(default)]
    deck_id: Option<String>,
    /// The document to save the style of.
    #[serde(default)]
    document_id: Option<String>,
    /// The social to save the style of.
    #[serde(default)]
    social_id: Option<String>,
    /// The print to save the style of.
    #[serde(default)]
    print_id: Option<String>,
    /// The mailing to save the style of.
    #[serde(default)]
    mailing_id: Option<String>,
    /// The campaign to save the style of.
    #[serde(default)]
    campaign_id: Option<String>,
    /// The artwork to save the style of.
    #[serde(default)]
    artwork_id: Option<String>,
    /// The name to show in the template list.
    name: String,
}

/// Where a template's style comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TemplateSource {
    /// The design with this id.
    Design(String),
    /// The deck with this id.
    Deck(String),
    /// The document with this id.
    Document(String),
    /// The social with this id.
    Social(String),
    /// The print with this id.
    Print(String),
    /// The mailing with this id.
    Mailing(String),
    /// The campaign with this id.
    Campaign(String),
    /// The artwork with this id.
    Artwork(String),
}

/// The one source a save request names, or the message when it names
/// none or several.
fn template_source(request: &SaveRequest) -> Result<TemplateSource, String> {
    let named = [
        request
            .design_id
            .as_ref()
            .map(|id| TemplateSource::Design(id.clone())),
        request
            .deck_id
            .as_ref()
            .map(|id| TemplateSource::Deck(id.clone())),
        request
            .document_id
            .as_ref()
            .map(|id| TemplateSource::Document(id.clone())),
        request
            .social_id
            .as_ref()
            .map(|id| TemplateSource::Social(id.clone())),
        request
            .print_id
            .as_ref()
            .map(|id| TemplateSource::Print(id.clone())),
        request
            .mailing_id
            .as_ref()
            .map(|id| TemplateSource::Mailing(id.clone())),
        request
            .campaign_id
            .as_ref()
            .map(|id| TemplateSource::Campaign(id.clone())),
        request
            .artwork_id
            .as_ref()
            .map(|id| TemplateSource::Artwork(id.clone())),
    ];
    let mut sources = named.into_iter().flatten();
    match (sources.next(), sources.next()) {
        (Some(source), None) => Ok(source),
        _ => Err(
            "name exactly one source: `design_id`, `deck_id`, `document_id`, `social_id`, \
             `print_id`, `mailing_id`, `campaign_id`, or `artwork_id`"
                .to_owned(),
        ),
    }
}

/// The theme, canvas, and example screens of a source.
struct SourceStyle {
    theme: Theme,
    viewport: Viewport,
    screens: Vec<Screen>,
}

/// Filesystem-backed template storage: one `<id>.json` file per template.
#[derive(Clone)]
pub struct TemplateStore {
    directory: PathBuf,
}

impl TemplateStore {
    /// Creates a store over `directory`. The directory may not exist
    /// yet; it is created on the first save.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }

    /// Reads one template. `Ok(None)` means no template has that id.
    pub async fn load(&self, id: &str) -> anyhow::Result<Option<Template>> {
        match tokio::fs::read_to_string(self.path_of(id)).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes one template, creating the directory when needed.
    pub async fn save(&self, template: &Template) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.directory).await?;
        let json = serde_json::to_string_pretty(template)?;
        tokio::fs::write(self.path_of(&template.id), json).await?;
        Ok(())
    }

    /// Removes one template. `Ok(false)` means it was not there.
    pub async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        match tokio::fs::remove_file(self.path_of(id)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Every template, newest first. A malformed file is skipped and
    /// reported, so one bad file never empties the list.
    pub async fn list(&self) -> anyhow::Result<Vec<Template>> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut templates = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let raw = match tokio::fs::read_to_string(&path).await {
                Ok(raw) => raw,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            match serde_json::from_str::<Template>(&raw) {
                Ok(template) => templates.push(template),
                Err(error) => tracing::warn!(
                    path = %path.display(),
                    %error,
                    "skipping malformed template file: delete it or save the template again"
                ),
            }
        }
        templates.sort_by(|first, second| second.saved_at.cmp(&first.saved_at));
        Ok(templates)
    }
}

/// True for an id `template_id` could have produced: lowercase letters,
/// digits, and hyphens. It keeps a path separator or a `..` out of the
/// filename the store builds.
fn is_valid_template_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 120
        && id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

/// The error for an id that cannot name a template file.
fn invalid_template_id(id: &str) -> Response {
    api_error::error_response(
        StatusCode::BAD_REQUEST,
        &format!("invalid template id `{id}`: run `GET /templates` for the saved list"),
        Vec::new(),
    )
}

/// The `/templates` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/templates", get(list_templates).post(save_template))
        .route("/templates/extract", post(extract_template))
        .route("/templates/{id}", get(get_template).delete(delete_template))
        .route("/templates/{id}/default", put(set_default_template))
        .route("/templates/{id}/render", get(render_template))
}

/// The one-screen page that shows a template with no example
/// screens: the theme name, a heading, body copy, and an accent
/// button, in the template's colors and fonts.
fn swatch_screen(template: &Template) -> Screen {
    let note = template
        .note
        .as_deref()
        .unwrap_or("Extracted from brand material.");
    Screen {
        name: "Swatch".to_owned(),
        html: format!(
            "<section class='swatch'><p class='kicker'>{}</p><h1>{}</h1><p class='copy'>{}</p>\
             <p><a class='button' href='#screen-1'>Get started</a></p></section>",
            html_escape(&template.theme.name),
            html_escape(&template.name),
            html_escape(note),
        ),
        css: Some(
            ".swatch { padding: 96px; display: flex; flex-direction: column; gap: 24px; \
              justify-content: center; min-height: 100%; } \
              .kicker { color: var(--muted); font-family: var(--mono-font); font-size: 24px; } \
              h1 { font-size: 88px; line-height: 1; } \
              .copy { max-width: 900px; color: var(--muted); } \
              .button { display: inline-block; padding: 20px 40px; border-radius: 12px; \
              background: var(--accent); color: var(--background); text-decoration: none; }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// `text` with the HTML metacharacters escaped.
fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Makes a template from a website or the user's files: the model
/// reads the material and answers with a theme and a style note.
async fn extract_template(
    State(store): State<TemplateStore>,
    State(uploads): State<crate::uploads::UploadStore>,
    State(settings): State<crate::settings::SettingsStore>,
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
        Ok(Some(stored)) => crate::model_client::configuration_from_settings(&stored),
        Ok(None) => None,
        Err(error) => return api_error::internal_error(&error),
    }
    .or_else(crate::model_client::configured_model);
    let Some(configuration) = configuration else {
        return api_error::error_response(
            StatusCode::CONFLICT,
            "choose a model in the studio settings before extracting a template",
            Vec::new(),
        );
    };
    let material = match url {
        Some(url) => crate::brand::material_from_url(url).await,
        None => {
            let scope = request
                .scope
                .as_deref()
                .unwrap_or(crate::uploads::DRAFT_SCOPE);
            crate::brand::material_from_uploads(&uploads, scope, &request.uploads).await
        }
    };
    let material = match material {
        Ok(material) => material,
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_REQUEST, &message, Vec::new());
        }
    };
    let http = match crate::model_client::ModelClient::build_http_client() {
        Ok(http) => http,
        Err(message) => {
            return api_error::error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &message,
                Vec::new(),
            );
        }
    };
    let client = crate::model_client::ModelClient::new(configuration, Some(settings));
    let style = match crate::brand::extract_style(&client, &http, &material).await {
        Ok(style) => style,
        Err(message) => {
            return api_error::error_response(StatusCode::BAD_GATEWAY, &message, Vec::new());
        }
    };
    let template = Template {
        id: template_id(name),
        name: name.to_owned(),
        saved_at: crate::time::rfc3339_now(),
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
async fn set_default_template(
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
struct RenderQuery {
    /// Render only this one-based example screen. Used by the thumbnails
    /// in the template picker.
    #[serde(default)]
    screen: Option<usize>,
}

/// Renders a template's example screens as a design page, so the picker can
/// show the real look in an iframe. The template name is the design title;
/// the screens are the ones saved as layout examples.
async fn render_template(
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
    let options = crate::render::RenderOptions {
        only_screen,
        ..crate::render::RenderOptions::default()
    };
    Html(crate::render::render_design_with(&design, options)).into_response()
}

/// The error for an id no template file matches.
fn template_not_found(id: &str) -> Response {
    api_error::error_response(
        StatusCode::NOT_FOUND,
        &format!("no template `{id}`: run `GET /templates` for the saved list"),
        Vec::new(),
    )
}

/// Returns every saved template, newest first.
async fn list_templates(State(store): State<TemplateStore>) -> Response {
    match store.list().await {
        Ok(templates) => Json(templates.iter().map(summarize).collect::<Vec<_>>()).into_response(),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Returns one template with its theme and example screens.
async fn get_template(State(store): State<TemplateStore>, Path(id): Path<String>) -> Response {
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
async fn design_style(designs: &DesignStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn deck_style(decks: &DeckStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn document_style(documents: &DocumentStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn social_style(socials: &SocialStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn print_style(prints: &PrintStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn mailing_style(mailings: &MailingStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn campaign_style(campaigns: &CampaignStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn artwork_style(artworks: &ArtworkStore, id: &str) -> Result<SourceStyle, Response> {
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
async fn save_template(
    State(store): State<TemplateStore>,
    State(stores): State<crate::session_routes::ArtifactStores>,
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
        saved_at: crate::time::rfc3339_now(),
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
async fn delete_template(
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
fn summarize(template: &Template) -> TemplateSummary {
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
fn template_id(name: &str) -> String {
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
    format!("{slug}-{}", crate::time::unix_now_seconds())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sample_design() -> design_model::Design {
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap()
    }

    #[test]
    fn a_save_request_needs_exactly_one_source() {
        let both = SaveRequest {
            design_id: Some("a".to_owned()),
            deck_id: Some("b".to_owned()),
            document_id: None,
            social_id: None,
            print_id: None,
            mailing_id: None,
            campaign_id: None,
            artwork_id: None,
            name: "x".to_owned(),
        };
        assert!(template_source(&both).is_err());
        let none = SaveRequest {
            design_id: None,
            deck_id: None,
            document_id: None,
            social_id: None,
            print_id: None,
            mailing_id: None,
            campaign_id: None,
            artwork_id: None,
            name: "x".to_owned(),
        };
        assert!(template_source(&none).is_err());
        let deck = SaveRequest {
            design_id: None,
            deck_id: Some("talk".to_owned()),
            document_id: None,
            social_id: None,
            print_id: None,
            mailing_id: None,
            campaign_id: None,
            artwork_id: None,
            name: "x".to_owned(),
        };
        assert_eq!(
            template_source(&deck).unwrap(),
            TemplateSource::Deck("talk".to_owned())
        );
        let document = SaveRequest {
            design_id: None,
            deck_id: None,
            document_id: Some("report".to_owned()),
            social_id: None,
            print_id: None,
            mailing_id: None,
            campaign_id: None,
            artwork_id: None,
            name: "x".to_owned(),
        };
        assert_eq!(
            template_source(&document).unwrap(),
            TemplateSource::Document("report".to_owned())
        );
        let social = SaveRequest {
            design_id: None,
            deck_id: None,
            document_id: None,
            social_id: Some("launch".to_owned()),
            print_id: None,
            mailing_id: None,
            campaign_id: None,
            artwork_id: None,
            name: "x".to_owned(),
        };
        assert_eq!(
            template_source(&social).unwrap(),
            TemplateSource::Social("launch".to_owned())
        );
        let document_and_social = SaveRequest {
            design_id: None,
            deck_id: None,
            document_id: Some("report".to_owned()),
            social_id: Some("launch".to_owned()),
            print_id: None,
            mailing_id: None,
            campaign_id: None,
            artwork_id: None,
            name: "x".to_owned(),
        };
        assert!(template_source(&document_and_social).is_err());
    }

    #[tokio::test]
    async fn a_template_saves_from_a_social() {
        let directory = tempfile::tempdir().unwrap();
        let socials = SocialStore::new(directory.path().join("socials"));
        socials
            .save("launch", &crate::test_support::sample_social())
            .await
            .unwrap();
        let style = social_style(&socials, "launch").await.ok().unwrap();
        assert_eq!(style.viewport, design_model::PORTRAIT_VIEWPORT);
        assert_eq!(style.screens.len(), 3);
        assert!(style.screens[0].name.is_empty());
        assert!(style.screens[0].html.contains("One harness"));
        assert!(social_style(&socials, "missing").await.is_err());
    }

    #[tokio::test]
    async fn a_template_saves_from_a_document() {
        let directory = tempfile::tempdir().unwrap();
        let documents = DocumentStore::new(directory.path().join("documents"));
        documents
            .save("report", &crate::test_support::sample_document())
            .await
            .unwrap();
        let style = document_style(&documents, "report").await.ok().unwrap();
        assert_eq!(style.viewport, design_model::A4_VIEWPORT);
        assert_eq!(style.screens.len(), 3);
        assert!(style.screens[0].name.is_empty());
        assert!(style.screens[0].html.contains("Swift Design"));
        assert!(document_style(&documents, "missing").await.is_err());
    }

    #[tokio::test]
    async fn a_template_saves_from_a_deck() {
        let directory = tempfile::tempdir().unwrap();
        let decks = DeckStore::new(directory.path().join("decks"));
        decks
            .save("talk", &crate::test_support::sample_deck())
            .await
            .unwrap();
        let style = deck_style(&decks, "talk").await.ok().unwrap();
        assert_eq!(style.viewport, DECK_VIEWPORT);
        assert_eq!(style.screens.len(), 3);
        assert!(style.screens[0].name.is_empty());
        assert!(style.screens[0].html.contains("Swift Design"));
        assert!(deck_style(&decks, "missing").await.is_err());
    }

    #[tokio::test]
    async fn a_screenless_template_renders_as_a_swatch_and_takes_the_default_mark() {
        let directory = tempfile::tempdir().unwrap();
        let application = crate::test_support::test_application(&directory);
        let store = TemplateStore::new(directory.path().join("templates"));
        let design = sample_design();
        store
            .save(&Template {
                id: "acme-1".to_owned(),
                name: "Acme".to_owned(),
                saved_at: "2024-01-01T00:00:00Z".to_owned(),
                source_design: "https://acme.com".to_owned(),
                theme: design.theme.clone(),
                viewport: design.viewport,
                screens: Vec::new(),
                note: Some("Generous whitespace & 8px corners.".to_owned()),
                is_default: false,
            })
            .await
            .unwrap();
        let (status, body) = crate::test_support::send(
            application.clone(),
            "GET",
            "/templates/acme-1/render?screen=1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("Generous whitespace &amp; 8px corners."));
        assert!(body.contains("Get started"));
        let (status, body) = crate::test_support::send(
            application.clone(),
            "PUT",
            "/templates/acme-1/default",
            Some(r#"{"is_default":true}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"is_default\":true"));
        let (_, listing) =
            crate::test_support::send(application.clone(), "GET", "/templates", None).await;
        assert!(listing.contains("\"is_default\":true"));
        assert!(store.load("acme-1").await.unwrap().unwrap().is_default);
        let (status, _) = crate::test_support::send(
            application.clone(),
            "PUT",
            "/templates/missing/default",
            Some(r#"{"is_default":true}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_extraction_needs_a_name_one_source_and_a_model() {
        let directory = tempfile::tempdir().unwrap();
        let application = crate::test_support::test_application(&directory);
        let (status, body) = crate::test_support::send(
            application.clone(),
            "POST",
            "/templates/extract",
            Some(r#"{"name":"","url":"https://acme.com"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = crate::test_support::send(
            application.clone(),
            "POST",
            "/templates/extract",
            Some(r#"{"name":"Acme"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("exactly one source"));
        // No model is chosen in a fresh test application.
        let (status, body) = crate::test_support::send(
            application.clone(),
            "POST",
            "/templates/extract",
            Some(r#"{"name":"Acme","url":"https://acme.com"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body.contains("choose a model"));
    }

    #[test]
    fn template_ids_are_slugs_with_the_save_time() {
        let id = template_id("Midnight Finance!");
        assert!(id.starts_with("midnight-finance-"), "{id}");
        assert!(template_id("***").starts_with("template-"));
        assert!(!template_id("A").contains("--"));
    }

    #[tokio::test]
    async fn saved_templates_round_trip_and_list_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        let store = TemplateStore::new(directory.path().to_path_buf());
        assert!(store.list().await.unwrap().is_empty());
        let design = sample_design();
        let older = Template {
            id: "older".to_owned(),
            name: "Older".to_owned(),
            saved_at: "2024-01-01T00:00:00Z".to_owned(),
            source_design: "talk".to_owned(),
            theme: design.theme.clone(),
            viewport: design.viewport,
            note: None,
            is_default: false,
            screens: design.screens.clone(),
        };
        let newer = Template {
            id: "newer".to_owned(),
            name: "Newer".to_owned(),
            saved_at: "2025-01-01T00:00:00Z".to_owned(),
            ..older.clone()
        };
        store.save(&older).await.unwrap();
        store.save(&newer).await.unwrap();
        let listed = store.list().await.unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["newer", "older"]
        );
        let loaded = store.load("older").await.unwrap().unwrap();
        assert_eq!(loaded.theme, design.theme);
        assert_eq!(loaded.screens, design.screens);
        assert!(store.delete("older").await.unwrap());
        assert!(!store.delete("older").await.unwrap());
        assert!(store.load("older").await.unwrap().is_none());
    }
}
