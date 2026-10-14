#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    apply_artwork_continuation, artwork_edit_prompt, artwork_system_prompt, continuation_covers,
    cover_count_note, focused_artwork_json, parse_artwork, partial_artwork, placeholder_cover,
    shown_artwork, size_note,
};
use crate::artworks::ArtworkStore;
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_ARTWORK, low_effort_options, sample_artwork};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_artwork_edit_shows_only_the_named_covers_and_their_findings() {
    let artwork = sample_artwork();
    let focused = focused_artwork_json(&artwork, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["cover_count"], artwork.covers.len());
    assert_eq!(value["size"], "thumbnail");
    assert_eq!(value["covers"].as_array().unwrap().len(), 1);
    assert_eq!(value["covers"][0]["index"], 1);
    let request = SessionRequest {
        request: "A launch cover.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Artwork,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["covers[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[cover 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only cover 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = artwork_edit_prompt(&request, &input);
    assert!(prompt.contains("Only cover 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- covers[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [cover 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    assert!(!prompt.contains("slide"));
}

#[test]
fn the_cover_count_and_the_size_hold_the_artwork_to_the_asked_shape() {
    assert_eq!(size_note(None), "");
    assert_eq!(size_note(Some("tall")), "");
    assert_eq!(
        size_note(Some("banner")),
        "Lay the covers out on the banner size: 2560 by 1440 px. Set `size` to `banner`.\n"
    );
    assert_eq!(
        size_note(Some("album")),
        "Lay the covers out on the album size: 3000 by 3000 px. Set `size` to `album`.\n"
    );
    assert_eq!(cover_count_note(None, None), "");
    assert_eq!(cover_count_note(None, Some(1)), "");
    assert_eq!(
        cover_count_note(Some(2), None),
        "The user asked for 2 covers. Write exactly 2 covers.\n"
    );
    // A preview writes one cover, so the length goes to the outline.
    assert_eq!(
        cover_count_note(Some(2), Some(1)),
        "The user asked for 2 covers. Put exactly 2 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    artworks: ArtworkStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        artworks: ArtworkStore::new(directory.path().join("artworks")),
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
    .with_artworks(stores.artworks.clone())
}

/// A fresh one-candidate, low-effort artwork session, still in
/// intake.
async fn artwork_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("report", "Launch", "A launch cover.")
                .with_kind(ArtifactKind::Artwork)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// An artwork session past its setup card: the app's own questions
/// were asked, so the next planner turn is free to write.
async fn set_up_artwork_session(sessions: &SessionStore) {
    artwork_session(sessions).await;
    sessions
        .apply("report", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn artwork_system_prompt_carries_artwork_rules_the_schema_and_the_example() {
    let prompt = artwork_system_prompt();
    assert!(prompt.contains("judged at a glance"));
    assert!(prompt.contains("1280 by 720 px for thumbnail"));
    assert!(prompt.contains("\"covers\""));
    assert!(prompt.contains("\"size\""));
    assert!(prompt.contains("Swift Design launch thumbnails"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
    assert!(!prompt.contains("\"slides\""));
}

#[test]
fn partial_artwork_returns_complete_covers_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"size":"banner","covers":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let artwork = partial_artwork(text).unwrap();
    assert_eq!(artwork.covers.len(), 1);
    assert_eq!(artwork.size, design_model::CoverSize::Banner);
    assert!(partial_artwork("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_covers_reject_a_short_reply() {
    let mut preview = sample_artwork();
    preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
    assert!(apply_artwork_continuation(&preview, "{\"covers\":[]}").is_err());
    let patch = r#"{"covers":[{"index":2,"insert":true,"cover":{"html":"<h2>C</h2>"}}]}"#;
    let continued = apply_artwork_continuation(&preview, patch).unwrap();
    assert_eq!(continued.covers.len(), 3);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_covers(2, patch).unwrap().len(), 1);
    assert!(continuation_covers(2, "{\"slides\":[]}").is_err());
    assert!(parse_artwork("no json").is_err());
}

#[test]
fn shown_artworks_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_artwork();
    preview.outline = (1..=4).map(|number| format!("Cover {number}")).collect();
    let chunks = [
        ContinueChunk { first: 2, count: 1 },
        ContinueChunk { first: 3, count: 1 },
    ];
    let board = vec![Vec::new(), vec![placeholder_cover("x")]];
    let shown = shown_artwork(&preview, &chunks, &board);
    assert_eq!(shown.covers.len(), 4);
    assert!(shown.covers[2].html.contains("Cover 3"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_artwork_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    artwork_session(&stores.sessions).await;
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
async fn a_valid_artwork_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_ARTWORK);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_artwork_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .artworks
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
    assert!(planner.contains("You plan cover art"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("cover art as JSON artworks"));
    assert!(request.contains("Build an artwork"));
    let runs = stores.sessions.runs("report").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_artwork_open_patches_that_artwork() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"covers":[{"index":0,"cover":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .artworks
        .save("report-candidate-1", &sample_artwork())
        .await
        .unwrap();
    artwork_session(&stores.sessions).await;
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
            ChatMessage::user("[cover 1] Tighten the title.", Some("report-candidate-1")),
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
    assert!(reports[0].contains("Modified cover 1"));
    let messages = stores.sessions.messages("report").await.unwrap();
    assert!(
        messages
            .last()
            .unwrap()
            .content
            .contains("Modified cover 1")
    );
    let edited = stores
        .artworks
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.covers[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("report").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing artwork session with `count` saved candidates.
async fn reviewing_artwork_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .artworks
            .save(&format!("report-candidate-{number}"), &sample_artwork())
            .await
            .unwrap();
    }
    set_up_artwork_session(&stores.sessions).await;
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
async fn a_regenerated_cover_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its cover itself.
    server.push_text(
            r#"{"covers":[{"index":0,"cover":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut artwork = sample_artwork();
    artwork.covers[0].html = "<h1>Old title markup</h1>".to_owned();
    stores
        .artworks
        .save("report-candidate-1", &artwork)
        .await
        .unwrap();
    reviewing_artwork_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::regenerate_request(
                "[cover 1] Write this cover anew.",
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
    assert!(text.contains("Write cover 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .artworks
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.covers[0].html.contains("Fresh"));
    assert_eq!(edited.covers.len(), artwork.covers.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_artworks_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_ARTWORK);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"covers":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_artwork_session_with(&stores, 2).await;
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
            .artworks
            .load("report-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one artwork"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_artwork_session_without_a_artwork_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_ARTWORK);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_artwork_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("report", silent_log()).await.unwrap_err();
    assert!(error.contains("no artwork store"));
}
