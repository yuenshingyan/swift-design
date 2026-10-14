#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    apply_mailing_continuation, continuation_emails, email_count_note, focused_mailing_json,
    format_note, mailing_edit_prompt, mailing_system_prompt, parse_mailing, partial_mailing,
    placeholder_email, shown_mailing,
};
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::mailings::MailingStore;
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_MAILING, low_effort_options, sample_mailing};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_mailing_edit_shows_only_the_named_emails_and_their_findings() {
    let mailing = sample_mailing();
    let focused = focused_mailing_json(&mailing, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["email_count"], mailing.emails.len());
    assert_eq!(value["format"], "standard");
    assert_eq!(value["emails"].as_array().unwrap().len(), 1);
    assert_eq!(value["emails"][0]["index"], 1);
    let request = SessionRequest {
        request: "A launch email.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Mailing,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["emails[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[email 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only email 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = mailing_edit_prompt(&request, &input);
    assert!(prompt.contains("Only email 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- emails[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [email 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    assert!(!prompt.contains("slide"));
}

#[test]
fn the_email_count_and_the_format_hold_the_mailing_to_the_asked_shape() {
    assert_eq!(format_note(None), "");
    assert_eq!(format_note(Some("tall")), "");
    assert_eq!(
        format_note(Some("long")),
        "Lay the emails out on the long format: 600 by 1800 px. Set `format` to `long`.\n"
    );
    assert_eq!(
        format_note(Some("short")),
        "Lay the emails out on the short format: 600 by 800 px. Set `format` to `short`.\n"
    );
    assert_eq!(email_count_note(None, None), "");
    assert_eq!(email_count_note(None, Some(1)), "");
    assert_eq!(
        email_count_note(Some(2), None),
        "The user asked for 2 emails. Write exactly 2 emails.\n"
    );
    // A preview writes one email, so the length goes to the outline.
    assert_eq!(
        email_count_note(Some(2), Some(1)),
        "The user asked for 2 emails. Put exactly 2 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    mailings: MailingStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        mailings: MailingStore::new(directory.path().join("mailings")),
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
    .with_mailings(stores.mailings.clone())
}

/// A fresh one-candidate, low-effort mailing session, still in
/// intake.
async fn mailing_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("report", "Launch", "A launch email.")
                .with_kind(ArtifactKind::Mailing)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// A mailing session past its setup card: the app's own questions
/// were asked, so the next planner turn is free to write.
async fn set_up_mailing_session(sessions: &SessionStore) {
    mailing_session(sessions).await;
    sessions
        .apply("report", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn mailing_system_prompt_carries_mailing_rules_the_schema_and_the_example() {
    let prompt = mailing_system_prompt();
    assert!(prompt.contains("email sequences read in an inbox"));
    assert!(prompt.contains("600 by 1200 px for standard"));
    assert!(prompt.contains("\"emails\""));
    assert!(prompt.contains("\"format\""));
    assert!(prompt.contains("Swift Design launch email"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
    assert!(!prompt.contains("\"slides\""));
}

#[test]
fn partial_mailing_returns_complete_emails_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"format":"long","emails":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let mailing = partial_mailing(text).unwrap();
    assert_eq!(mailing.emails.len(), 1);
    assert_eq!(mailing.format, design_model::EmailFormat::Long);
    assert!(partial_mailing("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_emails_reject_a_short_reply() {
    let mut preview = sample_mailing();
    preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
    assert!(apply_mailing_continuation(&preview, "{\"emails\":[]}").is_err());
    let patch = r#"{"emails":[{"index":2,"insert":true,"email":{"html":"<h2>C</h2>"}}]}"#;
    let continued = apply_mailing_continuation(&preview, patch).unwrap();
    assert_eq!(continued.emails.len(), 3);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_emails(2, patch).unwrap().len(), 1);
    assert!(continuation_emails(2, "{\"slides\":[]}").is_err());
    assert!(parse_mailing("no json").is_err());
}

#[test]
fn shown_mailings_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_mailing();
    preview.outline = (1..=4).map(|number| format!("Email {number}")).collect();
    let chunks = [
        ContinueChunk { first: 2, count: 1 },
        ContinueChunk { first: 3, count: 1 },
    ];
    let board = vec![Vec::new(), vec![placeholder_email("x")]];
    let shown = shown_mailing(&preview, &chunks, &board);
    assert_eq!(shown.emails.len(), 4);
    assert!(shown.emails[2].html.contains("Email 3"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_mailing_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    mailing_session(&stores.sessions).await;
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
async fn a_valid_mailing_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_MAILING);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_mailing_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .mailings
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
    assert!(planner.contains("You plan emails"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("mailings as JSON mailings"));
    assert!(request.contains("Build a mailing"));
    let runs = stores.sessions.runs("report").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_mailing_open_patches_that_mailing() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"emails":[{"index":0,"email":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .mailings
        .save("report-candidate-1", &sample_mailing())
        .await
        .unwrap();
    mailing_session(&stores.sessions).await;
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
            ChatMessage::user("[email 1] Tighten the title.", Some("report-candidate-1")),
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
    assert!(reports[0].contains("Modified email 1"));
    let messages = stores.sessions.messages("report").await.unwrap();
    assert!(
        messages
            .last()
            .unwrap()
            .content
            .contains("Modified email 1")
    );
    let edited = stores
        .mailings
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.emails[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("report").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing mailing session with `count` saved candidates.
async fn reviewing_mailing_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .mailings
            .save(&format!("report-candidate-{number}"), &sample_mailing())
            .await
            .unwrap();
    }
    set_up_mailing_session(&stores.sessions).await;
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
async fn a_regenerated_email_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its email itself.
    server.push_text(
            r#"{"emails":[{"index":0,"email":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut mailing = sample_mailing();
    mailing.emails[0].html = "<h1>Old title markup</h1>".to_owned();
    stores
        .mailings
        .save("report-candidate-1", &mailing)
        .await
        .unwrap();
    reviewing_mailing_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::regenerate_request(
                "[email 1] Write this email anew.",
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
    assert!(text.contains("Write email 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .mailings
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.emails[0].html.contains("Fresh"));
    assert_eq!(edited.emails.len(), mailing.emails.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_mailings_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_MAILING);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"emails":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_mailing_session_with(&stores, 2).await;
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
            .mailings
            .load("report-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one mailing"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_mailing_session_without_a_mailing_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_MAILING);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_mailing_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("report", silent_log()).await.unwrap_err();
    assert!(error.contains("no mailing store"));
}
