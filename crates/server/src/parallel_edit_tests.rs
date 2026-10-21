#![allow(clippy::unwrap_used)]

#[path = "parallel_edit_tests/flows.rs"]
mod flows;
#[path = "parallel_edit_tests/persistence.rs"]
mod persistence;
use persistence::verify_queued_edit_baseline;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{Barrier, Notify};

use design_model::ArtifactKind;

use super::*;
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::edit_report::EDIT_REPORT_PREFIX;
use crate::events::ChangeNotifier;
use crate::generation::{ArtifactRequest, GenerationEngine, GenerationOutcome, GenerationStop};
use crate::history::HistoryStore;
use crate::model_client::{LogSink, ModelClient, ModelConfiguration, ProviderAuth, WireFormat};
use crate::polish::DemoDomRunner;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{
    SAMPLE_ARTWORK, SAMPLE_CAMPAIGN, SAMPLE_DECK, SAMPLE_DESIGN, SAMPLE_DOCUMENT, SAMPLE_MAILING,
    SAMPLE_PRINT, SAMPLE_SOCIAL, clean_demo_dom_runner, low_effort_options, sample_artwork,
    sample_campaign, sample_deck, sample_document, sample_mailing, sample_print, sample_social,
};

const EDIT_PLAN: &str = r#"{"instruction":"Update all screens.","screens":[],"minimum_new_screens":0,"requirements":["Update all screens"]}"#;

#[derive(Clone)]
struct ParallelFakeModel {
    url: String,
    state: Arc<ParallelFakeState>,
}

struct ParallelFakeState {
    requests: Mutex<Vec<Value>>,
    replies: Mutex<VecDeque<String>>,
    active_workers: AtomicUsize,
    max_active_workers: AtomicUsize,
    worker_count: AtomicUsize,
    coordinator_count: AtomicUsize,
    baseline_file: Mutex<Option<(PathBuf, Vec<u8>)>>,
    baseline_changed_during_requests: AtomicBool,
    release_workers: Option<Arc<Barrier>>,
    first_worker_batch_started: Arc<Notify>,
    auto_worker: Mutex<Option<(ArtifactKind, Value, String)>>,
}

impl ParallelFakeModel {
    async fn start(release_workers: Option<Arc<Barrier>>) -> Self {
        let state = Arc::new(ParallelFakeState {
            requests: Mutex::new(Vec::new()),
            replies: Mutex::new(VecDeque::new()),
            active_workers: AtomicUsize::new(0),
            max_active_workers: AtomicUsize::new(0),
            worker_count: AtomicUsize::new(0),
            coordinator_count: AtomicUsize::new(0),
            baseline_file: Mutex::new(None),
            baseline_changed_during_requests: AtomicBool::new(false),
            release_workers,
            first_worker_batch_started: Arc::new(Notify::new()),
            auto_worker: Mutex::new(None),
        });
        let router = Router::new()
            .route("/v1/chat/completions", post(handle_parallel_fake))
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

    fn configuration(&self) -> ModelConfiguration {
        ModelConfiguration {
            provider: "fake".to_owned(),
            chat_url: self.url.clone(),
            auth: ProviderAuth::None,
            wire: WireFormat::OpenAiChat,
            model: "fake-model".to_owned(),
        }
    }

    fn push_text(&self, content: String) {
        self.state.replies.lock().unwrap().push_back(content);
    }

    fn requests(&self) -> Vec<Value> {
        self.state.requests.lock().unwrap().clone()
    }

    fn auto_worker_patches(&self, kind: ArtifactKind, original: &Value, marker: &str) {
        *self.state.auto_worker.lock().unwrap() = Some((kind, original.clone(), marker.to_owned()));
    }

    async fn wait_for_workers(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if self.state.worker_count.load(Ordering::SeqCst) >= count {
                    return;
                }
                self.state.first_worker_batch_started.notified().await;
            }
        })
        .await
        .unwrap();
    }

    async fn watch_baseline(&self, path: &Path) {
        let bytes = tokio::fs::read(path).await.unwrap();
        *self.state.baseline_file.lock().unwrap() = Some((path.to_owned(), bytes));
    }
}

