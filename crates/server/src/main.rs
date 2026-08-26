//! Swift Design server: serves the editor UI, design files, and uploads.

mod agent_runs;
mod api_error;
mod briefs;
mod candidate_questions;
mod candidates;
mod concepts;
mod designs;
mod events;
mod export;
mod generation;
mod history;
mod icon;
mod instructions;
mod patch;
mod polish;
mod projects;
mod provenance;
mod questions;
mod render;
mod screen_css;
mod screenshots;
mod settings;
mod static_files;
mod templates;
mod uploads;

use std::path::PathBuf;

use axum::extract::FromRef;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use design_model::Design;
use tracing_subscriber::EnvFilter;

use crate::agent_runs::AgentRunner;
use crate::briefs::BriefStore;
use crate::designs::DesignStore;
use crate::events::ChangeNotifier;
use crate::history::HistoryStore;
use crate::questions::QuestionStore;
use crate::settings::SettingsStore;
use crate::static_files::UiDirectory;
use crate::templates::TemplateStore;
use crate::uploads::UploadStore;

/// Shared state behind every route.
#[derive(Clone)]
pub(crate) struct AppState {
    /// Design storage.
    designs: DesignStore,
    /// Upload storage.
    uploads: UploadStore,
    /// Brief storage.
    briefs: BriefStore,
    /// Agent question storage.
    questions: QuestionStore,
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

impl FromRef<AppState> for UploadStore {
    fn from_ref(state: &AppState) -> UploadStore {
        state.uploads.clone()
    }
}

impl FromRef<AppState> for BriefStore {
    fn from_ref(state: &AppState) -> BriefStore {
        state.briefs.clone()
    }
}

impl FromRef<AppState> for QuestionStore {
    fn from_ref(state: &AppState) -> QuestionStore {
        state.questions.clone()
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
    let brief_path =
        std::env::var("SWIFT_DESIGN_BRIEF_PATH").unwrap_or_else(|_| "data/brief.json".to_owned());
    let questions_path = std::env::var("SWIFT_DESIGN_QUESTIONS_PATH")
        .unwrap_or_else(|_| "data/questions.json".to_owned());
    let ui_directory = std::env::var("SWIFT_DESIGN_UI_DIR")
        .unwrap_or_else(|_| "target/dx/ui/release/web/public".to_owned());
    let settings_path = std::env::var("SWIFT_DESIGN_SETTINGS_PATH")
        .unwrap_or_else(|_| "data/settings.json".to_owned());
    let templates_directory =
        std::env::var("SWIFT_DESIGN_TEMPLATES_DIR").unwrap_or_else(|_| "templates".to_owned());
    let history_directory =
        std::env::var("SWIFT_DESIGN_HISTORY_DIR").unwrap_or_else(|_| "history".to_owned());
    let changes = ChangeNotifier::new();
    let designs = DesignStore::new(PathBuf::from(designs_directory))
        .with_history(HistoryStore::new(PathBuf::from(history_directory)));
    let briefs = BriefStore::new(PathBuf::from(brief_path));
    let settings = SettingsStore::new(PathBuf::from(settings_path), address.clone());
    let questions = QuestionStore::new(PathBuf::from(questions_path));
    let templates = TemplateStore::new(PathBuf::from(templates_directory));
    let uploads = UploadStore::new(PathBuf::from(uploads_directory));
    let agent = AgentRunner::new(
        std::env::var("SWIFT_DESIGN_AGENT_COMMAND").ok(),
        settings.clone(),
        designs.clone(),
        briefs.clone(),
        questions.clone(),
        changes.clone(),
    )
    .with_templates(templates.clone())
    .with_uploads(uploads.clone());
    let state = AppState {
        designs,
        uploads,
        briefs,
        questions,
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
        .merge(agent_runs::routes())
        .merge(briefs::routes())
        .merge(candidates::routes())
        .merge(events::routes())
        .merge(icon::routes())
        .merge(instructions::routes())
        .merge(designs::routes())
        .merge(projects::routes())
        .merge(export::routes())
        .merge(questions::routes())
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::designs::DesignStore;
    use crate::uploads::UploadStore;
    use crate::{AppState, router};

    const SAMPLE_DESIGN: &str = include_str!("../../../fixtures/sample-design.json");
    const MULTIPART_BOUNDARY: &str = "swiftdesignboundary";

    fn test_application(directory: &TempDir) -> Router {
        let changes = crate::events::ChangeNotifier::new();
        let designs = DesignStore::new(directory.path().join("designs")).with_history(
            crate::history::HistoryStore::new(directory.path().join("history")),
        );
        let briefs = crate::briefs::BriefStore::new(directory.path().join("data/brief.json"));
        let settings = crate::settings::SettingsStore::new(
            directory.path().join("data/settings.json"),
            "127.0.0.1:3000".to_owned(),
        );
        let questions =
            crate::questions::QuestionStore::new(directory.path().join("data/questions.json"));
        let agent = crate::agent_runs::AgentRunner::new(
            Some("echo test-agent".to_owned()),
            settings.clone(),
            designs.clone(),
            briefs.clone(),
            questions.clone(),
            changes.clone(),
        );
        router(AppState {
            designs,
            uploads: UploadStore::new(directory.path().join("uploads")),
            briefs,
            questions,
            settings,
            agent,
            changes,
            templates: crate::templates::TemplateStore::new(directory.path().join("templates")),
            ui: crate::static_files::UiDirectory(directory.path().join("ui")),
        })
    }

    async fn send_raw(
        application: Router,
        method: &str,
        uri: &str,
        content_type: Option<&str>,
        body: Body,
    ) -> (StatusCode, String) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let response = application
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn send(
        application: Router,
        method: &str,
        uri: &str,
        json: Option<&str>,
    ) -> (StatusCode, String) {
        match json {
            Some(json) => {
                send_raw(
                    application,
                    method,
                    uri,
                    Some("application/json"),
                    Body::from(json.to_owned()),
                )
                .await
            }
            None => send_raw(application, method, uri, None, Body::empty()).await,
        }
    }

    fn multipart_body(file_name: &str, content: &str) -> Body {
        Body::from(format!(
            "--{MULTIPART_BOUNDARY}\r\n\
             content-disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
             content-type: application/octet-stream\r\n\r\n\
             {content}\r\n--{MULTIPART_BOUNDARY}--\r\n"
        ))
    }

    async fn send_upload(
        application: Router,
        file_name: &str,
        content: &str,
    ) -> (StatusCode, String) {
        let content_type = format!("multipart/form-data; boundary={MULTIPART_BOUNDARY}");
        send_raw(
            application,
            "POST",
            "/uploads",
            Some(&content_type),
            multipart_body(file_name, content),
        )
        .await
    }

    fn invalid_sample_design() -> String {
        let mut design: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        design["title"] = serde_json::json!("");
        design["screens"] = serde_json::json!([]);
        design.to_string()
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
    async fn saving_then_fetching_a_design_round_trips() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/overview",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let fetched: serde_json::Value = serde_json::from_str(&body).unwrap();
        let original: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        assert_eq!(fetched, original);
    }

    #[tokio::test]
    async fn lists_saved_designs_sorted_by_id() {
        let directory = TempDir::new().unwrap();
        for id in ["zebra", "alpha"] {
            let uri = format!("/designs/{id}");
            let (status, _) = send(
                test_application(&directory),
                "PUT",
                &uri,
                Some(SAMPLE_DESIGN),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        let (status, body) = send(test_application(&directory), "GET", "/designs", None).await;
        assert_eq!(status, StatusCode::OK);
        let summaries: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(summaries[0]["id"], "alpha");
        assert_eq!(summaries[1]["id"], "zebra");
        assert_eq!(summaries[0]["title"], "Swift Design Overview");
        assert_eq!(summaries[0]["screen_count"], 3);
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
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/gone",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            test_application(&directory),
            "DELETE",
            "/designs/gone",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(test_application(&directory), "GET", "/designs/gone", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            test_application(&directory),
            "DELETE",
            "/designs/gone",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn saving_an_invalid_design_reports_every_error() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
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
    async fn chooser_shows_every_candidate() {
        let directory = TempDir::new().unwrap();
        for id in ["talk-candidate-1", "talk-candidate-2"] {
            let uri = format!("/designs/{id}");
            let (status, _) = send(
                test_application(&directory),
                "PUT",
                &uri,
                Some(SAMPLE_DESIGN),
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT);
        }
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/candidates/talk",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("/designs/talk-candidate-1/render"));
        assert!(body.contains("/designs/talk-candidate-2/render"));
    }

    #[tokio::test]
    async fn choosing_a_candidate_copies_it_and_keeps_every_candidate() {
        let directory = TempDir::new().unwrap();
        for id in ["talk-candidate-1", "talk-candidate-2"] {
            let uri = format!("/designs/{id}");
            send(
                test_application(&directory),
                "PUT",
                &uri,
                Some(SAMPLE_DESIGN),
            )
            .await;
        }
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/candidates/talk/choose",
            Some(r#"{"id":"talk-candidate-2"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"id\":\"talk\""));
        let (status, _) = send(test_application(&directory), "GET", "/designs/talk", None).await;
        assert_eq!(status, StatusCode::OK);
        for id in ["talk-candidate-1", "talk-candidate-2"] {
            let uri = format!("/designs/{id}");
            let (status, _) = send(test_application(&directory), "GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn chooser_with_no_candidates_returns_404() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/candidates/talk",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no candidates for `talk`"));
    }

    #[tokio::test]
    async fn choosing_a_foreign_design_is_rejected() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/other",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
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
        assert_eq!(stored[0]["url"], "/uploads/chart-final.png");

        let (status, body) = send(test_application(&directory), "GET", "/uploads", None).await;
        assert_eq!(status, StatusCode::OK);
        let listing: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(listing[0]["name"], "chart-final.png");
        assert_eq!(listing[0]["size_bytes"], 7);
        assert_eq!(listing[0]["content_type"], "image/png");
        assert_eq!(listing[0]["is_image"], true);

        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/uploads/chart-final.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "PNGDATA");
    }

    #[tokio::test]
    async fn deleting_an_upload_removes_it_and_bumps_the_revision() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (status, _) = send_upload(application.clone(), "Chart.PNG", "PNGDATA").await;
        assert_eq!(status, StatusCode::OK);
        let (_, body) = send(application.clone(), "GET", "/events", None).await;
        let before: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(before["revision"], 1);
        let (status, _) = send(application.clone(), "DELETE", "/uploads/chart.png", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(application.clone(), "GET", "/uploads/chart.png", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no upload named"));
        let (_, body) = send(application, "GET", "/events", None).await;
        let after: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(after["revision"], 2);
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
        let (status, body) = send(
            test_application(&directory),
            "DELETE",
            "/uploads/..%2Fsecret",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("invalid upload name"));
    }

    #[tokio::test]
    async fn rejects_an_upload_request_without_files() {
        let directory = TempDir::new().unwrap();
        let content_type = format!("multipart/form-data; boundary={MULTIPART_BOUNDARY}");
        let empty = Body::from(format!("--{MULTIPART_BOUNDARY}--\r\n"));
        let (status, body) = send_raw(
            test_application(&directory),
            "POST",
            "/uploads",
            Some(&content_type),
            empty,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("no file parts in request"));
    }

    #[tokio::test]
    async fn rejects_a_traversal_upload_name() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/uploads/..%2Fsecret",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("invalid upload name"));
    }

    #[tokio::test]
    async fn root_shows_not_built_page_without_a_ui_bundle() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("The editor UI is not built yet"));
    }

    #[tokio::test]
    async fn serves_built_ui_files() {
        let directory = TempDir::new().unwrap();
        let ui_directory = directory.path().join("ui");
        std::fs::create_dir_all(&ui_directory).unwrap();
        std::fs::write(ui_directory.join("index.html"), "<p>editor</p>").unwrap();
        std::fs::write(ui_directory.join("main.css"), "body{}").unwrap();

        let (status, body) = send(test_application(&directory), "GET", "/", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<p>editor</p>");

        let (status, body) = send(test_application(&directory), "GET", "/main.css", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "body{}");
    }

    #[tokio::test]
    async fn screen_images_need_a_design_a_screen_and_chrome() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/missing/screens/1.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no design with id"));
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/shots",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/shots/screens/9.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("has no screen 9"));
        let (status, _) = send(
            test_application(&directory),
            "GET",
            "/designs/shots/screens/one.png",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pdf_exports_need_a_valid_stored_design() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/Bad_Id/export.pdf",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("invalid design id"));
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/missing/export.pdf",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no design with id"));
    }

    #[tokio::test]
    async fn pdf_export_returns_a_pdf_when_chrome_is_installed() {
        if crate::screenshots::find_chrome().is_none() {
            return;
        }
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let response = test_application(&directory)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/designs/overview/export.pdf")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/pdf"
        );
        assert_eq!(
            response.headers().get("content-disposition").unwrap(),
            "attachment; filename=\"overview.pdf\""
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(bytes.starts_with(b"%PDF-"));
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
    async fn the_index_page_is_never_cached() {
        let directory = TempDir::new().unwrap();
        let ui_directory = directory.path().join("ui");
        std::fs::create_dir_all(&ui_directory).unwrap();
        std::fs::write(ui_directory.join("index.html"), "<p>editor</p>").unwrap();
        let response = test_application(&directory)
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-cache");
    }

    #[tokio::test]
    async fn blocks_hidden_and_traversal_static_paths() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(test_application(&directory), "GET", "/.env", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chooser_cards_show_id_and_theme() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/talk-candidate-1",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/candidates/talk",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("talk-candidate-1 · midnight"));
    }

    #[tokio::test]
    async fn writing_then_reading_a_brief_round_trips() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"A talk about schemas.","variations":3}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        assert_eq!(status, StatusCode::OK);
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(brief["prompt"], "A talk about schemas.");
        assert_eq!(brief["variations"], 3);
        assert_eq!(brief["preview"], true);
        assert_eq!(brief["answers"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn mutations_bump_the_events_revision() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (status, body) = send(application.clone(), "GET", "/events", None).await;
        assert_eq!(status, StatusCode::OK);
        let start: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(start["revision"], 0);

        let (status, _) = send(
            application.clone(),
            "PUT",
            "/designs/talk",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(application.clone(), "GET", "/events", None).await;
        let bumped: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(bumped["revision"], 1);

        // A waiter behind the current revision returns at once.
        let (_, body) = send(application, "GET", "/events?after=0&wait=30", None).await;
        let waited: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(waited["revision"], 1);
    }

    #[tokio::test]
    async fn agent_run_starts_and_reports_its_log() {
        let directory = TempDir::new().unwrap();
        let application = test_application(&directory);
        let (status, _) = send(application.clone(), "POST", "/agent-runs", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        for _ in 0..100 {
            let (_, body) = send(application.clone(), "GET", "/agent-runs", None).await;
            let run: serde_json::Value = serde_json::from_str(&body).unwrap();
            if run["is_running"] == false {
                assert_eq!(run["exit_code"], 0);
                assert!(run["log_tail"].as_str().unwrap().contains("test-agent"));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("agent run did not finish");
    }

    #[tokio::test]
    async fn instructions_and_schema_are_served_by_the_app() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/instructions", None).await;
        assert_eq!(status, StatusCode::OK);
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["routes"]["brief"], "GET /briefs");

        let (status, body) =
            send(test_application(&directory), "GET", "/schemas/design", None).await;
        assert_eq!(status, StatusCode::OK);
        let schema: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(schema["properties"]["screens"].is_object());
        assert!(schema["properties"]["viewport"].is_object());

        let (status, body) =
            send(test_application(&directory), "GET", "/schemas/brief", None).await;
        assert_eq!(status, StatusCode::OK);
        let schema: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(schema["properties"]["confirmed_facts"].is_object());
        assert!(schema["properties"]["assumptions"].is_object());

        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/schemas/question-set",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let schema: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(schema["properties"]["can_proceed_with_assumptions"].is_object());
    }

    #[tokio::test]
    async fn rejects_an_empty_or_oversized_brief() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"  ","variations":2}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("brief prompt is empty"));
        let (status, body) = send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"ok","variations":6}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("variations must be between 1 and 5"));
    }

    #[tokio::test]
    async fn count_and_variety_answers_land_in_the_brief() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"ok"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(brief.get("scenario").is_none());
        assert!(brief.get("length").is_none());
        assert!(brief.get("variations").is_none());
        assert!(brief.get("variety").is_none());
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/questions/answers",
            Some(
                r#"{"answers":[{"question":"What scenario is the design for?","answer":"Finance, Business"},{"question":"How long should the design be?","answer":"Short: 5 to 8 screens"},{"question":"How many candidates should I write?","answer":"2 candidates"},{"question":"How different should the candidates be?","answer":"Low: same structure, new colors and fonts"}]}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(brief["scenario"], "Finance, Business");
        assert_eq!(brief["length"], "5-8");
        assert_eq!(brief["variations"], 2);
        assert_eq!(brief["variety"], "low");
    }

    #[tokio::test]
    async fn reading_an_absent_brief_returns_404() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no brief exists"));
    }

