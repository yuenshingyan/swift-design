//! Shared test helpers: a test application, request helpers, and a fake
//! model provider.
//!
//! The fake provider is a real HTTP boundary: an axum server on an
//! ephemeral port that returns canned OpenAI chat-completion replies.
//! The engines read it exactly as they read a real provider, so their
//! tests exercise the whole request path without a network.
#![allow(clippy::unwrap_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::model_client::{ModelConfiguration, ProviderAuth, WireFormat};
use crate::sessions::{RunOptions, SessionStore};
use crate::{AppState, router};

/// The canonical valid design, as JSON.
pub(crate) const SAMPLE_DESIGN: &str = include_str!("../../../fixtures/sample-design.json");

/// The canonical valid deck, as JSON.
pub(crate) const SAMPLE_DECK: &str = include_str!("../../../fixtures/sample-deck.json");

/// The canonical valid deck, parsed.
pub(crate) fn sample_deck() -> design_model::Deck {
    serde_json::from_str(SAMPLE_DECK).unwrap()
}

/// The canonical valid document, as JSON.
pub(crate) const SAMPLE_DOCUMENT: &str = include_str!("../../../fixtures/sample-document.json");

/// The canonical valid document, parsed.
pub(crate) fn sample_document() -> design_model::Document {
    serde_json::from_str(SAMPLE_DOCUMENT).unwrap()
}

/// The canonical valid social, as JSON.
pub(crate) const SAMPLE_SOCIAL: &str = include_str!("../../../fixtures/sample-social.json");

/// The canonical valid social, parsed.
pub(crate) fn sample_social() -> design_model::Social {
    serde_json::from_str(SAMPLE_SOCIAL).unwrap()
}

/// The canonical valid print, as JSON.
pub(crate) const SAMPLE_PRINT: &str = include_str!("../../../fixtures/sample-print.json");

/// The canonical valid print, parsed.
pub(crate) fn sample_print() -> design_model::Print {
    serde_json::from_str(SAMPLE_PRINT).unwrap()
}

/// The canonical valid mailing, as JSON.
pub(crate) const SAMPLE_MAILING: &str = include_str!("../../../fixtures/sample-mailing.json");

/// The canonical valid mailing, parsed.
pub(crate) fn sample_mailing() -> design_model::Mailing {
    serde_json::from_str(SAMPLE_MAILING).unwrap()
}

/// The multipart boundary the upload helper uses.
pub(crate) const MULTIPART_BOUNDARY: &str = "swiftdesignboundary";

/// Builds a test application over `directory`. It has no custom command
/// and no configured model, so an auto-started run no-ops and the
/// session keeps its state. Tests that need a real run use
/// `application_with_command`.
pub(crate) fn test_application(directory: &TempDir) -> Router {
    application_with_command(directory, None)
}

/// Builds a test application whose runner uses `command`, or the
/// built-in engine when `None`.
pub(crate) fn application_with_command(directory: &TempDir, command: Option<String>) -> Router {
    let changes = crate::events::ChangeNotifier::new();
    let designs = DesignStore::new(directory.path().join("designs")).with_history(
        crate::history::HistoryStore::new(directory.path().join("history")),
    );
    let decks = DeckStore::new(directory.path().join("decks")).with_history(
        crate::history::HistoryStore::new(directory.path().join("deck-history")),
    );
    let documents = DocumentStore::new(directory.path().join("documents")).with_history(
        crate::history::HistoryStore::new(directory.path().join("document-history")),
    );
    let socials = SocialStore::new(directory.path().join("socials")).with_history(
        crate::history::HistoryStore::new(directory.path().join("social-history")),
    );
    let prints = PrintStore::new(directory.path().join("prints")).with_history(
        crate::history::HistoryStore::new(directory.path().join("print-history")),
    );
    let mailings = MailingStore::new(directory.path().join("mailings")).with_history(
        crate::history::HistoryStore::new(directory.path().join("mailing-history")),
    );
    let sessions = SessionStore::new(directory.path().join("data/sessions"));
    let settings = crate::settings::SettingsStore::new(
        directory.path().join("data/settings.json"),
        "127.0.0.1:3000".to_owned(),
    );
    let agent = crate::agent_runs::AgentRunner::new(
        command,
        settings.clone(),
        designs.clone(),
        sessions.clone(),
        "http://127.0.0.1:3000".to_owned(),
        changes.clone(),
    )
    .with_decks(decks.clone())
    .with_documents(documents.clone())
    .with_socials(socials.clone())
    .with_prints(prints.clone())
    .with_mailings(mailings.clone());
    router(AppState {
        designs,
        decks,
        documents,
        socials,
        prints,
        mailings,
        uploads: UploadStore::new(directory.path().join("uploads")),
        sessions,
        settings,
        agent,
        changes,
        templates: crate::templates::TemplateStore::new(directory.path().join("templates")),
        ui: crate::static_files::UiDirectory(directory.path().join("ui")),
    })
}

