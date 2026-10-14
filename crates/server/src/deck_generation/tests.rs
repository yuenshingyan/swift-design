#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    apply_deck_continuation, continuation_slides, deck_edit_prompt, deck_system_prompt,
    focused_deck_json, parse_deck, partial_deck, placeholder_slide, scenario_note, shown_deck,
    slide_count_note,
};
use crate::decks::DeckStore;
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_DECK, low_effort_options, sample_deck};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_deck_edit_shows_only_the_named_slides_and_their_findings() {
    let deck = sample_deck();
    let focused = focused_deck_json(&deck, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["slide_count"], deck.slides.len());
    assert_eq!(value["slides"].as_array().unwrap().len(), 1);
    assert_eq!(value["slides"][0]["index"], 1);
    let request = SessionRequest {
        request: "A talk.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Deck,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["slides[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[slide 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only slide 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = deck_edit_prompt(&request, &input);
    assert!(prompt.contains("Only slide 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- slides[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [slide 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
}

#[test]
fn the_slide_count_holds_the_deck_to_the_length_the_user_asked_for() {
    assert_eq!(scenario_note(None), "");
    assert_eq!(scenario_note(Some(" ")), "");
    assert!(scenario_note(Some("Training")).contains("scenario: Training"));
    assert_eq!(slide_count_note(None, None), "");
    assert_eq!(slide_count_note(None, Some(3)), "");
    assert_eq!(
        slide_count_note(Some(12), None),
        "The user asked for 12 slides. Write exactly 12 slides.\n"
    );
    // A preview writes three slides, so the length goes to the outline.
    assert_eq!(
        slide_count_note(Some(12), Some(3)),
        "The user asked for 12 slides. Put exactly 12 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    decks: DeckStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        decks: DeckStore::new(directory.path().join("decks")),
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
    .with_decks(stores.decks.clone())
}

/// A fresh one-candidate, low-effort deck session, still in intake.
async fn deck_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("talk", "Talk", "A talk.")
                .with_kind(ArtifactKind::Deck)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// A deck session past its setup card: the app's own questions were
/// asked, so the next planner turn is free to write.
async fn set_up_deck_session(sessions: &SessionStore) {
    deck_session(sessions).await;
    sessions
        .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn deck_system_prompt_carries_deck_rules_the_schema_and_the_example() {
    let prompt = deck_system_prompt();
    assert!(prompt.contains("slide decks"));
    assert!(prompt.contains("1920 by 1080 px"));
    assert!(prompt.contains("\"slides\""));
    assert!(prompt.contains("Swift Design Deck Overview"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
}

#[test]
fn partial_deck_returns_complete_slides_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#101418","text":"#f5f5f5","accent":"#4f8cff","muted":"#8a94a6"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"slides":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let deck = partial_deck(text).unwrap();
    assert_eq!(deck.slides.len(), 1);
    assert!(partial_deck("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_slides_reject_a_short_reply() {
    let mut preview = sample_deck();
    preview.outline = vec![
        "A".to_owned(),
        "B".to_owned(),
        "C".to_owned(),
        "D".to_owned(),
    ];
    assert!(apply_deck_continuation(&preview, "{\"slides\":[]}").is_err());
    let patch = r#"{"slides":[{"index":3,"insert":true,"slide":{"html":"<h2>D</h2>"}}]}"#;
    let continued = apply_deck_continuation(&preview, patch).unwrap();
    assert_eq!(continued.slides.len(), 4);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_slides(3, patch).unwrap().len(), 1);
    assert!(continuation_slides(3, "{\"screens\":[]}").is_err());
    assert!(parse_deck("no json").is_err());
}

#[test]
fn shown_decks_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_deck();
    preview.outline = (1..=7).map(|number| format!("Slide {number}")).collect();
    let chunks = [
        ContinueChunk { first: 3, count: 2 },
        ContinueChunk { first: 5, count: 2 },
    ];
    let board = vec![
        Vec::new(),
        vec![placeholder_slide("x"), placeholder_slide("y")],
    ];
    let shown = shown_deck(&preview, &chunks, &board);
    assert_eq!(shown.slides.len(), 7);
    assert!(shown.slides[3].html.contains("Slide 4"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_deck_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    deck_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("talk", silent_log())
        .await
        .unwrap();
    // The deck's own questions live on this card: the scenario, the
    // length, the density, the evidence, the candidates, and the
    // variety. Without the card the deck flow asked nothing at all.
    assert!(matches!(
        outcome,
        GenerationOutcome::NeedsClarification { question_set: 1 }
    ));
    let set = stores
        .sessions
        .read_question_set("talk", 1)
        .await
        .unwrap()
        .unwrap();
    assert!(set.questions.is_empty());
    assert!(set.can_proceed_with_assumptions);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_valid_deck_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DECK);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_deck_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .decks
            .load("talk-candidate-1")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        stores
            .designs
            .load("talk-candidate-1")
            .await
            .unwrap()
            .is_none()
    );
    let planner = server.requests()[0].to_string();
    assert!(planner.contains("You plan slide decks"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("slide decks"));
    assert!(request.contains("Build a deck"));
    let runs = stores.sessions.runs("talk").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_deck_open_patches_that_deck() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"slides":[{"index":0,"slide":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:96px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .decks
        .save("talk-candidate-1", &sample_deck())
        .await
        .unwrap();
    deck_session(&stores.sessions).await;
    stores
        .sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    stores
        .sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    stores
        .sessions
        .append_message(
            "talk",
            ChatMessage::user("[slide 1] Tighten the title.", Some("talk-candidate-1")),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Edited { reports, .. } = outcome else {
        panic!("expected a saved edit report");
    };
    assert!(reports[0].contains("Modified slide 1"));
    let messages = stores.sessions.messages("talk").await.unwrap();
    assert!(
        messages
            .last()
            .unwrap()
            .content
            .contains("Modified slide 1")
    );
    let edited = stores
        .decks
        .load("talk-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.slides[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("talk").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing deck session with `count` saved candidates.
async fn reviewing_deck_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .decks
            .save(&format!("talk-candidate-{number}"), &sample_deck())
            .await
            .unwrap();
    }
    set_up_deck_session(&stores.sessions).await;
    stores
        .sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    stores
        .sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_regenerated_slide_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its slide itself.
    server.push_text(
            r#"{"slides":[{"index":0,"slide":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:96px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut deck = sample_deck();
    deck.slides[0].html = "<h1>Old title markup</h1>".to_owned();
    stores.decks.save("talk-candidate-1", &deck).await.unwrap();
    reviewing_deck_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "talk",
            ChatMessage::regenerate_request("[slide 1] Write this slide anew.", "talk-candidate-1"),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Edited { .. }));
    let text = server.requests()[0].to_string();
    assert!(text.contains("Write slide 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .decks
        .load("talk-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.slides[0].html.contains("Fresh"));
    assert_eq!(edited.slides.len(), deck.slides.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_decks_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_DECK);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"slides":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_deck_session_with(&stores, 2).await;
    let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-2".to_owned()];
    stores
        .sessions
        .append_message(
            "talk",
            ChatMessage::user(
                "[candidate 1] [candidate 2] Opening from 1, close from 2.",
                None,
            )
            .with_pinned(pinned),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Wrote { design_ids } = outcome else {
        panic!("expected a write");
    };
    assert_eq!(design_ids, vec!["talk-candidate-3".to_owned()]);
    assert!(
        stores
            .decks
            .load("talk-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one deck"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_deck_session_without_a_deck_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DECK);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_deck_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("talk", silent_log()).await.unwrap_err();
    assert!(error.contains("no deck store"));
}
