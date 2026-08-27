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
use axum::routing::get;
use axum::{Json, Router};
use design_model::{DECK_VIEWPORT, Design, Screen, Theme, Viewport};
use serde::{Deserialize, Serialize};

use crate::api_error;
use crate::decks::{DeckStore, is_pending_slide};
use crate::designs::{DesignStore, is_pending_screen};
use crate::events::ChangeNotifier;

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
    /// Screens kept as layout examples, in design order.
    pub screens: Vec<Screen>,
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
}

/// Body of `POST /templates`. Exactly one of `design_id` and `deck_id`
/// names the source.
#[derive(Debug, Deserialize)]
struct SaveRequest {
    /// The design to save the style of.
    #[serde(default)]
    design_id: Option<String>,
    /// The deck to save the style of.
    #[serde(default)]
    deck_id: Option<String>,
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
}

/// The one source a save request names, or the message when it names
/// none or both.
fn template_source(request: &SaveRequest) -> Result<TemplateSource, String> {
    match (&request.design_id, &request.deck_id) {
        (Some(design_id), None) => Ok(TemplateSource::Design(design_id.clone())),
        (None, Some(deck_id)) => Ok(TemplateSource::Deck(deck_id.clone())),
        _ => Err("name exactly one source: `design_id` or `deck_id`".to_owned()),
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
        .route("/templates/{id}", get(get_template).delete(delete_template))
        .route("/templates/{id}/render", get(render_template))
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
    let design = Design {
        title: template.name.clone(),
        theme: template.theme.clone(),
        viewport: template.viewport,
        screens: template.screens.clone(),
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

/// Saves the style of one design or deck as a template.
async fn save_template(
    State(store): State<TemplateStore>,
    State(designs): State<DesignStore>,
    State(decks): State<DeckStore>,
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
        TemplateSource::Design(id) => (id.clone(), design_style(&designs, id).await),
        TemplateSource::Deck(id) => (id.clone(), deck_style(&decks, id).await),
    };
    let style = match style {
        Ok(style) => style,
        Err(response) => return response,
    };
    if style.screens.is_empty() {
        return api_error::error_response(
            StatusCode::BAD_REQUEST,
            &format!("`{source_id}` has no written screens or slides to save as a template"),
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
            name: "x".to_owned(),
        };
        assert!(template_source(&both).is_err());
        let none = SaveRequest {
            design_id: None,
            deck_id: None,
            name: "x".to_owned(),
        };
        assert!(template_source(&none).is_err());
        let deck = SaveRequest {
            design_id: None,
            deck_id: Some("talk".to_owned()),
            name: "x".to_owned(),
        };
        assert_eq!(
            template_source(&deck).unwrap(),
            TemplateSource::Deck("talk".to_owned())
        );
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