use crate::decks::DeckStore;
use crate::designs::DesignStore;
use crate::documents::DocumentStore;
use crate::mailings::MailingStore;
use crate::prints::PrintStore;
use crate::socials::SocialStore;
use crate::uploads::UploadStore;

/// Sends a request with an optional content type and body.
pub(crate) async fn send_raw(
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

/// Sends a JSON request, or a bodyless one when `json` is `None`.
pub(crate) async fn send(
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

/// Sends a request with the `x-swift-design-author: user` header, for
/// user-authored design saves.
pub(crate) async fn send_user_put(
    application: Router,
    uri: &str,
    json: &str,
) -> (StatusCode, String) {
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

/// The multipart body for one uploaded file.
pub(crate) fn multipart_body(file_name: &str, content: &str) -> Body {
    Body::from(format!(
        "--{MULTIPART_BOUNDARY}\r\n\
         content-disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n\
         content-type: application/octet-stream\r\n\r\n\
         {content}\r\n--{MULTIPART_BOUNDARY}--\r\n"
    ))
}

/// Uploads one file to the application.
pub(crate) async fn send_upload(
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

/// A copy of the sample design with an empty title and no screens, for
/// validation tests.
pub(crate) fn invalid_sample_design() -> String {
    let mut design: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    design["title"] = serde_json::json!("");
    design["screens"] = serde_json::json!([]);
    design.to_string()
}

/// Creates a demo session and drives it to the generating state, so
/// design writes are allowed. Asserts the two calls succeed.
pub(crate) async fn open_generating_session(application: &Router, id: &str) {
    let body = format!("{{\"id\":\"{id}\",\"request\":\"Design {id}.\"}}");
    open_generating_session_with(application, id, &body).await;
}

/// Creates a deck session and drives it to the generating state, so
/// deck writes are allowed.
pub(crate) async fn open_generating_deck_session(application: &Router, id: &str) {
    let body = format!(
        "{{\"id\":\"{id}\",\"request\":\"A deck about {id}.\",\"artifact_kind\":\"deck\"}}"
    );
    open_generating_session_with(application, id, &body).await;
}

/// Creates a document session and drives it to the generating state, so
/// document writes are allowed.
pub(crate) async fn open_generating_document_session(application: &Router, id: &str) {
    let body = format!(
        "{{\"id\":\"{id}\",\"request\":\"A report about {id}.\",\"artifact_kind\":\"document\"}}"
    );
    open_generating_session_with(application, id, &body).await;
}

/// Creates a social session and drives it to the generating state, so
/// social writes are allowed.
pub(crate) async fn open_generating_social_session(application: &Router, id: &str) {
    let body = format!(
        "{{\"id\":\"{id}\",\"request\":\"A carousel about {id}.\",\"artifact_kind\":\"social\"}}"
    );
    open_generating_session_with(application, id, &body).await;
}

/// Creates a print session and drives it to the generating state, so
/// print writes are allowed.
pub(crate) async fn open_generating_print_session(application: &Router, id: &str) {
    let body = format!(
        "{{\"id\":\"{id}\",\"request\":\"A poster about {id}.\",\"artifact_kind\":\"print\"}}"
    );
    open_generating_session_with(application, id, &body).await;
}

/// Creates a mailing session and drives it to the generating state,
/// so mailing writes are allowed.
pub(crate) async fn open_generating_mailing_session(application: &Router, id: &str) {
    let body = format!(
        "{{\"id\":\"{id}\",\"request\":\"An email about {id}.\",\"artifact_kind\":\"mailing\"}}"
    );
    open_generating_session_with(application, id, &body).await;
}

/// Creates a session from `body` and opens it for generation.
async fn open_generating_session_with(application: &Router, id: &str, body: &str) {
    let (status, _) = send(application.clone(), "POST", "/sessions", Some(body)).await;
    assert!(
        status == StatusCode::CREATED,
        "creating session {id}: {status}"
    );
    let (status, _) = send(
        application.clone(),
        "POST",
        &format!("/sessions/{id}/generate"),
        None,
    )
    .await;
    assert!(
        status == StatusCode::OK,
        "opening session {id} for generation: {status}"
    );
}

/// Low-effort options: no polish rounds and few fix rounds, so engine
/// tests never invoke Chrome.
pub(crate) fn low_effort_options() -> RunOptions {
    RunOptions {
        effort: "low".to_owned(),
        variations: Some(1),
        ..RunOptions::default()
    }
}

/// One canned reply the fake provider returns.
pub(crate) enum FakeReply {
    /// A successful assistant message with this content.
    Text(String),
}

/// Shared state of the fake provider: the queue of replies and the
/// bodies it received.
struct FakeState {
    replies: Mutex<VecDeque<FakeReply>>,
    requests: Mutex<Vec<serde_json::Value>>,
}

/// A fake OpenAI-compatible chat provider on an ephemeral port.
pub(crate) struct FakeModelServer {
    url: String,
    state: Arc<FakeState>,
}

impl FakeModelServer {
    /// Starts the server and returns once it is listening.
    pub(crate) async fn start() -> Self {
        let state = Arc::new(FakeState {
            replies: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
        });
        let router = Router::new()
            .route("/v1/chat/completions", post(handle))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            url: format!("http://{address}/v1/chat/completions"),
            state,
        }
    }

    /// Queues one successful reply with `content`.
    pub(crate) fn push_text(&self, content: &str) {
        self.state
            .replies
            .lock()
            .unwrap()
            .push_back(FakeReply::Text(content.to_owned()));
    }

    /// The request bodies the server received, in order.
    pub(crate) fn requests(&self) -> Vec<serde_json::Value> {
        self.state.requests.lock().unwrap().clone()
    }

    /// A model configuration pointing at this server. The provider name
    /// `fake` and model `fake-model` have no vision, so the polish pass
    /// never screenshots.
    pub(crate) fn configuration(&self) -> ModelConfiguration {
        ModelConfiguration {
            provider: "fake".to_owned(),
            chat_url: self.url.clone(),
            auth: ProviderAuth::None,
            wire: WireFormat::OpenAiChat,
            model: "fake-model".to_owned(),
        }
    }
}

/// Serves the next canned reply as a plain (non-streaming) JSON body,
/// which the engine reads through its fallback path.
async fn handle(
    State(state): State<Arc<FakeState>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    state.requests.lock().unwrap().push(body);
    let reply = state.replies.lock().unwrap().pop_front();
    match reply {
        Some(FakeReply::Text(content)) => axum::Json(serde_json::json!({
            "id": "fake",
            "choices": [{ "message": { "role": "assistant", "content": content } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5 },
        }))
        .into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "no canned reply queued".to_owned(),
        )
            .into_response(),
    }
}
