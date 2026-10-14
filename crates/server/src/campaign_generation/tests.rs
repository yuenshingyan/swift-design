#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{ArtifactKind, WorkflowState};

use super::{
    ad_count_note, apply_campaign_continuation, campaign_edit_prompt, campaign_system_prompt,
    continuation_ads, focused_campaign_json, parse_campaign, partial_campaign, placeholder_ad,
    shown_campaign, size_note,
};
use crate::campaigns::CampaignStore;
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::events::ChangeNotifier;
use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
use crate::model_client::LogSink;
use crate::request::SessionRequest;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::test_support::{FakeModelServer, SAMPLE_CAMPAIGN, low_effort_options, sample_campaign};

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

#[test]
fn a_focused_campaign_edit_shows_only_the_named_ads_and_their_findings() {
    let campaign = sample_campaign();
    let focused = focused_campaign_json(&campaign, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["ad_count"], campaign.ads.len());
    assert_eq!(value["size"], "medium_rectangle");
    assert_eq!(value["ads"].as_array().unwrap().len(), 1);
    assert_eq!(value["ads"][0]["index"], 1);
    let request = SessionRequest {
        request: "A launch ad.".to_owned(),
        conversation: String::new(),
        kind: ArtifactKind::Campaign,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["ads[1] p (0/2): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[ad 2, node 0/2 <p>: x] Fix the overflow.",
        artifact_json: &focused,
        note: "Only ad 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = campaign_edit_prompt(&request, &input);
    assert!(prompt.contains("Only ad 2 is shown."));
    assert!(prompt.contains("Chrome measured these layout problems"));
    assert!(prompt.contains("- ads[1] p (0/2): overflow: shorten"));
    assert!(prompt.contains("Apply this change: [ad 2, node 0/2 <p>: x] Fix the overflow."));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    assert!(!prompt.contains("slide"));
}

#[test]
fn the_ad_count_and_the_size_hold_the_campaign_to_the_asked_shape() {
    assert_eq!(size_note(None), "");
    assert_eq!(size_note(Some("tall")), "");
    assert_eq!(
        size_note(Some("leaderboard")),
        "Lay the ads out on the leaderboard size: 728 by 90 px. Set `size` to `leaderboard`.\n"
    );
    assert_eq!(
        size_note(Some("skyscraper")),
        "Lay the ads out on the skyscraper size: 160 by 600 px. Set `size` to `skyscraper`.\n"
    );
    assert_eq!(ad_count_note(None, None), "");
    assert_eq!(ad_count_note(None, Some(1)), "");
    assert_eq!(
        ad_count_note(Some(2), None),
        "The user asked for 2 ads. Write exactly 2 ads.\n"
    );
    // A preview writes one ad, so the length goes to the outline.
    assert_eq!(
        ad_count_note(Some(2), Some(1)),
        "The user asked for 2 ads. Put exactly 2 titles in `outline`.\n"
    );
}

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

struct Stores {
    designs: DesignStore,
    campaigns: CampaignStore,
    sessions: SessionStore,
}

fn stores(directory: &tempfile::TempDir) -> Stores {
    Stores {
        designs: DesignStore::new(directory.path().join("designs")),
        campaigns: CampaignStore::new(directory.path().join("campaigns")),
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
    .with_campaigns(stores.campaigns.clone())
}

/// A fresh one-candidate, low-effort campaign session, still in
/// intake.
async fn campaign_session(sessions: &SessionStore) {
    sessions
        .create(
            NewSession::demo("report", "Launch", "A launch ad.")
                .with_kind(ArtifactKind::Campaign)
                .with_options(low_effort_options()),
        )
        .await
        .unwrap();
}

/// A campaign session past its setup card: the app's own questions
/// were asked, so the next planner turn is free to write.
async fn set_up_campaign_session(sessions: &SessionStore) {
    campaign_session(sessions).await;
    sessions
        .apply("report", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn campaign_system_prompt_carries_campaign_rules_the_schema_and_the_example() {
    let prompt = campaign_system_prompt();
    assert!(prompt.contains("display units read on a page"));
    assert!(prompt.contains("300 by 250 px for medium_rectangle"));
    assert!(prompt.contains("\"ads\""));
    assert!(prompt.contains("\"size\""));
    assert!(prompt.contains("Swift Design launch ads"));
    assert!(prompt.contains("needs_clarification"));
    assert!(!prompt.contains("\"viewport\""));
    assert!(!prompt.contains("\"slides\""));
}

#[test]
fn partial_campaign_returns_complete_ads_only() {
    let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"size":"leaderboard","ads":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
    let campaign = partial_campaign(text).unwrap();
    assert_eq!(campaign.ads.len(), 1);
    assert_eq!(campaign.size, design_model::AdSize::Leaderboard);
    assert!(partial_campaign("{\"title\":\"T\"").is_none());
}

#[test]
fn continuation_ads_reject_a_short_reply() {
    let mut preview = sample_campaign();
    preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
    assert!(apply_campaign_continuation(&preview, "{\"ads\":[]}").is_err());
    let patch = r#"{"ads":[{"index":2,"insert":true,"ad":{"html":"<h2>C</h2>"}}]}"#;
    let continued = apply_campaign_continuation(&preview, patch).unwrap();
    assert_eq!(continued.ads.len(), 3);
    assert!(continued.outline.is_empty());
    assert_eq!(continuation_ads(2, patch).unwrap().len(), 1);
    assert!(continuation_ads(2, "{\"slides\":[]}").is_err());
    assert!(parse_campaign("no json").is_err());
}

#[test]
fn shown_campaigns_pad_earlier_chunks_with_placeholders() {
    let mut preview = sample_campaign();
    preview.outline = (1..=4).map(|number| format!("Ad {number}")).collect();
    let chunks = [
        ContinueChunk { first: 2, count: 1 },
        ContinueChunk { first: 3, count: 1 },
    ];
    let board = vec![Vec::new(), vec![placeholder_ad("x")]];
    let shown = shown_campaign(&preview, &chunks, &board);
    assert_eq!(shown.ads.len(), 4);
    assert!(shown.ads[2].html.contains("Ad 3"));
    assert!(shown.validate().is_empty());
}

#[tokio::test]
async fn a_campaign_run_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    campaign_session(&stores.sessions).await;
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
async fn a_valid_campaign_reply_is_saved_as_a_candidate() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_CAMPAIGN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_campaign_session(&stores.sessions).await;
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(
        stores
            .campaigns
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
    assert!(planner.contains("You plan ads"));
    let request = server.requests()[1].to_string();
    assert!(request.contains("campaigns as JSON campaigns"));
    assert!(request.contains("Build a campaign"));
    let runs = stores.sessions.runs("report").await.unwrap();
    assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
}

#[tokio::test]
async fn a_chat_request_with_a_campaign_open_patches_that_campaign() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
    server.push_text(
            r#"{"ads":[{"index":0,"ad":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    stores
        .campaigns
        .save("report-candidate-1", &sample_campaign())
        .await
        .unwrap();
    campaign_session(&stores.sessions).await;
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
            ChatMessage::user("[ad 1] Tighten the title.", Some("report-candidate-1")),
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
    assert!(reports[0].contains("Modified ad 1"));
    let messages = stores.sessions.messages("report").await.unwrap();
    assert!(messages.last().unwrap().content.contains("Modified ad 1"));
    let edited = stores
        .campaigns
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.ads[0].html.contains("Tighter"));
    assert_eq!(
        stores.sessions.read("report").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

/// A reviewing campaign session with `count` saved candidates.
async fn reviewing_campaign_session_with(stores: &Stores, count: usize) {
    for number in 1..=count {
        stores
            .campaigns
            .save(&format!("report-candidate-{number}"), &sample_campaign())
            .await
            .unwrap();
    }
    set_up_campaign_session(&stores.sessions).await;
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
async fn a_regenerated_ad_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its ad itself.
    server.push_text(
            r#"{"ads":[{"index":0,"ad":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    let mut campaign = sample_campaign();
    campaign.ads[0].html = "<h1>Old title markup</h1>".to_owned();
    stores
        .campaigns
        .save("report-candidate-1", &campaign)
        .await
        .unwrap();
    reviewing_campaign_session_with(&stores, 0).await;
    stores
        .sessions
        .append_message(
            "report",
            ChatMessage::regenerate_request("[ad 1] Write this ad anew.", "report-candidate-1"),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &stores)
        .run("report", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Edited { .. }));
    let text = server.requests()[0].to_string();
    assert!(text.contains("Write ad 1 of"));
    assert!(!text.contains("Old title markup"));
    let edited = stores
        .campaigns
        .load("report-candidate-1")
        .await
        .unwrap()
        .unwrap();
    assert!(edited.ads[0].html.contains("Fresh"));
    assert_eq!(edited.ads.len(), campaign.ads.len());
}

#[tokio::test]
async fn a_merge_of_two_pinned_campaigns_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_CAMPAIGN);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"ads":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    reviewing_campaign_session_with(&stores, 2).await;
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
            .campaigns
            .load("report-candidate-3")
            .await
            .unwrap()
            .is_some()
    );
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one campaign"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_campaign_session_without_a_campaign_store_fails_plainly() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_CAMPAIGN);
    let directory = tempfile::tempdir().unwrap();
    let stores = stores(&directory);
    set_up_campaign_session(&stores.sessions).await;
    let engine = GenerationEngine::new(
        server.configuration(),
        stores.designs.clone(),
        stores.sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    let error = engine.run("report", silent_log()).await.unwrap_err();
    assert!(error.contains("no campaign store"));
}
