#![allow(clippy::unwrap_used)]

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

#[test]
fn guard_rejects_malformed_or_reversed_json_delimiters_without_panicking() {
    for content in ["no JSON", "} before {", "{not JSON}"] {
        assert!(guard_page_patch(content, ArtifactKind::Demo, &[0]).is_err());
    }
    assert!(guard_page_patch("{}", ArtifactKind::Demo, &[0]).is_ok());
}

#[test]
fn worker_and_coordination_prompts_preserve_attachment_parts() {
    let original = sample_design();
    let snapshot = serde_json::to_value(&original).unwrap();
    let artifact_json = serde_json::to_string(&original).unwrap();
    let note = "Old focused edit note.";
    let input = edit_input(&artifact_json, "Update the chosen screens.", note);
    let messages = vec![json!({ "role": "user", "content": [
        { "type": "text", "text": format!("{artifact_json}\n{note}") },
        { "type": "image_url", "image_url": { "url": "image boundary" } },
        { "type": "text", "text": "User's uploaded requirements" }
    ] })];
    let request = request_for(&original);
    let edit = EditExecution {
        original: &original,
        messages: messages.clone(),
        input: &input,
        indexes: &[0, 2],
        is_fresh: false,
        request: &request,
    };
    let worker = page_messages(&edit, &snapshot, 0).unwrap();
    let coordinator = coordination_messages(&edit, &original).unwrap();
    for rewritten in [&worker, &coordinator] {
        assert_eq!(rewritten[0]["content"][1], messages[0]["content"][1]);
        assert_eq!(rewritten[0]["content"][2], messages[0]["content"][2]);
        assert!(
            !rewritten[0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(note)
        );
    }
    assert!(
        request_text(&json!({"messages": worker})).contains("Assigned unit index (zero-based): 0")
    );
    assert!(
        request_text(&json!({"messages": coordinator})).contains("Coordinate the parallel edits.")
    );
}

#[test]
fn guard_rejects_shared_meta_changes_for_scoped_patch() {
    let original: Value = serde_json::from_str(SAMPLE_DOCUMENT).unwrap();
    let mut patch: Value =
        serde_json::from_str(&patch_for(ArtifactKind::Document, &original, 1, "ok")).unwrap();
    patch["title"] = json!("Changed title");
    let error = guard_page_patch(&patch.to_string(), ArtifactKind::Document, &[1]).unwrap_err();
    assert!(error.contains("shared") || error.contains("title"));
}

#[test]
fn a_page_worker_cannot_change_shared_app_window_bounds() {
    let patch = json!({"app_window": {"left": 80, "top": 60, "width": 1280, "height": 780}});
    assert!(guard_page_patch(&patch.to_string(), ArtifactKind::Demo, &[0]).is_err());
}

#[test]
fn guard_rejects_cross_unit_changes_for_scoped_patch() {
    let original: Value = serde_json::from_str(SAMPLE_SOCIAL).unwrap();
    let patch = patch_for(ArtifactKind::Social, &original, 0, "wrong frame");
    let error = guard_page_patch(&patch, ArtifactKind::Social, &[1]).unwrap_err();
    assert!(error.contains("assigned") || error.contains("other units"));
}

#[test]
fn guard_rejects_insert_delete_and_duplicate_operations() {
    let original: Value = serde_json::from_str(SAMPLE_PRINT).unwrap();
    let insert_error = guard_page_patch(
        &insert_patch_for(ArtifactKind::Print, &original, 0),
        ArtifactKind::Print,
        &[0],
    )
    .unwrap_err();
    assert!(insert_error.contains("insert"));

    let delete_error = guard_page_patch(
        &delete_patch_for(ArtifactKind::Print, 0),
        ArtifactKind::Print,
        &[0],
    )
    .unwrap_err();
    assert!(delete_error.contains("delete") || delete_error.contains("null"));

    let duplicate_error = guard_page_patch(
        &duplicate_patch_for(ArtifactKind::Print, &original, 0),
        ArtifactKind::Print,
        &[0],
    )
    .unwrap_err();
    assert!(duplicate_error.contains("assigned") || duplicate_error.contains("at most once"));
}

#[test]
fn focused_worker_json_shows_one_index_and_omits_markup_for_regeneration() {
    let original: Value = serde_json::from_str(SAMPLE_DECK).unwrap();
    let focused = focused_worker_json(&original, ArtifactKind::Deck, 2, false).unwrap();
    let focused_json: Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(focused_json["slides"].as_array().unwrap().len(), 1);
    assert_eq!(focused_json["slides"][0]["index"], json!(2));
    assert!(focused.contains("html"));
    assert!(focused.contains("css"));

    let fresh = focused_worker_json(&original, ArtifactKind::Deck, 2, true).unwrap();
    assert!(fresh.contains("index"));
    assert!(!fresh.contains("<"));
    assert!(!fresh.contains(".slide"));
}

#[test]
fn all_kinds_use_their_patch_vocabulary_with_actual_fixtures() {
    let cases = [
        (
            ArtifactKind::Demo,
            serde_json::from_str::<Value>(SAMPLE_DESIGN).unwrap(),
        ),
        (
            ArtifactKind::Deck,
            serde_json::from_str::<Value>(SAMPLE_DECK).unwrap(),
        ),
        (
            ArtifactKind::Document,
            serde_json::from_str::<Value>(SAMPLE_DOCUMENT).unwrap(),
        ),
        (
            ArtifactKind::Social,
            serde_json::from_str::<Value>(SAMPLE_SOCIAL).unwrap(),
        ),
        (
            ArtifactKind::Print,
            serde_json::from_str::<Value>(SAMPLE_PRINT).unwrap(),
        ),
        (
            ArtifactKind::Mailing,
            serde_json::from_str::<Value>(SAMPLE_MAILING).unwrap(),
        ),
        (
            ArtifactKind::Campaign,
            serde_json::from_str::<Value>(SAMPLE_CAMPAIGN).unwrap(),
        ),
        (
            ArtifactKind::Artwork,
            serde_json::from_str::<Value>(SAMPLE_ARTWORK).unwrap(),
        ),
    ];

    for (kind, artifact) in cases {
        let focused = focused_worker_json(&artifact, kind, 0, false).unwrap();
        let focused_json: Value = serde_json::from_str(&focused).unwrap();
        assert!(focused_json.get(unit_key(kind)).is_some(), "{kind:?}");
        assert_eq!(
            focused_json[unit_key(kind)].as_array().unwrap().len(),
            1,
            "{kind:?}"
        );
        assert_eq!(
            focused_json[unit_key(kind)][0]["index"],
            json!(0),
            "{kind:?}"
        );

        let patch = patch_for(kind, &artifact, 0, "changed");
        guard_page_patch(&patch, kind, &[0]).unwrap();
        assert!(patch.contains(unit_key(kind)), "{kind:?}");
        assert!(patch.contains(singular_key(kind)), "{kind:?}");
    }
}

#[test]
fn merge_worker_results_preserves_original_order_from_out_of_order_workers() {
    let original = sample_document();
    let original_json = serde_json::to_value(&original).unwrap();
    let second = <design_model::Document as ParallelEditable>::apply_edit(
        &original,
        &patch_for(ArtifactKind::Document, &original_json, 1, "second first"),
    )
    .unwrap();
    let first = <design_model::Document as ParallelEditable>::apply_edit(
        &original,
        &patch_for(ArtifactKind::Document, &original_json, 0, "first second"),
    )
    .unwrap();

    let merged = merge_worker_results(original.clone(), vec![(1, second), (0, first)]).unwrap();
    assert_eq!(merged.pages[0].notes.as_deref(), Some("first second"));
    assert_eq!(merged.pages[1].notes.as_deref(), Some("second first"));
    assert_eq!(merged.pages[2], original.pages[2]);
}

#[tokio::test]
async fn multi_unit_edit_runs_workers_before_one_serial_coordination_call() {
    let original = sample_document();
    let original_json = serde_json::to_value(&original).unwrap();
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(ArtifactKind::Document, &original_json, "worker");
    fake.push_text(patch_for(
        ArtifactKind::Document,
        &original_json,
        0,
        "coordinated zero",
    ));
    let engine = test_engine(fake.configuration());

    let edited = succeeded(request_edit_for(&engine, &original, &[1, 0], false).await);

    assert_eq!(edited.pages[0].notes.as_deref(), Some("coordinated zero"));
    assert_eq!(fake.state.worker_count.load(Ordering::SeqCst), 2);
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 1);
    let requests = fake.requests();
    assert!(request_text(&requests[0]).contains("Assigned unit index (zero-based):"));
    assert!(request_text(&requests[1]).contains("Assigned unit index (zero-based):"));
    assert!(request_text(&requests[2]).contains("Coordinate the parallel edits."));
}

