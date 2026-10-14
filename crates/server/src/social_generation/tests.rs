#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    apply_social_continuation, continuation_frames, focused_social_json, format_note,
    frame_count_note, parse_social, partial_social, placeholder_frame, shown_social,
    social_edit_prompt, social_system_prompt,
};
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::socials::SocialStore;
use crate::test_support::{FakeModelServer, SAMPLE_SOCIAL, low_effort_options, sample_social};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_social_edit_shows_only_the_named_frames_and_their_findings() {
    let social = sample_social();
    let focused = focused_social_json(&social, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["frame_count"], social.frames.len());
    assert_eq!(value["format"], "portrait");
    assert_eq!(value["frames"].as_array().unwrap().len(), 1);
    assert_eq!(value["frames"][0]["index"], 1);
    let request = SessionRequest {
        request: "A launch carousel.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Social,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["frames[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[frame 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only frame 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = social_edit_prompt(&request, &input);
    assert!(prompt.contains("Only frame 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- frames[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [frame 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    assert!(!prompt.contains("slide"));
}

#[test]
fn the_frame_count_and_the_format_hold_the_social_to_the_asked_shape() {
    assert_eq!(format_note(None), "");
    assert_eq!(format_note(Some("banner")), "");
    assert_eq!(
        format_note(Some("story")),
        "Lay the frames out on the story format: 1080 by 1920 px. Set `format` to `story`.\n"
    );
    assert_eq!(frame_count_note(None, None), "");
    assert_eq!(frame_count_note(None, Some(3)), "");
    assert_eq!(
        frame_count_note(Some(8), None),
        "The user asked for 8 frames. Write exactly 8 frames.\n"
    );
    // A preview writes three frames, so the length goes to the outline.
    assert_eq!(
        frame_count_note(Some(8), Some(3)),
        "The user asked for 8 frames. Put exactly 8 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    socials: SocialStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        socials: SocialStore::new(directory.path().join("socials")),
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
    .with_socials(stores.socials.clone())
}

/// A fresh one-candidate, low-effort social session, still in
/// intake.
async fn social_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("report", "Launch", "A launch carousel.")
                .with_kind(ArtifactKind::Social)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// A social session past its setup card: the app's own questions
/// were asked, so the next planner turn is free to write.
async fn set_up_social_session(sessions: &SessionStore) {
    social_session(sessions).await;
    sessions
        .apply("report", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn social_system_prompt_carries_social_rules_the_schema_and_the_example() {
    let prompt = social_system_prompt();
    assert!(prompt.contains("social posts and carousels"));
    assert!(prompt.contains("1080 by 1350 px for portrait"));
    assert!(prompt.contains("\"frames\""));
    assert!(prompt.contains("\"format\""));
    assert!(prompt.contains("Swift Design launch carousel"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
    assert!(!prompt.contains("\"slides\""));
}

#[test]
fn partial_social_returns_complete_frames_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"format":"story","frames":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let social = partial_social(text).unwrap();
    assert_eq!(social.frames.len(), 1);
    assert_eq!(social.format, design_model::Format::Story);
    assert!(partial_social("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_frames_reject_a_short_reply() {
    let mut preview = sample_social();
    preview.outline = vec![
        "A".to_owned(),
        "B".to_owned(),
        "C".to_owned(),
        "D".to_owned(),
    ];
    assert!(apply_social_continuation(&preview, "{\"frames\":[]}").is_err());
    let patch = r#"{"frames":[{"index":3,"insert":true,"frame":{"html":"<h2>D</h2>"}}]}"#;
    let continued = apply_social_continuation(&preview, patch).unwrap();
    assert_eq!(continued.frames.len(), 4);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_frames(3, patch).unwrap().len(), 1);
    assert!(continuation_frames(3, "{\"slides\":[]}").is_err());
    assert!(parse_social("no json").is_err());
}

#[test]
fn shown_socials_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_social();
    preview.outline = (1..=7).map(|number| format!("Frame {number}")).collect();
    let chunks = [
        ContinueChunk { first: 3, count: 2 },
        ContinueChunk { first: 5, count: 2 },
    ];
    let board = vec![
        Vec::new(),
        vec![placeholder_frame("x"), placeholder_frame("y")],
    ];
    let shown = shown_social(&preview, &chunks, &board);
    assert_eq!(shown.frames.len(), 7);
    assert!(shown.frames[3].html.contains("Frame 4"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_social_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    social_session(&stores.sessions).await;
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
async fn a_valid_social_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_SOCIAL);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_social_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .socials
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
    assert!(planner.contains("You plan social posts"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("social posts and carousels"));
    assert!(request.contains("Build a social post or carousel"));
    let runs = stores.sessions.runs("report").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_social_open_patches_that_social() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"frames":[{"index":0,"frame":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .socials
        .save("report-candidate-1", &sample_social())
        .await
        .unwrap();
    social_session(&stores.sessions).await;
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
            ChatMessage::user("[frame 1] Tighten the title.", Some("report-candidate-1")),
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
    assert!(reports[0].contains("Modified frame 1"));
    let messages = stores.sessions.messages("report").await.unwrap();
    assert!(
        messages
            .last()
            .unwrap()
            .content
            .contains("Modified frame 1")
    );
    let edited = stores
        .socials
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.frames[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("report").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing social session with `count` saved candidates.
async fn reviewing_social_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .socials
            .save(&format!("report-candidate-{number}"), &sample_social())
            .await
            .unwrap();
    }
    set_up_social_session(&stores.sessions).await;
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
async fn a_regenerated_frame_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its frame itself.
    server.push_text(
            r#"{"frames":[{"index":0,"frame":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut social = sample_social();
    social.frames[0].html = "<h1>Old title markup</h1>".to_owned();
    stores
        .socials
        .save("report-candidate-1", &social)
        .await
        .unwrap();
    reviewing_social_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::regenerate_request(
                "[frame 1] Write this frame anew.",
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
    assert!(text.contains("Write frame 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .socials
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.frames[0].html.contains("Fresh"));
    assert_eq!(edited.frames.len(), social.frames.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_socials_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_SOCIAL);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"frames":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_social_session_with(&stores, 2).await;
    let pinned = vec![
        "report-candidate-1".to_owned(),
        "report-candidate-2".to_owned(),
    ];
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::user(
                "[candidate 1] [candidate 2] Hook from 1, call to action from 2.",
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
            .socials
            .load("report-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one social"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_social_session_without_a_social_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_SOCIAL);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_social_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("report", silent_log()).await.unwrap_err();
    assert!(error.contains("no social store"));
}