    #[tokio::test]
    async fn exporting_a_design_returns_an_html_download() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let response = test_application(&directory)
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
        let disposition = response
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(disposition, "attachment; filename=\"overview.html\"");
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.starts_with("<!doctype html>"));
    }

    #[tokio::test]
    async fn exporting_a_missing_design_returns_404() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "GET",
            "/designs/missing/export",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn history_lists_snapshots_and_restores_one() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/overview/history",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "[]");
        let renamed = SAMPLE_DESIGN.replace("Swift Design Overview", "Second Title");
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(&renamed),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(
            test_application(&directory),
            "GET",
            "/designs/overview/history",
            None,
        )
        .await;
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        let stamp = rows[0]["stamp"].as_str().unwrap().to_owned();
        assert!(rows[0]["saved_at"].as_str().unwrap().ends_with('Z'));
        assert!(rows[0]["size_bytes"].as_u64().unwrap() > 0);
        let (status, _) = send(
            test_application(&directory),
            "POST",
            &format!("/designs/overview/history/{stamp}/restore"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(
            test_application(&directory),
            "GET",
            "/designs/overview",
            None,
        )
        .await;
        assert!(body.contains("Swift Design Overview"));
        let (_, body) = send(
            test_application(&directory),
            "GET",
            "/designs/overview/history",
            None,
        )
        .await;
        let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn history_rejects_unknown_designs_and_bad_stamps() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "GET",
            "/designs/missing/history",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/designs/overview/history/2000-01-01T00-00-00Z/restore",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains(
            "unknown history stamp `2000-01-01T00-00-00Z`: run GET /designs/overview/history for the list"
        ));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/designs/overview/history/..%2Fdesign/restore",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("invalid history stamp"));
    }

    async fn send_user_put(application: Router, uri: &str, json: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .method("PUT")
            .uri(uri)
            .header("content-type", "application/json")
            .header("x-swift-design-author", "user")
            .body(Body::from(json.to_owned()))
            .unwrap();
        let response = application.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn sample_design_with_title(title: &str) -> String {
        let mut design: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
        design["title"] = serde_json::json!(title);
        design.to_string()
    }

    #[tokio::test]
    async fn user_saves_mark_changed_fields_and_agent_saves_clear_them() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/talk",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, _) = send_user_put(
            test_application(&directory),
            "/designs/talk",
            &sample_design_with_title("Edited by hand"),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/talk/authors",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let authors: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(authors["user_paths"], serde_json::json!(["title"]));

        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/talk",
            Some(&sample_design_with_title("Agent rewrote this")),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(
            test_application(&directory),
            "GET",
            "/designs/talk/authors",
            None,
        )
        .await;
        let authors: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(authors["user_paths"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn renaming_a_project_moves_its_designs_and_the_brief() {
        let directory = TempDir::new().unwrap();
        for id in ["talk", "talk-candidate-1", "other"] {
            let uri = format!("/designs/{id}");
            send(
                test_application(&directory),
                "PUT",
                &uri,
                Some(SAMPLE_DESIGN),
            )
            .await;
        }
        send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"A talk.","variations":2,"project":"talk"}"#),
        )
        .await;
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/projects/talk/rename",
            Some(r#"{"name":"other"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/projects/talk/rename",
            Some(r#"{"name":"pitch"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"name\":\"pitch\""));
        for id in ["pitch", "pitch-candidate-1", "other"] {
            let uri = format!("/designs/{id}");
            let (status, _) = send(test_application(&directory), "GET", &uri, None).await;
            assert_eq!(status, StatusCode::OK, "{id}");
        }
        let (status, _) = send(test_application(&directory), "GET", "/designs/talk", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(brief["project"], "pitch");
    }

    #[tokio::test]
    async fn messages_append_to_the_brief_and_need_a_brief_first() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"content":"Shorter, please."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"A talk.","variations":1}"#),
        )
        .await;
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"content":"Shorter, please."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"role":"assistant","content":"Done."}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"role":"robot","content":"x"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (_, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(brief["messages"][0]["role"], "user");
        assert_eq!(brief["messages"][0]["content"], "Shorter, please.");
        assert_eq!(brief["messages"][1]["role"], "assistant");
    }

    #[tokio::test]
    async fn continue_messages_carry_the_action_and_need_a_design() {
        let directory = TempDir::new().unwrap();
        send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"A talk.","variations":2}"#),
        )
        .await;
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"content":"Continue.","action":"continue"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("design id"));
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"content":"Continue.","design":"talk-candidate-1","action":"extend"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("`extend` is unknown"));
        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/briefs/messages",
            Some(r#"{"content":"Continue.","design":"talk-candidate-1","action":"continue"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(brief["messages"][0]["action"], "continue");
        assert_eq!(brief["messages"][0]["design"], "talk-candidate-1");
        assert_eq!(brief["preview"], true);
    }

    #[tokio::test]
    async fn question_answers_land_in_the_brief_and_close_the_questions() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/briefs",
            Some(r#"{"prompt":"A talk.","variations":2}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/questions",
            Some(r#"{"questions":[{"question":"Who is in the room?","options":["Engineers","Leadership"]}]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(test_application(&directory), "GET", "/questions", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Who is in the room?"));

        let (status, _) = send(
            test_application(&directory),
            "POST",
            "/questions/answers",
            Some(r#"{"answers":[{"question":"Who is in the room?","answer":"Engineers"}]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(test_application(&directory), "GET", "/questions", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, body) = send(test_application(&directory), "GET", "/briefs", None).await;
        let brief: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(brief["prompt"], "A talk.");
        assert_eq!(brief["answers"][0]["question"], "Who is in the room?");
        assert_eq!(brief["answers"][0]["answer"], "Engineers");
    }

    #[tokio::test]
    async fn rejects_an_empty_question_list() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "PUT",
            "/questions",
            Some(r#"{"questions":[]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("between 1 and 5"));
    }

    #[tokio::test]
    async fn only_the_editable_preview_carries_the_editing_script() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/talk",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, plain) = send(
            test_application(&directory),
            "GET",
            "/designs/talk/render",
            None,
        )
        .await;
        assert!(!plain.contains("swift-design-edit"));
        let (_, editable) = send(
            test_application(&directory),
            "GET",
            "/designs/talk/render?editable=true",
            None,
        )
        .await;
        assert!(editable.contains("swift-design-html"));
    }

    #[tokio::test]
    async fn renders_a_stored_design() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, body) = send(
            test_application(&directory),
            "GET",
            "/designs/overview/render",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!doctype html>"));
    }

    #[tokio::test]
    async fn a_design_can_be_saved_as_a_template_and_listed() {
        let directory = TempDir::new().unwrap();
        let (status, _) = send(
            test_application(&directory),
            "PUT",
            "/designs/overview",
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/templates",
            Some(r#"{"design_id":"overview","name":"Midnight Finance"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
        let id = saved["id"].as_str().unwrap().to_owned();
        assert_eq!(saved["name"], "Midnight Finance");

        let (status, body) = send(test_application(&directory), "GET", "/templates", None).await;
        assert_eq!(status, StatusCode::OK);
        let listed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);

        let (status, body) = send(
            test_application(&directory),
            "GET",
            &format!("/templates/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let template: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(template["source_design"], "overview");
        assert!(template["theme"]["colors"]["accent"].is_string());
        assert!(!template["screens"].as_array().unwrap().is_empty());

        let (status, _) = send(
            test_application(&directory),
            "DELETE",
            &format!("/templates/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            test_application(&directory),
            "GET",
            &format!("/templates/{id}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn saving_a_template_reports_a_missing_design_and_an_empty_name() {
        let directory = TempDir::new().unwrap();
        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/templates",
            Some(r#"{"design_id":"missing","name":"Nice"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("no design `missing`"));

        let (status, body) = send(
            test_application(&directory),
            "POST",
            "/templates",
            Some(r#"{"design_id":"missing","name":"   "}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("template name"));
    }
}