#[tokio::test]
async fn parallel_workers_are_limited_before_coordination() {
    let mut original = sample_design();
    while original.screens.len() < 5 {
        original.screens.push(original.screens[0].clone());
    }
    let original_json = serde_json::to_value(&original).unwrap();
    let barrier = Arc::new(Barrier::new(5));
    let fake = ParallelFakeModel::start(Some(Arc::clone(&barrier))).await;
    fake.auto_worker_patches(ArtifactKind::Demo, &original_json, "worker");
    fake.push_text(patch_for(
        ArtifactKind::Demo,
        &original_json,
        0,
        "coordinator",
    ));
    let engine = test_engine(fake.configuration());
    let edit =
        tokio::spawn(
            async move { request_edit_for(&engine, &original, &[0, 1, 2, 3, 4], false).await },
        );

    fake.wait_for_workers(4).await;
    assert_eq!(fake.state.worker_count.load(Ordering::SeqCst), 4);
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        fake.state.active_workers.load(Ordering::SeqCst),
        PARALLEL_EDIT_LIMIT
    );
    barrier.wait().await;
    let _ = succeeded(
        tokio::time::timeout(Duration::from_secs(2), edit)
            .await
            .unwrap()
            .unwrap(),
    );

    assert_eq!(
        fake.state.max_active_workers.load(Ordering::SeqCst),
        PARALLEL_EDIT_LIMIT
    );
}

