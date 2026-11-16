//! Swift Design server: serves the editor UI, design, deck, document,
//! and social files, and uploads.

mod agent_runs;
mod api_error;
mod artwork;
mod brand;
mod campaign;
mod candidates;
mod capture;
mod concepts;
mod deck;
mod design;
mod document;
mod edit_focus;
mod edit_report;
mod events;
mod export;
mod files;
mod generation;
mod history;
mod icon;
mod instructions;
mod mailing;
mod model_client;
mod model_retry;
mod office;
mod parallel_edit;
mod planner;
mod polish;
mod print;
mod projects;
mod provenance;
mod render;
mod request;
mod screen_css;
mod screenshots;
mod session_routes;
mod sessions;
mod settings;
mod social;
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
use crate::artwork::core::ArtworkStore;
use crate::campaign::core::CampaignStore;
use crate::deck::core::DeckStore;
use crate::design::core::DesignStore;
use crate::document::core::DocumentStore;
use crate::events::ChangeNotifier;
use crate::history::HistoryStore;
use crate::mailing::core::MailingStore;
use crate::print::core::PrintStore;
use crate::sessions::SessionStore;
use crate::settings::SettingsStore;
use crate::social::core::SocialStore;
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
        .merge(crate::design::core::routes())
        .merge(crate::deck::core::routes())
        .merge(crate::document::core::routes())
        .merge(crate::social::core::routes())
        .merge(crate::print::core::routes())
        .merge(crate::mailing::core::routes())
        .merge(crate::campaign::core::routes())
        .merge(crate::artwork::core::routes())
        .merge(crate::deck::presenter::routes())
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
        return Html(crate::design::render::render_design(&design, false)).into_response();
    }
    api_error::validation_failed(&errors)
}

/// Renders a posted deck to HTML, or reports every validation error.
async fn render_deck(Json(deck): Json<Deck>) -> Response {
    let errors = deck.validate();
    if errors.is_empty() {
        return Html(crate::deck::render::render_deck(&deck, false)).into_response();
    }
    api_error::deck_validation_failed(&errors)
}

/// Renders a posted document to HTML, or reports every validation
/// error.
async fn render_document(Json(document): Json<Document>) -> Response {
    let errors = document.validate();
    if errors.is_empty() {
        return Html(crate::document::render::render_document(&document, false)).into_response();
    }
    api_error::document_validation_failed(&errors)
}

/// Renders a posted social to HTML, or reports every validation error.
async fn render_social(Json(social): Json<Social>) -> Response {
    let errors = social.validate();
    if errors.is_empty() {
        return Html(crate::social::render::render_social(&social, false)).into_response();
    }
    api_error::social_validation_failed(&errors)
}

/// Renders a posted print to HTML, or reports every validation error.
async fn render_print(Json(print): Json<Print>) -> Response {
    let errors = print.validate();
    if errors.is_empty() {
        return Html(crate::print::render::render_print(&print, false)).into_response();
    }
    api_error::print_validation_failed(&errors)
}

/// Renders a posted mailing to HTML, or reports every validation
/// error.
async fn render_mailing(Json(mailing): Json<Mailing>) -> Response {
    let errors = mailing.validate();
    if errors.is_empty() {
        return Html(crate::mailing::render::render_mailing(&mailing, false)).into_response();
    }
    api_error::mailing_validation_failed(&errors)
}

/// Renders a posted campaign to HTML, or reports every validation
/// error.
async fn render_campaign(Json(campaign): Json<Campaign>) -> Response {
    let errors = campaign.validate();
    if errors.is_empty() {
        return Html(crate::campaign::render::render_campaign(&campaign, false)).into_response();
    }
    api_error::campaign_validation_failed(&errors)
}

/// Renders a posted artwork to HTML, or reports every validation
/// error.
async fn render_artwork(Json(artwork): Json<Artwork>) -> Response {
    let errors = artwork.validate();
    if errors.is_empty() {
        return Html(crate::artwork::render::render_artwork(&artwork, false)).into_response();
    }
    api_error::artwork_validation_failed(&errors)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
