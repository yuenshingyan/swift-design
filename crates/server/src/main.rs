//! Swift Design server: serves the editor UI, design, deck, document,
//! and social files, and uploads.

mod agent_runs;
mod api_error;
mod artwork_generation;
mod artwork_patch;
mod artwork_polish;
mod artwork_render;
mod artworks;
mod brand;
mod campaign_generation;
mod campaign_patch;
mod campaign_polish;
mod campaign_render;
mod campaigns;
mod candidates;
mod capture;
mod concepts;
mod deck_generation;
mod deck_patch;
mod deck_polish;
mod deck_render;
mod decks;
mod designs;
mod document_generation;
mod document_patch;
mod document_polish;
mod document_render;
mod documents;
mod docx;
mod edit_focus;
mod email_html;
mod events;
mod export;
mod files;
mod generation;
mod history;
mod icon;
mod instructions;
mod mailing_generation;
mod mailing_patch;
mod mailing_polish;
mod mailing_render;
mod mailings;
mod model_client;
mod office;
mod patch;
mod planner;
mod polish;
mod pptx;
mod presenter;
mod print_generation;
mod print_patch;
mod print_polish;
mod print_render;
mod prints;
mod projects;
mod provenance;
mod render;
mod request;
mod screen_css;
mod screenshots;
mod session_routes;
mod sessions;
mod settings;
mod social_generation;
mod social_patch;
mod social_polish;
mod social_render;
mod socials;
mod static_files;
mod templates;
#[cfg(test)]
mod test_support;
mod time;
mod uploads;

use std::path::PathBuf;

use anyhow::Context;
use axum::extract::FromRef;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::{Artwork, Campaign, Deck, Design, Document, Mailing, Print, Social};
use tracing_subscriber::EnvFilter;

use crate::agent_runs::AgentRunner;
use crate::artworks::ArtworkStore;
use crate::campaigns::CampaignStore;
use crate::decks::DeckStore;
use crate::designs::DesignStore;
use crate::documents::DocumentStore;
use crate::events::ChangeNotifier;
use crate::history::HistoryStore;
use crate::mailings::MailingStore;
use crate::prints::PrintStore;
use crate::sessions::SessionStore;
use crate::settings::SettingsStore;
use crate::socials::SocialStore;
use crate::static_files::UiDirectory;
use crate::templates::TemplateStore;
use crate::uploads::UploadStore;

/// Shared state behind every route.
#[derive(Clone)]
pub(crate) struct AppState {
    /// Design storage.
    designs: DesignStore,
    /// Deck storage.
    decks: DeckStore,
    /// Document storage.
    documents: DocumentStore,
    /// Social storage.
    socials: SocialStore,
    /// Print storage.
    prints: PrintStore,
    /// Mailing storage.
    mailings: MailingStore,
    /// Campaign storage.
    campaigns: CampaignStore,
    /// Artwork storage.
    artworks: ArtworkStore,
    /// Upload storage.
    uploads: UploadStore,
    /// Session storage.
    sessions: SessionStore,
    /// Revision counter behind `GET /events`.
    changes: ChangeNotifier,
    /// Model settings the user picked in the studio.
    settings: SettingsStore,
    /// Starter for generation runs.
    agent: AgentRunner,
    /// Saved design templates.
    templates: TemplateStore,
    /// Directory with the built editor UI.
    ui: UiDirectory,
}

impl FromRef<AppState> for DesignStore {
    fn from_ref(state: &AppState) -> DesignStore {
        state.designs.clone()
    }
}

impl FromRef<AppState> for DeckStore {
    fn from_ref(state: &AppState) -> DeckStore {
        state.decks.clone()
    }
}

impl FromRef<AppState> for DocumentStore {
    fn from_ref(state: &AppState) -> DocumentStore {
        state.documents.clone()
    }
}

impl FromRef<AppState> for SocialStore {
    fn from_ref(state: &AppState) -> SocialStore {
        state.socials.clone()
    }
}

impl FromRef<AppState> for PrintStore {
    fn from_ref(state: &AppState) -> PrintStore {
        state.prints.clone()
    }
}

impl FromRef<AppState> for MailingStore {
    fn from_ref(state: &AppState) -> MailingStore {
        state.mailings.clone()
    }
}

impl FromRef<AppState> for CampaignStore {
    fn from_ref(state: &AppState) -> CampaignStore {
        state.campaigns.clone()
    }
}

impl FromRef<AppState> for ArtworkStore {
    fn from_ref(state: &AppState) -> ArtworkStore {
        state.artworks.clone()
    }
}

impl FromRef<AppState> for UploadStore {
    fn from_ref(state: &AppState) -> UploadStore {
        state.uploads.clone()
    }
}

impl FromRef<AppState> for SessionStore {
    fn from_ref(state: &AppState) -> SessionStore {
        state.sessions.clone()
    }
}

impl FromRef<AppState> for ChangeNotifier {
    fn from_ref(state: &AppState) -> ChangeNotifier {
        state.changes.clone()
    }
}

impl FromRef<AppState> for SettingsStore {
    fn from_ref(state: &AppState) -> SettingsStore {
        state.settings.clone()
    }
}