#[tokio::test]
async fn a_systemic_edit_saves_only_after_coordination_and_reports_the_final_changes() {
    let mut original = sample_design();
    while original.screens.len() < 5 {
        original.screens.push(original.screens[0].clone());
    }
    let barrier = Arc::new(Barrier::new(PARALLEL_EDIT_LIMIT + 1));
    let fake = ParallelFakeModel::start(Some(Arc::clone(&barrier))).await;
    fake.push_text(r#"{"reply":"Editing the workspace.","edit":true}"#.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(&original).unwrap(),
        "worker",
    );
    fake.push_text(r#"{"title":"Coordinated workspace","screens":[]}"#.to_owned());
    let candidate = ReviewingCandidate::new(&fake);
    candidate.seed(&original, "Update all screens.").await;
    fake.watch_baseline(&candidate.path()).await;
    let engine = candidate.engine.clone();
    let run = tokio::spawn(async move { engine.run("talk", log_sink()).await });
    fake.wait_for_workers(PARALLEL_EDIT_LIMIT).await;
    let mut pending = ChatMessage::user("QUEUED_ONLY_CHANGE", Some("talk-candidate-1"));
    pending.is_queued = true;
    candidate
        .sessions
        .append_message("talk", pending)
        .await
        .unwrap();
    assert_eq!(
        candidate
            .designs
            .load("talk-candidate-1")
            .await
            .unwrap()
            .unwrap(),
        original
    );
    assert!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
    barrier.wait().await;
    let outcome = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let GenerationOutcome::Edited { reports, .. } = outcome else {
        panic!("expected an edit report");
    };
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.title, "Coordinated workspace");
    for (index, screen) in saved.screens.iter().enumerate() {
        assert_eq!(
            screen.notes.as_deref(),
            Some(format!("worker {index}").as_str())
        );
        assert!(reports[0].contains(&format!("Updated notes only for screen {}", index + 1)));
    }
    assert!(reports[0].contains("Shared settings changed: title"));
    assert_eq!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        !fake
            .state
            .baseline_changed_during_requests
            .load(Ordering::SeqCst)
    );
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 1);
    let messages = candidate.sessions.messages("talk").await.unwrap();
    assert!(
        candidate
            .sessions
            .has_queued_messages("talk")
            .await
            .unwrap()
    );
    assert!(
        fake.requests()
            .iter()
            .all(|body| !request_text(body).contains("QUEUED_ONLY_CHANGE"))
    );
    let report = messages.last().unwrap();
    assert!(report.content.contains(&reports[0]));
    let history = candidate.designs.history("talk-candidate-1").await.unwrap();
    assert_eq!(
        report.snapshot_stamps.as_ref().unwrap()["talk-candidate-1"],
        history[0].stamp
    );
    verify_queued_edit_baseline(&candidate, &fake, &saved).await;
}

async fn verify_queued_edit_baseline(
    candidate: &ReviewingCandidate,
    fake: &ParallelFakeModel,
    baseline: &design_model::Design,
) {
    candidate
        .sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    candidate
        .sessions
        .activate_next_message("talk")
        .await
        .unwrap()
        .unwrap();
    fake.watch_baseline(&candidate.path()).await;
    fake.push_text(r#"{"reply":"Applying the queued change.","edit":true}"#.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(baseline).unwrap(),
        "queued worker",
    );
    fake.push_text(r#"{"title":"Queued workspace","screens":[]}"#.to_owned());
    candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .unwrap();
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.title, "Queued workspace");
    let messages = candidate.sessions.messages("talk").await.unwrap();
    let stamp = &messages.last().unwrap().snapshot_stamps.as_ref().unwrap()["talk-candidate-1"];
    let restored = candidate
        .designs
        .restore("talk-candidate-1", stamp)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&restored, baseline);
}

