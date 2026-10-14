//! Template storage and the `/templates` routes.

//! A template is a design or a deck the user liked, kept as a style
//! reference. It holds the theme and the first few screens or slides,
//! stored as screens. A run that names a template puts both into the
//! candidate prompt, so the model writes new content in that look. A
//! template never carries content forward: the screens are examples of
//! layout and CSS, not text to reuse. One template store serves both
//! artifact kinds.
//!
//! Templates live as `<id>.json` files in one directory, like designs.

mod handlers;
#[cfg(test)]
mod tests;
#[cfg(test)]
use handlers::{deck_style, document_style, social_style, template_id};
use handlers::{
    delete_template, extract_template, get_template, list_templates, render_template,
    save_template, set_default_template,
};

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use design_model::{DECK_VIEWPORT, Design, Screen, Theme, Viewport};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::api_error;
use crate::artworks::{ArtworkStore, is_pending_cover};
use crate::brand::{extract_style, material_from_uploads, material_from_url};
use crate::campaigns::{CampaignStore, is_pending_ad};
use crate::decks::{DeckStore, is_pending_slide};
use crate::designs::{DesignStore, is_pending_screen};
use crate::documents::{DocumentStore, is_pending_page};
use crate::events::ChangeNotifier;
use crate::mailings::{MailingStore, is_pending_email};
use crate::model_client::{ModelClient, configuration_from_settings, configured_model};
use crate::prints::{PrintStore, is_pending_sheet};
use crate::render::{RenderOptions, render_design_with};
use crate::session_routes::ArtifactStores;
use crate::settings::SettingsStore;
use crate::socials::{SocialStore, is_pending_frame};
use crate::time::{rfc3339_now, unix_now_seconds};
use crate::uploads::{DRAFT_SCOPE, UploadStore};

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
pub fn routes() -> Router<AppState> {
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