impl FromRef<AppState> for AgentRunner {
    fn from_ref(state: &AppState) -> AgentRunner {
        state.agent.clone()
    }
}

impl FromRef<AppState> for TemplateStore {
    fn from_ref(state: &AppState) -> TemplateStore {
        state.templates.clone()
    }
}

impl FromRef<AppState> for UiDirectory {
    fn from_ref(state: &AppState) -> UiDirectory {
        state.ui.clone()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let address =
        std::env::var("SWIFT_DESIGN_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let designs_directory =
        std::env::var("SWIFT_DESIGN_DESIGNS_DIR").unwrap_or_else(|_| "designs".to_owned());
    let uploads_directory =
        std::env::var("SWIFT_DESIGN_UPLOADS_DIR").unwrap_or_else(|_| "uploads".to_owned());
    let sessions_directory =
        std::env::var("SWIFT_DESIGN_SESSIONS_DIR").unwrap_or_else(|_| "data/sessions".to_owned());
    let ui_directory = std::env::var("SWIFT_DESIGN_UI_DIR")
        .unwrap_or_else(|_| "target/dx/ui/release/web/public".to_owned());
    let settings_path = std::env::var("SWIFT_DESIGN_SETTINGS_PATH")
        .unwrap_or_else(|_| "data/settings.json".to_owned());
    let templates_directory =
        std::env::var("SWIFT_DESIGN_TEMPLATES_DIR").unwrap_or_else(|_| "templates".to_owned());
    let history_directory =
        std::env::var("SWIFT_DESIGN_HISTORY_DIR").unwrap_or_else(|_| "history".to_owned());
    let decks_directory =
        std::env::var("SWIFT_DESIGN_DECKS_DIR").unwrap_or_else(|_| "decks".to_owned());
    let deck_history_directory = std::env::var("SWIFT_DESIGN_DECK_HISTORY_DIR")
        .unwrap_or_else(|_| "deck-history".to_owned());
    let documents_directory =
        std::env::var("SWIFT_DESIGN_DOCUMENTS_DIR").unwrap_or_else(|_| "documents".to_owned());
    let document_history_directory = std::env::var("SWIFT_DESIGN_DOCUMENT_HISTORY_DIR")
        .unwrap_or_else(|_| "document-history".to_owned());
    let socials_directory =
        std::env::var("SWIFT_DESIGN_SOCIALS_DIR").unwrap_or_else(|_| "socials".to_owned());
    let social_history_directory = std::env::var("SWIFT_DESIGN_SOCIAL_HISTORY_DIR")
        .unwrap_or_else(|_| "social-history".to_owned());
    let prints_directory =
        std::env::var("SWIFT_DESIGN_PRINTS_DIR").unwrap_or_else(|_| "prints".to_owned());
    let print_history_directory = std::env::var("SWIFT_DESIGN_PRINT_HISTORY_DIR")
        .unwrap_or_else(|_| "print-history".to_owned());
    let mailings_directory =
        std::env::var("SWIFT_DESIGN_MAILINGS_DIR").unwrap_or_else(|_| "mailings".to_owned());
    let mailing_history_directory = std::env::var("SWIFT_DESIGN_MAILING_HISTORY_DIR")
        .unwrap_or_else(|_| "mailing-history".to_owned());
    let campaigns_directory =
        std::env::var("SWIFT_DESIGN_CAMPAIGNS_DIR").unwrap_or_else(|_| "campaigns".to_owned());
    let campaign_history_directory = std::env::var("SWIFT_DESIGN_CAMPAIGN_HISTORY_DIR")
        .unwrap_or_else(|_| "campaign-history".to_owned());
    let artworks_directory =
        std::env::var("SWIFT_DESIGN_ARTWORKS_DIR").unwrap_or_else(|_| "artworks".to_owned());
    let artwork_history_directory = std::env::var("SWIFT_DESIGN_ARTWORK_HISTORY_DIR")
        .unwrap_or_else(|_| "artwork-history".to_owned());
    let changes = ChangeNotifier::new();
    let designs = DesignStore::new(PathBuf::from(designs_directory))
        .with_history(HistoryStore::new(PathBuf::from(history_directory)));
    let decks = DeckStore::new(PathBuf::from(decks_directory))
        .with_history(HistoryStore::new(PathBuf::from(deck_history_directory)));
    let documents = DocumentStore::new(PathBuf::from(documents_directory))
        .with_history(HistoryStore::new(PathBuf::from(document_history_directory)));
    let socials = SocialStore::new(PathBuf::from(socials_directory))
        .with_history(HistoryStore::new(PathBuf::from(social_history_directory)));
    let prints = PrintStore::new(PathBuf::from(prints_directory))
        .with_history(HistoryStore::new(PathBuf::from(print_history_directory)));
    let mailings = MailingStore::new(PathBuf::from(mailings_directory))
        .with_history(HistoryStore::new(PathBuf::from(mailing_history_directory)));
    let campaigns = CampaignStore::new(PathBuf::from(campaigns_directory))
        .with_history(HistoryStore::new(PathBuf::from(campaign_history_directory)));
    let artworks = ArtworkStore::new(PathBuf::from(artworks_directory))
        .with_history(HistoryStore::new(PathBuf::from(artwork_history_directory)));
    let sessions = SessionStore::new(PathBuf::from(sessions_directory));
    // A run dies with the process. Its session would wait for it forever.
    for session_id in sessions
        .stop_orphaned_runs()
        .await
        .context("stopping the runs the last process left behind")?
    {
        tracing::info!(%session_id, "stopped an orphaned run");
    }
    let settings = SettingsStore::new(PathBuf::from(settings_path), address.clone());
    let templates = TemplateStore::new(PathBuf::from(templates_directory));
    let uploads = UploadStore::new(PathBuf::from(uploads_directory));
    let agent = AgentRunner::new(
        std::env::var("SWIFT_DESIGN_AGENT_COMMAND").ok(),
        settings.clone(),
        designs.clone(),
        sessions.clone(),
        format!("http://{address}"),
        changes.clone(),
    )
    .with_decks(decks.clone())
    .with_documents(documents.clone())
    .with_socials(socials.clone())
    .with_prints(prints.clone())
    .with_mailings(mailings.clone())
    .with_campaigns(campaigns.clone())
    .with_artworks(artworks.clone())
    .with_templates(templates.clone())
    .with_uploads(uploads.clone());
    let state = AppState {
        designs,
        decks,
        documents,
        socials,
        prints,
        mailings,
        campaigns,
        artworks,
        uploads,
        sessions,
        settings,
        agent,
        changes,
        templates,
        ui: UiDirectory(PathBuf::from(ui_directory)),
    };

    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, "server listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

/// Builds the HTTP route table.
fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/designs/render", post(render_design))
        .route("/decks/render", post(render_deck))
        .route("/documents/render", post(render_document))
        .route("/socials/render", post(render_social))
        .route("/prints/render", post(render_print))
        .route("/mailings/render", post(render_mailing))
        .route("/campaigns/render", post(render_campaign))
        .route("/artworks/render", post(render_artwork))
        .merge(agent_runs::routes())
        .merge(candidates::routes())
        .merge(events::routes())
        .merge(icon::routes())
        .merge(instructions::routes())
        .merge(designs::routes())
        .merge(decks::routes())
        .merge(documents::routes())
        .merge(socials::routes())
        .merge(prints::routes())
        .merge(mailings::routes())
        .merge(campaigns::routes())
        .merge(artworks::routes())
        .merge(presenter::routes())
        .merge(projects::routes())
        .merge(export::routes())
        .merge(session_routes::routes())
        .merge(screenshots::routes())
        .merge(settings::routes())
        .merge(templates::routes())
        .merge(uploads::routes())
        .fallback(get(static_files::serve_ui))
        .with_state(state)
}

/// Reports that the server is running.
async fn health() -> &'static str {
    "ok"
}

/// Renders a posted design to HTML, or reports every validation error.
async fn render_design(Json(design): Json<Design>) -> Response {
    let errors = design.validate();
    if errors.is_empty() {
        return Html(render::render_design(&design, false)).into_response();
    }
    api_error::validation_failed(&errors)
}

/// Renders a posted deck to HTML, or reports every validation error.
async fn render_deck(Json(deck): Json<Deck>) -> Response {
    let errors = deck.validate();
    if errors.is_empty() {
        return Html(deck_render::render_deck(&deck, false)).into_response();
    }
    api_error::deck_validation_failed(&errors)
}

/// Renders a posted document to HTML, or reports every validation
/// error.
async fn render_document(Json(document): Json<Document>) -> Response {
    let errors = document.validate();
    if errors.is_empty() {
        return Html(document_render::render_document(&document, false)).into_response();
    }
    api_error::document_validation_failed(&errors)
}

/// Renders a posted social to HTML, or reports every validation error.
async fn render_social(Json(social): Json<Social>) -> Response {
    let errors = social.validate();
    if errors.is_empty() {
        return Html(social_render::render_social(&social, false)).into_response();
    }
    api_error::social_validation_failed(&errors)
}

/// Renders a posted print to HTML, or reports every validation error.
async fn render_print(Json(print): Json<Print>) -> Response {
    let errors = print.validate();
    if errors.is_empty() {
        return Html(print_render::render_print(&print, false)).into_response();
    }
    api_error::print_validation_failed(&errors)
}

/// Renders a posted mailing to HTML, or reports every validation
/// error.
async fn render_mailing(Json(mailing): Json<Mailing>) -> Response {
    let errors = mailing.validate();
    if errors.is_empty() {
        return Html(mailing_render::render_mailing(&mailing, false)).into_response();
    }
    api_error::mailing_validation_failed(&errors)
}

/// Renders a posted campaign to HTML, or reports every validation
/// error.
async fn render_campaign(Json(campaign): Json<Campaign>) -> Response {
    let errors = campaign.validate();
    if errors.is_empty() {
        return Html(campaign_render::render_campaign(&campaign, false)).into_response();
    }
    api_error::campaign_validation_failed(&errors)
}

/// Renders a posted artwork to HTML, or reports every validation
/// error.
async fn render_artwork(Json(artwork): Json<Artwork>) -> Response {
    let errors = artwork.validate();
    if errors.is_empty() {
        return Html(artwork_render::render_artwork(&artwork, false)).into_response();
    }
    api_error::artwork_validation_failed(&errors)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::test_support::{
        SAMPLE_ARTWORK, SAMPLE_CAMPAIGN, SAMPLE_DECK, SAMPLE_DESIGN, SAMPLE_DOCUMENT,
        SAMPLE_MAILING, SAMPLE_PRINT, SAMPLE_SOCIAL, application_with_command,
        invalid_sample_design, open_generating_artwork_session, open_generating_campaign_session,
        open_generating_deck_session, open_generating_document_session,
        open_generating_mailing_session, open_generating_print_session, open_generating_session,
        open_generating_social_session, send, send_upload, send_user_put, test_application,
    };

    #[tokio::test]
    async fn an_artwork_session_lists_its_artworks_and_chooses_from_the_artwork_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_artwork_session(&application, "launch").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "artwork");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/artworks/launch-candidate-1",
            Some(SAMPLE_ARTWORK),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["artworks"].as_array().unwrap().len(), 1);
        assert_eq!(view["artworks"][0]["size"], "thumbnail");
        assert_eq!(view["artworks"][0]["cover_count"], 2);
        assert_eq!(view["campaigns"].as_array().unwrap().len(), 0);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this artwork"));
        assert!(body.contains("/artworks/launch-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/launch/choose",
            Some(r#"{"id":"launch-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/artworks/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/campaigns/launch", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_artwork_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/artworks/render",
            Some(SAMPLE_ARTWORK),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"1280\""));
        assert!(body.contains("data-swift-design-height=\"720\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/artworks/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"covers":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("artwork failed validation"));
        assert!(body.contains("artwork has no covers"));
    }

    #[tokio::test]
    async fn saving_then_fetching_an_artwork_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/artworks/launch-candidate-1",
            Some(SAMPLE_ARTWORK),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/artworks/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let artwork: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(artwork["covers"].as_array().unwrap().len(), 2);
        let (status, body) = send(application.clone(), "GET", "/artworks", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"cover_count\":2"));
        // The artwork is not a campaign.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/campaigns/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/artworks/launch-candidate-1/render?cover=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
        let (status, _) = send(
            application,
            "GET",
            "/artworks/launch-candidate-1/render?cover=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_campaign_session_lists_its_campaigns_and_chooses_from_the_campaign_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_campaign_session(&application, "launch").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "campaign");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/campaigns/launch-candidate-1",
            Some(SAMPLE_CAMPAIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["campaigns"].as_array().unwrap().len(), 1);
        assert_eq!(view["campaigns"][0]["size"], "medium_rectangle");
        assert_eq!(view["campaigns"][0]["ad_count"], 2);
        assert_eq!(view["mailings"].as_array().unwrap().len(), 0);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this campaign"));
        assert!(body.contains("/campaigns/launch-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/launch/choose",
            Some(r#"{"id":"launch-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/campaigns/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/mailings/launch", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_campaign_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/campaigns/render",
            Some(SAMPLE_CAMPAIGN),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"300\""));
        assert!(body.contains("data-swift-design-height=\"250\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/campaigns/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"ads":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("campaign failed validation"));
        assert!(body.contains("campaign has no ads"));
    }

    #[tokio::test]
    async fn saving_then_fetching_a_campaign_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/campaigns/launch-candidate-1",
            Some(SAMPLE_CAMPAIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/campaigns/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let campaign: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(campaign["ads"].as_array().unwrap().len(), 2);
        let (status, body) = send(application.clone(), "GET", "/campaigns", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"ad_count\":2"));
        // The campaign is not a mailing.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/mailings/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/campaigns/launch-candidate-1/render?ad=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
        let (status, _) = send(
            application,
            "GET",
            "/campaigns/launch-candidate-1/render?ad=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_mailing_session_lists_its_mailings_and_chooses_from_the_mailing_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_mailing_session(&application, "launch").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "mailing");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/mailings/launch-candidate-1",
            Some(SAMPLE_MAILING),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["mailings"].as_array().unwrap().len(), 1);
        assert_eq!(view["mailings"][0]["format"], "standard");
        assert_eq!(view["mailings"][0]["email_count"], 2);
        assert_eq!(view["prints"].as_array().unwrap().len(), 0);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this mailing"));
        assert!(body.contains("/mailings/launch-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/launch/choose",
            Some(r#"{"id":"launch-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/mailings/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/prints/launch", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_mailing_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/mailings/render",
            Some(SAMPLE_MAILING),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"600\""));
        assert!(body.contains("data-swift-design-height=\"1200\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/mailings/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"emails":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("mailing failed validation"));
        assert!(body.contains("mailing has no emails"));
    }

    #[tokio::test]
    async fn saving_then_fetching_a_mailing_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/mailings/launch-candidate-1",
            Some(SAMPLE_MAILING),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/mailings/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let mailing: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(mailing["emails"].as_array().unwrap().len(), 2);
        let (status, body) = send(application.clone(), "GET", "/mailings", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"email_count\":2"));
        // The mailing is not a print.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/prints/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/mailings/launch-candidate-1/render?email=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
        let (status, _) = send(
            application,
            "GET",
            "/mailings/launch-candidate-1/render?email=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn one_email_serves_as_email_client_html() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        send(
            application.clone(),
            "PUT",
            "/mailings/launch",
            Some(SAMPLE_MAILING),
        )
        .await;
        let (status, body) = send(
            application.clone(),
            "GET",
            "/mailings/launch/emails/2.html",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!DOCTYPE html>"));
        assert!(body.contains("<!--[if mso]>"));
        assert!(!body.contains("var("));
        let (status, _) = send(
            application.clone(),
            "GET",
            "/mailings/launch/emails/9.html",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn exporting_a_mailing_returns_an_email_html_zip() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        send(
            application.clone(),
            "PUT",
            "/mailings/launch",
            Some(SAMPLE_MAILING),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/mailings/launch/export.email.zip")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"launch.email.zip\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "launch-email-1.html",
                "launch-email-2.html",
                "launch-subjects.txt"
            ]
        );
        let mut first = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("launch-email-1.html").unwrap(),
            &mut first,
        )
        .unwrap();
        assert!(first.contains("<!--[if mso]>"));
        assert!(first.contains("<h1 style="));
        assert!(!first.contains("var("));
    }

    #[tokio::test]
    async fn exporting_a_mailing_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        send(
            application.clone(),
            "PUT",
            "/mailings/launch",
            Some(SAMPLE_MAILING),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/mailings/launch/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"launch.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>Six kinds. One chat.</h1>")
        );
    }

    #[tokio::test]
    async fn exporting_an_artwork_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        send(
            application.clone(),
            "PUT",
            "/artworks/launch",
            Some(SAMPLE_ARTWORK),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/artworks/launch/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"launch.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>Eight kinds. One chat.</h1>")
        );
    }

    #[tokio::test]
    async fn exporting_a_campaign_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        send(
            application.clone(),
            "PUT",
            "/campaigns/launch",
            Some(SAMPLE_CAMPAIGN),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/campaigns/launch/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"launch.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>Seven kinds. One chat.</h1>")
        );
    }

    #[tokio::test]
    async fn a_print_session_lists_its_prints_and_chooses_from_the_print_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_print_session(&application, "poster").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/poster", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "print");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/prints/poster-candidate-1",
            Some(SAMPLE_PRINT),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/poster", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["prints"].as_array().unwrap().len(), 1);
        assert_eq!(view["prints"][0]["size"], "a4");
        assert_eq!(view["prints"][0]["orientation"], "portrait");
        assert_eq!(view["prints"][0]["sheet_count"], 2);
        assert_eq!(view["socials"].as_array().unwrap().len(), 0);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/poster", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this print piece"));
        assert!(body.contains("/prints/poster-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/poster/choose",
            Some(r#"{"id":"poster-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/prints/poster", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/socials/poster", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_print_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/prints/render",
            Some(SAMPLE_PRINT),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"794\""));
        assert!(body.contains("data-swift-design-height=\"1123\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/prints/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"sheets":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("print failed validation"));
        assert!(body.contains("print has no sheets"));
    }

    #[tokio::test]
    async fn saving_then_fetching_a_print_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "poster").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/prints/poster-candidate-1",
            Some(SAMPLE_PRINT),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/prints/poster-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let print: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(print["sheets"].as_array().unwrap().len(), 2);
        let (status, body) = send(application.clone(), "GET", "/prints", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"sheet_count\":2"));
        // The print is not a social.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/socials/poster-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/prints/poster-candidate-1/render?sheet=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
        let (status, _) = send(
            application,
            "GET",
            "/prints/poster-candidate-1/render?sheet=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn exporting_a_print_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "poster").await;
        send(
            application.clone(),
            "PUT",
            "/prints/poster",
            Some(SAMPLE_PRINT),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/prints/poster/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"poster.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>One harness. Five kinds.</h1>")
        );
    }

    #[tokio::test]
    async fn a_social_session_lists_its_socials_and_chooses_from_the_social_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_social_session(&application, "launch").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "social");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/socials/launch-candidate-1",
            Some(SAMPLE_SOCIAL),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["socials"].as_array().unwrap().len(), 1);
        assert_eq!(view["socials"][0]["format"], "portrait");
        assert_eq!(view["socials"][0]["frame_count"], 3);
        assert_eq!(view["documents"].as_array().unwrap().len(), 0);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this social post"));
        assert!(body.contains("/socials/launch-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/launch/choose",
            Some(r#"{"id":"launch-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/socials/launch", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/documents/launch", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_social_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/socials/render",
            Some(SAMPLE_SOCIAL),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"1080\""));
        assert!(body.contains("data-swift-design-height=\"1350\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/socials/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"frames":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("social failed validation"));
        assert!(body.contains("social has no frames"));
    }

    #[tokio::test]
    async fn saving_then_fetching_a_social_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/socials/launch-candidate-1",
            Some(SAMPLE_SOCIAL),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/socials/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let social: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(social["frames"].as_array().unwrap().len(), 3);
        let (status, body) = send(application.clone(), "GET", "/socials", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"frame_count\":3"));
        // The social is not a document.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/documents/launch-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/socials/launch-candidate-1/render?frame=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
        let (status, _) = send(
            application,
            "GET",
            "/socials/launch-candidate-1/render?frame=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn exporting_a_social_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "launch").await;
        send(
            application.clone(),
            "PUT",
            "/socials/launch",
            Some(SAMPLE_SOCIAL),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/socials/launch/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"launch.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>One harness. Four kinds.</h1>")
        );
        let (status, _) = send(
            application.clone(),
            "GET",
            "/socials/launch/frames/cover.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            application.clone(),
            "GET",
            "/socials/launch/frames/9.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(application, "GET", "/socials/missing/export.zip", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_document_session_lists_its_documents_and_chooses_from_the_document_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_document_session(&application, "report").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/report", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "document");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/documents/report-candidate-1",
            Some(SAMPLE_DOCUMENT),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/report", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["documents"].as_array().unwrap().len(), 1);
        assert_eq!(view["documents"][0]["paper"], "a4");
        assert_eq!(view["decks"].as_array().unwrap().len(), 0);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/report", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this document"));
        assert!(body.contains("/documents/report-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/report/choose",
            Some(r#"{"id":"report-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/documents/report", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/decks/report", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_document_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/documents/render",
            Some(SAMPLE_DOCUMENT),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"794\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/documents/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"pages":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("document failed validation"));
        assert!(body.contains("document has no pages"));
    }

    #[tokio::test]
    async fn saving_then_fetching_a_document_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "report").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/documents/report-candidate-1",
            Some(SAMPLE_DOCUMENT),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/documents/report-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let document: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(document["pages"].as_array().unwrap().len(), 3);
        let (status, body) = send(application.clone(), "GET", "/documents", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"page_count\":3"));
        // The document is not a deck.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/decks/report-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/documents/report-candidate-1/render?page=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
        let (status, _) = send(
            application,
            "GET",
            "/documents/report-candidate-1/render?page=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn exporting_a_document_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "report").await;
        send(
            application.clone(),
            "PUT",
            "/documents/report",
            Some(SAMPLE_DOCUMENT),
        )
        .await;
        let request = Request::builder()
            .method("GET")
            .uri("/documents/report/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"report.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>Swift Design</h1>")
        );
        let (status, _) = send(
            application.clone(),
            "GET",
            "/documents/report/pages/cover.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            application.clone(),
            "GET",
            "/documents/report/pages/9.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let request = Request::builder()
            .method("GET")
            .uri("/documents/report/export.docx")
            .body(Body::empty())
            .unwrap();
        let response = application.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"report.docx\""
        );
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .contains("wordprocessingml")
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        // A ZIP starts with `PK`.
        assert_eq!(&bytes[..2], b"PK");
    }

    #[tokio::test]
    async fn a_deck_session_lists_its_decks_and_chooses_from_the_deck_store() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_deck_session(&application, "talk").await;
        let (status, body) = send(application.clone(), "GET", "/sessions/talk", None).await;
        assert_eq!(status, StatusCode::OK);
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["artifact_kind"], "deck");
        assert_eq!(view["session"]["artifact_kind"], "deck");
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/decks/talk-candidate-1",
            Some(SAMPLE_DECK),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/sessions/talk", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["decks"].as_array().unwrap().len(), 1);
        assert_eq!(view["designs"].as_array().unwrap().len(), 0);
        let (status, body) = send(application.clone(), "GET", "/candidates/talk", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Choose this deck"));
        assert!(body.contains("/decks/talk-candidate-1/render"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/candidates/talk/choose",
            Some(r#"{"id":"talk-candidate-1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application.clone(), "GET", "/decks/talk", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(application, "GET", "/designs/talk", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renders_a_valid_deck_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/decks/render",
            Some(SAMPLE_DECK),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("data-swift-design-width=\"1920\""));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/decks/render",
            Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"slides":[]}"##),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("deck failed validation"));
        assert!(body.contains("deck has no slides"));
    }

    #[tokio::test]
    async fn saving_then_fetching_a_deck_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "talk").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/decks/talk-candidate-1",
            Some(SAMPLE_DECK),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) =
            send(application.clone(), "GET", "/decks/talk-candidate-1", None).await;
        assert_eq!(status, StatusCode::OK);
        let deck: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(deck["slides"].as_array().unwrap().len(), 3);
        let (status, body) = send(application.clone(), "GET", "/decks", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"slide_count\":3"));
        // The deck is not a design.
        let (status, _) = send(
            application.clone(),
            "GET",
            "/designs/talk-candidate-1",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(
            application,
            "GET",
            "/decks/talk-candidate-1/render?slide=2",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-swift-design-screen=\"1\""));
    }

    #[tokio::test]
    async fn deck_writes_are_rejected_unless_the_session_is_generating() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (status, body) =
            send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("no session `talk`"));
        let (status, _) = send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"talk","request":"A talk."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, body) = send(application, "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("not allowed"));
    }

    #[tokio::test]
    async fn presents_a_stored_deck_with_its_notes() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "talk").await;
        send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
        let (status, body) = send(application.clone(), "GET", "/decks/talk/present", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("<title>Swift Design Deck Overview · presenter</title>"));
        assert!(body.contains("Open with the one-line pitch."));
        assert!(body.contains("/decks/talk/render?audience=true"));
        let (status, _) = send(
            application.clone(),
            "GET",
            "/decks/talk/present?slide=9",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(application, "GET", "/decks/missing/present", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn only_the_audience_render_carries_the_follow_script() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "talk").await;
        send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
        let (status, plain) = send(application.clone(), "GET", "/decks/talk/render", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!plain.contains("data-swift-design-channel"));
        let (status, audience) =
            send(application, "GET", "/decks/talk/render?audience=true", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(audience.contains("data-swift-design-channel=\"swift-design-presenter:talk\""));
        assert!(audience.contains("swift-design-audience-hello"));
    }

    #[tokio::test]
    async fn exporting_a_deck_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "talk").await;
        send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
        let request = Request::builder()
            .method("GET")
            .uri("/decks/talk/export")
            .body(Body::empty())
            .unwrap();
        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-disposition"],
            "attachment; filename=\"talk.html\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .contains("<h1>Swift Design</h1>")
        );
        let (status, _) = send(
            application.clone(),
            "GET",
            "/decks/talk/slides/cover.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(application, "GET", "/decks/talk/slides/9.png", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn health_reports_ok() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/health", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn renders_a_valid_design_to_html() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/designs/render",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
        assert!(body.contains("<h1>Swift Design</h1>"));
    }

    #[tokio::test]
    async fn reports_every_validation_error_for_an_invalid_design() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/designs/render",
            Some(&invalid_sample_design()),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let response: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["error"]["message"], "design failed validation");
        assert_eq!(response["error"]["details"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn design_writes_are_rejected_unless_the_session_is_generating() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        // No session at all: 409.
        let (status, body) = send(
            application.clone(),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("no session `overview`"));
        // Intake session: still 409.
        send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"overview","request":"An overview."}"#),
        )
        .await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        // After generate the session is generating: 204.
        send(
            application.clone(),
            "POST",
            "/sessions/overview/generate",
            None,
        )
        .await;
        let (status, _) = send(application, "PUT", "/designs/overview", Some(SAMPLE_DESIGN)).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn saving_then_fetching_a_design_round_trips() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "overview").await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(application, "GET", "/designs/overview", None).await;
        assert_eq!(status, StatusCode::OK);
        let fetched: serde_json::Value = serde_json::from_str(&body).unwrap();
        let original: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        assert_eq!(fetched, original);
    }

    #[tokio::test]
    async fn lists_saved_designs_sorted_by_id() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        for id in ["zebra", "alpha"] {
            open_generating_session(&application, id).await;
            let (status, _) = send(
                application.clone(),
                "PUT",
                &format!("/designs/{id}"),
                Some(SAMPLE_DESIGN),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        let (status, body) = send(application, "GET", "/designs", None).await;
        assert_eq!(status, StatusCode::OK);
        let summaries: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(summaries[0]["id"], "alpha");
        assert_eq!(summaries[1]["id"], "zebra");
        assert_eq!(summaries[0]["screen_count"], 3);
        assert!(summaries[0]["viewport"].is_object());
    }

    #[tokio::test]
    async fn rejects_an_invalid_design_id() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "PUT",
            "/designs/Bad_Id",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("invalid design id"));
    }

    #[tokio::test]
    async fn fetching_a_missing_design_returns_404() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/missing",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no design with id `missing`"));
    }

    #[tokio::test]
    async fn deleting_a_design_removes_it() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "gone").await;
        send(
            application.clone(),
            "PUT",
            "/designs/gone",
            Some(SAMPLE_DESIGN),
        )
        .await;
        let (status, _) = send(application.clone(), "DELETE", "/designs/gone", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(application.clone(), "GET", "/designs/gone", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(application, "DELETE", "/designs/gone", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn saving_an_invalid_design_reports_every_error() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "bad").await;
        let (status, body) = send(
            application,
            "PUT",
            "/designs/bad",
            Some(&invalid_sample_design()),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let response: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["error"]["details"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn chooser_shows_every_candidate_and_records_the_choice() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "talk").await;
        for id in ["talk-candidate-1", "talk-candidate-2"] {
            send(
                application.clone(),
                "PUT",
                &format!("/designs/{id}"),
                Some(SAMPLE_DESIGN),
            )
            .await;
        }
        let (status, body) = send(application.clone(), "GET", "/candidates/talk", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("/designs/talk-candidate-1/render"));
        assert!(body.contains("talk-candidate-1 · midnight"));
        let (status, body) = send(
            application.clone(),
            "POST",
            "/candidates/talk/choose",
            Some(r#"{"id":"talk-candidate-2"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"id\":\"talk\""));
        let (_, body) = send(application, "GET", "/sessions/talk", None).await;
        let view: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(view["session"]["chosen_design"], "talk-candidate-2");
    }

    #[tokio::test]
    async fn choosing_a_foreign_design_is_rejected() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "other").await;
        send(
            application.clone(),
            "PUT",
            "/designs/other",
            Some(SAMPLE_DESIGN),
        )
        .await;
        let (status, body) = send(
            application,
            "POST",
            "/candidates/talk/choose",
            Some(r#"{"id":"other"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("is not a candidate of `talk`"));
    }

    #[tokio::test]
    async fn uploading_stores_lists_and_serves_a_file() {
        let directory = TempDir::new().unwrap();
        let (status, body) =
            send_upload(test_application(&directory), "Chart Final.PNG", "PNGDATA").await;
        assert_eq!(status, StatusCode::OK);
        let stored: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(stored[0]["name"], "chart-final.png");
        let (status, body) = send(test_application(&directory), "GET", "/uploads", None).await;
        assert_eq!(status, StatusCode::OK);
        let listing: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(listing[0]["size_bytes"], 7);
        assert_eq!(listing[0]["is_image"], true);
    }

    #[tokio::test]
    async fn deleting_a_missing_or_traversal_upload_is_rejected() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "DELETE",
            "/uploads/nothing.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no upload named `nothing.png`"));
        let (status, _) = send(
            test_application(&directory),
            "GET",
            "/uploads/..%2Fsecret",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn root_shows_not_built_page_without_a_ui_bundle() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("The editor UI is not built yet"));
    }

    #[tokio::test]
    async fn blocks_hidden_and_traversal_static_paths() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(test_application(&directory), "GET", "/.env", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn screen_images_need_a_design_a_screen_and_chrome() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (status, body) = send(
            application.clone(),
            "GET",
            "/designs/missing/screens/1.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no design with id"));
        open_generating_session(&application, "shots").await;
        send(
            application.clone(),
            "PUT",
            "/designs/shots",
            Some(SAMPLE_DESIGN),
        )
        .await;
        let (status, body) = send(application, "GET", "/designs/shots/screens/9.png", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("has no screen 9"));
    }

    #[tokio::test]
    async fn exporting_a_design_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "overview").await;
        send(
            application.clone(),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        let response = application
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/designs/overview/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .starts_with("<!doctype html>")
        );
    }

    #[tokio::test]
    async fn history_lists_snapshots_and_restores_one() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "overview").await;
        send(
            application.clone(),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        let (status, body) = send(
            application.clone(),
            "GET",
            "/designs/overview/history",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "[]");
        let renamed = SAMPLE_DESIGN.replace("Swift Design Overview", "Second Title");
        send(
            application.clone(),
            "PUT",
            "/designs/overview",
            Some(&renamed),
        )
        .await;
        let (_, body) = send(
            application.clone(),
            "GET",
            "/designs/overview/history",
            None,
        )
        .await;
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        let stamp = rows[0]["stamp"].as_str().unwrap().to_owned();
        let (status, _) = send(
            application.clone(),
            "POST",
            &format!("/designs/overview/history/{stamp}/restore"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application, "GET", "/designs/overview", None).await;
        assert!(body.contains("Swift Design Overview"));
    }

    #[tokio::test]
    async fn user_saves_are_allowed_while_reviewing() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        open_generating_session(&application, "talk").await;
        send(
            application.clone(),
            "PUT",
            "/designs/talk",
            Some(SAMPLE_DESIGN),
        )
        .await;
        // Complete the run so the session is reviewing.
        send(application.clone(), "POST", "/sessions/talk/complete", None).await;
        let mut edited: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        edited["title"] = serde_json::json!("Edited by hand");
        let (status, _) =
            send_user_put(application.clone(), "/designs/talk", &edited.to_string()).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        // An agent write is rejected while reviewing.
        let (status, _) = send(application, "PUT", "/designs/talk", Some(SAMPLE_DESIGN)).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn settings_report_chrome_availability() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/settings", None).await;
        assert_eq!(status, StatusCode::OK);
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(payload["has_chrome"].is_boolean());
    }

    #[tokio::test]
    async fn mutations_bump_the_events_revision() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (_, body) = send(application.clone(), "GET", "/events", None).await;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["revision"],
            0
        );
        send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"talk","request":"A talk."}"#),
        )
        .await;
        let (_, body) = send(application, "GET", "/events", None).await;
        assert!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["revision"]
                .as_u64()
                .unwrap()
                >= 1
        );
    }

    #[tokio::test]
    async fn agent_run_starts_and_reports_its_log() {
        let directory = TempDir::new().unwrap();
        let application = application_with_command(&directory, Some("echo test-agent".to_owned()));
        send(
            application.clone(),
            "POST",
            "/sessions",
            Some(r#"{"id":"talk","request":"A talk."}"#),
        )
        .await;
        send(
            application.clone(),
            "POST",
            "/agent-runs",
            Some(r#"{"session_id":"talk"}"#),
        )
        .await;
        for _ in 0..100 {
            let (_, body) = send(application.clone(), "GET", "/agent-runs", None).await;
            let run: serde_json::Value = serde_json::from_str(&body).unwrap();
            if run["is_running"] == false {
                assert!(run["log_tail"].as_str().unwrap().contains("test-agent"));
                assert_eq!(run["mode"], "generation");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("agent run did not finish");
    }

    #[tokio::test]
    async fn instructions_and_schemas_are_served_by_the_app() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/instructions", None).await;
        assert_eq!(status, StatusCode::OK);
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["routes"]["session"], "GET /sessions/{id}");
        assert!(payload.to_string().contains("There is no brief"));
        for (path, key) in [
            ("/schemas/design", "screens"),
            ("/schemas/deck", "slides"),
            ("/schemas/document", "pages"),
            ("/schemas/social", "frames"),
            ("/schemas/question-set", "can_proceed_with_assumptions"),
        ] {
            let (status, body) = send(test_application(&directory), "GET", path, None).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            let schema: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert!(schema["properties"][key].is_object(), "{path}");
        }
    }
}