#[tokio::test]
async fn an_unrepaired_layout_after_parallel_editing_does_not_replace_the_saved_candidate() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(r#"{"reply":"Editing selected screens.","edit":true}"#.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(&original).unwrap(),
        "worker",
    );
    fake.push_text(r#"{"title":"Unverified layout","screens":[]}"#.to_owned());
    fake.push_text("{}".to_owned());
    let mut candidate = ReviewingCandidate::new(&fake);
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let coverage: Vec<usize> = (0..design.screens.len()).collect();
        let findings = if design.title == "Unverified layout" {
            "[{&quot;screen&quot;:0,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;clipped text&quot;}]"
        } else {
            "[]"
        };
        let dom = format!(
            "<html data-swift-design-findings=\"{findings}\" data-swift-design-audited-screens=\"{}\">",
            serde_json::to_string(&coverage).unwrap()
        );
        Box::pin(async move { Ok(dom) })
    });
    candidate.engine = candidate.engine.with_demo_dom_runner(runner);
    candidate.seed(&original, "Update every screen.").await;
    let before = tokio::fs::read(candidate.path()).await.unwrap();
    assert!(
        candidate
            .engine
            .clone()
            .run("talk", log_sink())
            .await
            .is_err()
    );
    assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
    assert!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !candidate
            .sessions
            .messages("talk")
            .await
            .unwrap()
            .iter()
            .any(|message| message.content.starts_with("Saved edit report:"))
    );
}

#[tokio::test]
async fn coordination_failure_leaves_the_real_saved_candidate_and_history_unchanged() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(r#"{"reply":"Editing selected screens.","edit":true}"#.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(&original).unwrap(),
        "worker",
    );
    fake.push_text(r#"{"theme":null,"screens":[]}"#.to_owned());
    let candidate = ReviewingCandidate::new(&fake);
    candidate
        .seed(&original, "[screen 1] [screen 2] Update these screens.")
        .await;
    let before = tokio::fs::read(candidate.path()).await.unwrap();
    fake.watch_baseline(&candidate.path()).await;
    assert!(
        candidate
            .engine
            .clone()
            .run("talk", log_sink())
            .await
            .is_err()
    );
    assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
    assert!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !fake
            .state
            .baseline_changed_during_requests
            .load(Ordering::SeqCst)
    );
    let messages = candidate.sessions.messages("talk").await.unwrap();
    assert!(
        !messages
            .iter()
            .any(|message| { message.content.starts_with(EDIT_REPORT_PREFIX) })
    );
}

#[tokio::test]
async fn workers_read_identical_baseline_even_when_responses_are_out_of_order() {
    let original = sample_social();
    let original_json = serde_json::to_value(&original).unwrap();
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(ArtifactKind::Social, &original_json, "worker");
    fake.push_text(patch_for(
        ArtifactKind::Social,
        &original_json,
        0,
        "coordinated",
    ));
    let engine = test_engine(fake.configuration());

    succeeded(request_edit_for(&engine, &original, &[2, 0, 1], false).await);

    let worker_prompts: Vec<String> = fake
        .requests()
        .into_iter()
        .map(|request| request_text(&request))
        .filter(|prompt| prompt.contains("Assigned unit index (zero-based):"))
        .collect();
    assert_eq!(worker_prompts.len(), 3);
    for prompt in worker_prompts {
        assert!(prompt.contains(&original.title));
        assert!(prompt.contains(original.frames[0].notes.as_deref().unwrap_or("")));
        assert!(prompt.contains(original.frames[1].notes.as_deref().unwrap_or("")));
        assert!(prompt.contains(original.frames[2].notes.as_deref().unwrap_or("")));
    }
}

