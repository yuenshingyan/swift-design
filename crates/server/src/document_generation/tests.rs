#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    apply_document_continuation, continuation_pages, document_edit_prompt, document_system_prompt,
    focused_document_json, page_count_note, paper_note, parse_document, partial_document,
    placeholder_page, shown_document,
};
use crate::designs::DesignStore;
use crate::documents::DocumentStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_DOCUMENT, low_effort_options, sample_document};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_document_edit_shows_only_the_named_pages_and_their_findings() {
    let document = sample_document();
    let focused = focused_document_json(&document, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["page_count"], document.pages.len());
    assert_eq!(value["paper"], "a4");
    assert_eq!(value["pages"].as_array().unwrap().len(), 1);
    assert_eq!(value["pages"][0]["index"], 1);
    let request = SessionRequest {
        request: "A report.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Document,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["pages[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[page 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only page 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = document_edit_prompt(&request, &input);
    assert!(prompt.contains("Only page 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- pages[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [page 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    assert!(!prompt.contains("slide"));
}

#[test]
fn the_page_count_and_the_paper_hold_the_document_to_the_asked_shape() {
    assert_eq!(paper_note(None), "");
    assert_eq!(paper_note(Some("tabloid")), "");
    assert_eq!(
        paper_note(Some("letter")),
        "Lay the pages out on Letter paper: 816 by 1056 px. Set `paper` to `letter`.\n"
    );
    assert_eq!(page_count_note(None, None), "");
    assert_eq!(page_count_note(None, Some(3)), "");
    assert_eq!(
        page_count_note(Some(12), None),
        "The user asked for 12 pages. Write exactly 12 pages.\n"
    );
    // A preview writes three pages, so the length goes to the outline.
    assert_eq!(
        page_count_note(Some(12), Some(3)),
        "The user asked for 12 pages. Put exactly 12 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    documents: DocumentStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        documents: DocumentStore::new(directory.path().join("documents")),
        sessions: SessionStore::new(directory.path().join("sessions")),
    }
}

fn engine(server: &FakeModelServer, stores: &Stores) -> GenerationEngine {
    GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    )
    .with_documents(stores.documents.clone())
}

/// A fresh one-candidate, low-effort document session, still in
/// intake.
async fn document_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("report", "Report", "A report.")
                .with_kind(ArtifactKind::Document)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// A document session past its setup card: the app's own questions
/// were asked, so the next planner turn is free to write.
async fn set_up_document_session(sessions: &SessionStore) {
    document_session(sessions).await;
    sessions
        .apply("report", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn document_system_prompt_carries_document_rules_the_schema_and_the_example() {
    let prompt = document_system_prompt();
    assert!(prompt.contains("paged documents"));
    assert!(prompt.contains("794 by 1123 px"));
    assert!(prompt.contains("\"pages\""));
    assert!(prompt.contains("\"paper\""));
    assert!(prompt.contains("Swift Design Quarterly Report"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
    assert!(!prompt.contains("\"slides\""));
}

#[test]
fn partial_document_returns_complete_pages_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"paper":"letter","pages":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let document = partial_document(text).unwrap();
    assert_eq!(document.pages.len(), 1);
    assert_eq!(document.paper, design_model::Paper::Letter);
    assert!(partial_document("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_pages_reject_a_short_reply() {
    let mut preview = sample_document();
    preview.outline = vec![
        "A".to_owned(),
        "B".to_owned(),
        "C".to_owned(),
        "D".to_owned(),
    ];
    assert!(apply_document_continuation(&preview, "{\"pages\":[]}").is_err());
    let patch = r#"{"pages":[{"index":3,"insert":true,"page":{"html":"<h2>D</h2>"}}]}"#;
    let continued = apply_document_continuation(&preview, patch).unwrap();
    assert_eq!(continued.pages.len(), 4);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_pages(3, patch).unwrap().len(), 1);
    assert!(continuation_pages(3, "{\"slides\":[]}").is_err());
    assert!(parse_document("no json").is_err());
}

#[test]
fn shown_documents_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_document();
    preview.outline = (1..=7).map(|number| format!("Page {number}")).collect();
    let chunks = [
        ContinueChunk { first: 3, count: 2 },
        ContinueChunk { first: 5, count: 2 },
    ];
    let board = vec![
        Vec::new(),
        vec![placeholder_page("x"), placeholder_page("y")],
    ];
    let shown = shown_document(&preview, &chunks, &board);
    assert_eq!(shown.pages.len(), 7);
    assert!(shown.pages[3].html.contains("Page 4"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_document_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    document_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        GenerationOutcome::NeedsClarification { question_set: 1 }
    ));
    let set = stores
        .sessions
        .read_question_set("report", 1)
        .await
        .unwrap()
        .unwrap();
    assert!(set.questions.is_empty());
    assert!(set.can_proceed_with_assumptions);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_valid_document_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DOCUMENT);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_document_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .documents
            .load("report-candidate-1")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        stores
            .designs
            .load("report-candidate-1")
            .await
            .unwrap()
            .is_none()
    );
    let planner = server.requests()[0].to_string();
    assert!(planner.contains("You plan documents"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("paged documents"));
    assert!(request.contains("Build a document"));
    let runs = stores.sessions.runs("report").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_document_open_patches_that_document() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"pages":[{"index":0,"page":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .documents
        .save("report-candidate-1", &sample_document())
        .await
        .unwrap();
    document_session(&stores.sessions).await;
    stores
        .sessions
        .apply("report", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    stores
        .sessions
        .apply("report", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::user("[page 1] Tighten the title.", Some("report-candidate-1")),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Edited { reports, .. } = outcome else {
        panic!("expected a saved edit report");
    };
    assert!(reports[0].contains("Modified page 1"));
    let messages = stores.sessions.messages("report").await.unwrap();
    assert!(messages.last().unwrap().content.contains("Modified page 1"));
    let edited = stores
        .documents
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.pages[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("report").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing document session with `count` saved candidates.
async fn reviewing_document_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .documents
            .save(&format!("report-candidate-{number}"), &sample_document())
            .await
            .unwrap();
    }
    set_up_document_session(&stores.sessions).await;
    stores
        .sessions
        .apply("report", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    stores
        .sessions
        .apply("report", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_regenerated_page_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its page itself.
    server.push_text(
            r#"{"pages":[{"index":0,"page":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut document = sample_document();
    document.pages[0].html = "<h1>Old title markup</h1>".to_owned();
    stores
        .documents
        .save("report-candidate-1", &document)
        .await
        .unwrap();
    reviewing_document_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::regenerate_request("[page 1] Write this page anew.", "report-candidate-1"),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Edited { .. }));
    let text = server.requests()[0].to_string();
    assert!(text.contains("Write page 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .documents
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.pages[0].html.contains("Fresh"));
    assert_eq!(edited.pages.len(), document.pages.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_documents_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_DOCUMENT);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"pages":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_document_session_with(&stores, 2).await;
    let pinned = vec![
        "report-candidate-1".to_owned(),
        "report-candidate-2".to_owned(),
    ];
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::user(
                "[candidate 1] [candidate 2] Cover from 1, table from 2.",
                None,
            )
            .with_pinned(pinned),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Wrote { design_ids } = outcome else {
        panic!("expected a write");
    };
    assert_eq!(design_ids, vec!["report-candidate-3".to_owned()]);
    assert!(
        stores
            .documents
            .load("report-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one document"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_document_session_without_a_document_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DOCUMENT);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_document_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("report", silent_log()).await.unwrap_err();
    assert!(error.contains("no document store"));
}
