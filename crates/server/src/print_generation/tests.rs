#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    apply_print_continuation, continuation_sheets, focused_print_json, parse_print, partial_print,
    placeholder_sheet, print_edit_prompt, print_system_prompt, sheet_count_note, shown_print,
    size_note,
};
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::model_client::LogSink;
use crate::prints::PrintStore;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_PRINT, low_effort_options, sample_print};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_print_edit_shows_only_the_named_sheets_and_their_findings() {
    let print = sample_print();
    let focused = focused_print_json(&print, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["sheet_count"], print.sheets.len());
    assert_eq!(value["size"], "a4");
    assert_eq!(value["orientation"], "portrait");
    assert_eq!(value["sheets"].as_array().unwrap().len(), 1);
    assert_eq!(value["sheets"][0]["index"], 1);
    let request = SessionRequest {
        request: "A launch flyer.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Print,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["sheets[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[sheet 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only sheet 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = print_edit_prompt(&request, &input);
    assert!(prompt.contains("Only sheet 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- sheets[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [sheet 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    assert!(!prompt.contains("slide"));
}

#[test]
fn the_sheet_count_and_the_size_hold_the_print_to_the_asked_shape() {
    assert_eq!(size_note(None, None), "");
    assert_eq!(size_note(Some("a2"), None), "");
    assert_eq!(
        size_note(Some("a3"), Some("landscape")),
        "Lay the sheets out on the a3 size, landscape: 1587 by 1123 px. Set `size` to `a3` and `orientation` to `landscape`.\n"
    );
    assert_eq!(
        size_note(Some("letter"), None),
        "Lay the sheets out on the letter size. Set `size` to `letter`.\n"
    );
    assert_eq!(
        size_note(None, Some("landscape")),
        "Turn every sheet landscape. Set `orientation` to `landscape`.\n"
    );
    assert_eq!(sheet_count_note(None, None), "");
    assert_eq!(sheet_count_note(None, Some(1)), "");
    assert_eq!(
        sheet_count_note(Some(2), None),
        "The user asked for 2 sheets. Write exactly 2 sheets.\n"
    );
    // A preview writes one sheet, so the length goes to the outline.
    assert_eq!(
        sheet_count_note(Some(2), Some(1)),
        "The user asked for 2 sheets. Put exactly 2 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    prints: PrintStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        prints: PrintStore::new(directory.path().join("prints")),
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
    .with_prints(stores.prints.clone())
}

/// A fresh one-candidate, low-effort print session, still in
/// intake.
async fn print_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("report", "Launch", "A launch flyer.")
                .with_kind(ArtifactKind::Print)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// A print session past its setup card: the app's own questions
/// were asked, so the next planner turn is free to write.
async fn set_up_print_session(sessions: &SessionStore) {
    print_session(sessions).await;
    sessions
        .apply("report", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn print_system_prompt_carries_print_rules_the_schema_and_the_example() {
    let prompt = print_system_prompt();
    assert!(prompt.contains("print pieces"));
    assert!(prompt.contains("794 by 1123 px for A4"));
    assert!(prompt.contains("\"sheets\""));
    assert!(prompt.contains("\"size\""));
    assert!(prompt.contains("\"orientation\""));
    assert!(prompt.contains("Swift Design launch flyer"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
    assert!(!prompt.contains("\"slides\""));
}

#[test]
fn partial_print_returns_complete_sheets_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"size":"a3","orientation":"landscape","sheets":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let print = partial_print(text).unwrap();
    assert_eq!(print.sheets.len(), 1);
    assert_eq!(print.size, design_model::PrintSize::A3);
    assert_eq!(print.orientation, design_model::Orientation::Landscape);
    assert!(partial_print("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_sheets_reject_a_short_reply() {
    let mut preview = sample_print();
    preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
    assert!(apply_print_continuation(&preview, "{\"sheets\":[]}").is_err());
    let patch = r#"{"sheets":[{"index":2,"insert":true,"sheet":{"html":"<h2>C</h2>"}}]}"#;
    let continued = apply_print_continuation(&preview, patch).unwrap();
    assert_eq!(continued.sheets.len(), 3);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_sheets(2, patch).unwrap().len(), 1);
    assert!(continuation_sheets(2, "{\"slides\":[]}").is_err());
    assert!(parse_print("no json").is_err());
}

#[test]
fn shown_prints_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_print();
    preview.outline = (1..=4).map(|number| format!("Sheet {number}")).collect();
    let chunks = [
        ContinueChunk { first: 2, count: 1 },
        ContinueChunk { first: 3, count: 1 },
    ];
    let board = vec![Vec::new(), vec![placeholder_sheet("x")]];
    let shown = shown_print(&preview, &chunks, &board);
    assert_eq!(shown.sheets.len(), 4);
    assert!(shown.sheets[2].html.contains("Sheet 3"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_print_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    print_session(&stores.sessions).await;
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
async fn a_valid_print_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_PRINT);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_print_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .prints
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
    assert!(planner.contains("You plan print pieces"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("print pieces as JSON prints"));
    assert!(request.contains("Build a print piece"));
    let runs = stores.sessions.runs("report").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_print_open_patches_that_print() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"sheets":[{"index":0,"sheet":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .prints
        .save("report-candidate-1", &sample_print())
        .await
        .unwrap();
    print_session(&stores.sessions).await;
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
            ChatMessage::user("[sheet 1] Tighten the title.", Some("report-candidate-1")),
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
    assert!(reports[0].contains("Modified sheet 1"));
    let messages = stores.sessions.messages("report").await.unwrap();
    assert!(
        messages
            .last()
            .unwrap()
            .content
            .contains("Modified sheet 1")
    );
    let edited = stores
        .prints
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.sheets[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("report").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing print session with `count` saved candidates.
async fn reviewing_print_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .prints
            .save(&format!("report-candidate-{number}"), &sample_print())
            .await
            .unwrap();
    }
    set_up_print_session(&stores.sessions).await;
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
async fn a_regenerated_sheet_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its sheet itself.
    server.push_text(
            r#"{"sheets":[{"index":0,"sheet":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut print = sample_print();
    print.sheets[0].html = "<h1>Old title markup</h1>".to_owned();
    stores
        .prints
        .save("report-candidate-1", &print)
        .await
        .unwrap();
    reviewing_print_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::regenerate_request(
                "[sheet 1] Write this sheet anew.",
                "report-candidate-1",
            ),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Edited { .. }));
    let text = server.requests()[0].to_string();
    assert!(text.contains("Write sheet 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .prints
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.sheets[0].html.contains("Fresh"));
    assert_eq!(edited.sheets.len(), print.sheets.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_prints_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_PRINT);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"sheets":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_print_session_with(&stores, 2).await;
    let pinned = vec![
        "report-candidate-1".to_owned(),
        "report-candidate-2".to_owned(),
    ];
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::user(
                "[candidate 1] [candidate 2] Front from 1, back from 2.",
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
            .prints
            .load("report-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one print"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_print_session_without_a_print_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_PRINT);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_print_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("report", silent_log()).await.unwrap_err();
    assert!(error.contains("no print store"));
}