#[tokio::test]
async fn worker_failure_prevents_coordination() {
    let original = sample_print();
    let original_json = serde_json::to_value(&original).unwrap();
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(patch_for(ArtifactKind::Print, &original_json, 0, "ok"));
    fake.push_text(insert_patch_for(ArtifactKind::Print, &original_json, 1));
    let engine = test_engine(fake.configuration());

    let result = request_edit_for(&engine, &original, &[0, 1], false).await;

    assert!(result.is_err());
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn single_unit_edit_uses_the_old_single_request_path() {
    let original = sample_campaign();
    let original_json = serde_json::to_value(&original).unwrap();
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(patch_for(
        ArtifactKind::Campaign,
        &original_json,
        0,
        "single",
    ));
    let engine = test_engine(fake.configuration());

    let edited = succeeded(request_edit_for(&engine, &original, &[0], false).await);

    assert_eq!(edited.ads[0].notes.as_deref(), Some("single"));
    assert_eq!(fake.state.worker_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake.requests().len(), 1);
    assert!(!request_text(&fake.requests()[0]).contains("Assigned unit index (zero-based):"));
}

#[tokio::test]
async fn scoped_coordination_cannot_change_unselected_units_or_shared_values() {
    let original = sample_mailing();
    let original_json = serde_json::to_value(&original).unwrap();
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(ArtifactKind::Mailing, &original_json, "worker");
    let mut bad_coordination: Value = serde_json::from_str(&patch_for(
        ArtifactKind::Mailing,
        &original_json,
        0,
        "coordinator zero",
    ))
    .unwrap();
    bad_coordination["title"] = json!("Bad shared change");
    fake.push_text(bad_coordination.to_string());
    let engine = test_engine(fake.configuration());

    let result = request_edit_for(&engine, &original, &[0, 1], false).await;

    assert!(result.is_err());
    assert!(fake.state.coordinator_count.load(Ordering::SeqCst) >= 1);
}

fn assert_fixture_edit<T: ParallelEditable>(original: T) {
    let baseline = serde_json::to_value(&original).unwrap();
    let edited = T::apply_edit(&original, &patch_for(T::KIND, &baseline, 0, "changed")).unwrap();
    let mut returned = serde_json::to_value(edited).unwrap();
    returned["title"] = json!("Worker metadata must be ignored");
    let returned = serde_json::from_value::<T>(returned).unwrap();
    let merged = merge_worker_results(original, vec![(0, returned)]).unwrap();
    let merged = serde_json::to_value(merged).unwrap();
    let (_, plural) = unit_names(T::KIND);
    assert_eq!(merged[plural][0]["notes"], json!("changed"));
    assert_eq!(
        &merged[plural].as_array().unwrap()[1..],
        &baseline[plural].as_array().unwrap()[1..]
    );
    for (key, value) in baseline.as_object().unwrap() {
        if key != plural {
            assert_eq!(&merged[key], value);
        }
    }
}

#[test]
fn all_kinds_apply_fixture_patches_and_merge_only_the_assigned_units() {
    assert_fixture_edit(sample_design());
    assert_fixture_edit(sample_deck());
    assert_fixture_edit(sample_document());
    assert_fixture_edit(sample_social());
    assert_fixture_edit(sample_print());
    assert_fixture_edit(sample_mailing());
    assert_fixture_edit(sample_campaign());
    assert_fixture_edit(sample_artwork());
}

#[test]
fn merging_rejects_duplicate_and_out_of_range_worker_results() {
    let original = sample_design();
    assert!(
        merge_worker_results(
            original.clone(),
            vec![(0, original.clone()), (0, original.clone())]
        )
        .is_err()
    );
    let mut shorter = original.clone();
    shorter.screens.pop();
    assert!(
        merge_worker_results(
            shorter.clone(),
            vec![(original.screens.len() - 1, original.clone())]
        )
        .is_err()
    );
    assert!(
        merge_worker_results(
            original.clone(),
            vec![(original.screens.len() - 1, shorter)]
        )
        .is_err()
    );
}

#[tokio::test]
async fn regeneration_workers_do_not_receive_the_original_markup() {
    let original = sample_deck();
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(
        ArtifactKind::Deck,
        &serde_json::to_value(&original).unwrap(),
        "regenerated",
    );
    fake.push_text(r#"{"slides":[]}"#.to_owned());
    let engine = test_engine(fake.configuration());
    succeeded(request_edit_for(&engine, &original, &[0, 2], true).await);
    for body in fake.requests() {
        if !request_text(&body).contains("Assigned unit index (zero-based):") {
            continue;
        }
        let prompt = body["messages"][0]["content"].as_str().unwrap();
        let (_, artifact) = prompt.split_once("Here is the edit baseline:\n").unwrap();
        let (artifact, _) = artifact
            .split_once("\nApply the requested change.")
            .unwrap();
        let focused: Value = serde_json::from_str(artifact).unwrap();
        assert_eq!(focused["slides"][0]["slide"]["html"], json!(""));
        assert!(focused["slides"][0]["slide"].get("css").is_none());
        assert!(prompt.contains("Write the assigned unit anew"));
    }
}