async fn handle_parallel_fake(
    State(state): State<Arc<ParallelFakeState>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let prompt = request_text(&body);
    let is_worker = prompt.contains("Assigned unit index (zero-based):");
    let is_coordinator = prompt.contains("Coordinate the parallel edits.");
    if is_worker || is_coordinator {
        let baseline = state.baseline_file.lock().unwrap().clone();
        if let Some((path, bytes)) = baseline
            && tokio::fs::read(path).await.unwrap() != bytes
        {
            state
                .baseline_changed_during_requests
                .store(true, Ordering::SeqCst);
        }
    }
    if is_worker {
        let active = state.active_workers.fetch_add(1, Ordering::SeqCst) + 1;
        update_max(&state.max_active_workers, active);
        let ordinal = state.worker_count.fetch_add(1, Ordering::SeqCst) + 1;
        state.first_worker_batch_started.notify_one();
        if ordinal <= PARALLEL_EDIT_LIMIT
            && let Some(barrier) = &state.release_workers
        {
            barrier.wait().await;
        }
    } else if prompt.contains("Coordinate the parallel edits.") {
        state.coordinator_count.fetch_add(1, Ordering::SeqCst);
    }

    state.requests.lock().unwrap().push(body);
    let reply = if is_worker {
        let auto = state.auto_worker.lock().unwrap().clone();
        match auto.and_then(|(kind, original, marker)| {
            assigned_index(&prompt)
                .map(|index| patch_for(kind, &original, index, &format!("{marker} {index}")))
        }) {
            Some(content) => Some(content),
            None => state.replies.lock().unwrap().pop_front(),
        }
    } else {
        state.replies.lock().unwrap().pop_front()
    };
    if is_worker {
        state.active_workers.fetch_sub(1, Ordering::SeqCst);
    }
    match reply {
        Some(content) => axum::Json(json!({
            "id": "fake",
            "choices": [{ "message": { "role": "assistant", "content": content } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
        }))
        .into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "no canned reply queued").into_response(),
    }
}

fn update_max(maximum: &AtomicUsize, candidate: usize) {
    let mut current = maximum.load(Ordering::SeqCst);
    while candidate > current {
        match maximum.compare_exchange(current, candidate, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

fn request_text(body: &Value) -> String {
    serde_json::to_string(body).unwrap()
}

fn assigned_index(prompt: &str) -> Option<usize> {
    let marker = "Assigned unit index (zero-based): ";
    let start = prompt.find(marker)? + marker.len();
    let digits: String = prompt[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn sample_design() -> design_model::Design {
    serde_json::from_str(SAMPLE_DESIGN).unwrap()
}

fn succeeded<T>(result: Result<T, GenerationStop>) -> T {
    match result {
        Ok(value) => value,
        Err(GenerationStop::Failed(message)) => {
            panic!("generation failed: {message}")
        }
        Err(GenerationStop::NeedsClarification(_)) => {
            panic!("generation asked questions")
        }
    }
}

fn log_sink() -> LogSink {
    Arc::new(|_line| {})
}

fn test_engine(configuration: ModelConfiguration) -> GenerationEngine {
    let directory = TempDir::new().unwrap();
    GenerationEngine::new(
        configuration,
        DesignStore::new(directory.path().join("designs"))
            .with_history(HistoryStore::new(directory.path().join("history"))),
        SessionStore::new(directory.path().join("sessions")),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    )
    .with_demo_dom_runner(clean_demo_dom_runner())
}

struct ReviewingCandidate {
    directory: TempDir,
    designs: DesignStore,
    sessions: SessionStore,
    engine: GenerationEngine,
}

impl ReviewingCandidate {
    fn new(fake: &ParallelFakeModel) -> Self {
        let directory = TempDir::new().unwrap();
        let designs = DesignStore::new(directory.path().join("designs"))
            .with_history(HistoryStore::new(directory.path().join("history")));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let engine = GenerationEngine::new(
            fake.configuration(),
            designs.clone(),
            sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        )
        .with_demo_dom_runner(clean_demo_dom_runner());
        Self {
            directory,
            designs,
            sessions,
            engine,
        }
    }

    async fn seed(&self, original: &design_model::Design, instruction: &str) {
        self.designs
            .save("talk-candidate-1", original)
            .await
            .unwrap();
        self.sessions
            .create(
                NewSession::demo("talk", "Talk", "A workspace.").with_options(low_effort_options()),
            )
            .await
            .unwrap();
        for event in [
            design_model::WorkflowEvent::QuestionsAsked,
            design_model::WorkflowEvent::GenerationStarted,
            design_model::WorkflowEvent::GenerationSucceeded,
        ] {
            self.sessions.apply("talk", event).await.unwrap();
        }
        self.sessions
            .append_message(
                "talk",
                ChatMessage::user(instruction, Some("talk-candidate-1")),
            )
            .await
            .unwrap();
    }

    fn path(&self) -> PathBuf {
        self.directory.path().join("designs/talk-candidate-1.json")
    }
}

fn unit_key(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Demo => "screens",
        ArtifactKind::Deck => "slides",
        ArtifactKind::Document => "pages",
        ArtifactKind::Social => "frames",
        ArtifactKind::Print => "sheets",
        ArtifactKind::Mailing => "emails",
        ArtifactKind::Campaign => "ads",
        ArtifactKind::Artwork => "covers",
    }
}

fn singular_key(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Demo => "screen",
        ArtifactKind::Deck => "slide",
        ArtifactKind::Document => "page",
        ArtifactKind::Social => "frame",
        ArtifactKind::Print => "sheet",
        ArtifactKind::Mailing => "email",
        ArtifactKind::Campaign => "ad",
        ArtifactKind::Artwork => "cover",
    }
}

fn patch_for(kind: ArtifactKind, original: &Value, index: usize, marker: &str) -> String {
    let collection = unit_key(kind);
    let singular = singular_key(kind);
    let mut unit = original[collection][index].clone();
    unit["notes"] = json!(marker);
    json!({ collection: [{ "index": index, singular: unit }] }).to_string()
}

fn duplicate_patch_for(kind: ArtifactKind, original: &Value, index: usize) -> String {
    let collection = unit_key(kind);
    let singular = singular_key(kind);
    let unit = original[collection][index].clone();
    json!({ collection: [
        { "index": index, singular: unit.clone() },
        { "index": index, singular: unit }
    ] })
    .to_string()
}

fn delete_patch_for(kind: ArtifactKind, index: usize) -> String {
    json!({ unit_key(kind): [{ "index": index, singular_key(kind): Value::Null }] }).to_string()
}

fn insert_patch_for(kind: ArtifactKind, original: &Value, index: usize) -> String {
    let collection = unit_key(kind);
    let singular = singular_key(kind);
    let unit = original[collection][index].clone();
    json!({ collection: [{ "index": index, "insert": true, singular: unit }] }).to_string()
}

fn request_for<'a, T: ParallelEditable>(original: &'a T) -> ArtifactRequest<'a, T> {
    ArtifactRequest {
        effort: "low".to_owned(),
        label: "edit".to_owned(),
        parse: Box::new(move |content| T::apply_edit(original, content)),
        progress: None,
        live: None,
    }
}

fn edit_input<'a>(artifact_json: &'a str, instruction: &'a str, note: &'a str) -> EditInput<'a> {
    EditInput {
        instruction,
        artifact_json,
        note,
        findings: &[],
        conversation: "",
    }
}

async fn request_edit_for<T>(
    engine: &GenerationEngine,
    original: &T,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<T, GenerationStop>
where
    T: ParallelEditable,
{
    let client = ModelClient::build_http_client().unwrap();
    let artifact_json = serde_json::to_string(original).unwrap();
    let input = edit_input(&artifact_json, "Make the selected units clearer.", "");
    let messages = vec![json!({
        "role": "user",
        "content": format!("Here is the edit baseline:\n{artifact_json}\nApply the requested change.")
    })];
    let request = request_for(original);
    let execution = EditExecution {
        original,
        messages,
        input: &input,
        indexes,
        is_fresh,
        request: &request,
    };
    engine.request_edit(&client, execution, &log_sink()).await
}
