#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use design_model::{AppWindow, WorkflowState};

use super::edit_targets;

use super::artifacts::{candidate_plans, edit_prompt, focused_design_json, system_prompt};
use super::attachments::{
    ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES, UploadAttachment, text_of, user_content_with_attachments,
};
use super::streaming::LiveSaver;
use super::{
    ATTACHMENT_TEXT_LIMIT_BYTES, Attachments, GenerationContext, GenerationEngine,
    GenerationOutcome, ProgressGroup, ProgressSink, answers_since_last_write,
    trailing_continue_ids,
};
use crate::designs::DesignStore;
use crate::edit_focus::EditInput;
use crate::edit_report::EDIT_REPORT_PREFIX;
use crate::events::ChangeNotifier;
use crate::model_client::LogSink;
use crate::office::DOCX;
use crate::request::SessionRequest;
use crate::sessions::{AnswerRecord, ChatMessage, NewSession, SessionStore};
use crate::test_support::{
    FakeModelServer, SAMPLE_DESIGN, clean_demo_dom_runner, low_effort_options,
};
use crate::uploads::UploadStore;

/// The planner reply that writes candidates.
const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

fn silent_log() -> LogSink {
    Arc::new(|_line: &str| {})
}

fn engine(
    server: &FakeModelServer,
    designs: &DesignStore,
    sessions: &SessionStore,
) -> GenerationEngine {
    GenerationEngine::new(
        server.configuration(),
        designs.clone(),
        sessions.clone(),
        None,
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    )
    .with_demo_dom_runner(clean_demo_dom_runner())
}

#[tokio::test]
async fn failed_verification_restores_the_baseline_and_stops_pending_draft_writes() {
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let fake = FakeModelServer::start().await;
    let engine = engine(&fake, &designs, &sessions);
    let original: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    designs.save("candidate-1", &original).await.unwrap();
    let mut unverified = original.clone();
    unverified.title = "Unverified".to_owned();
    let saver = LiveSaver::new(&engine, "candidate-1");
    saver.offer(unverified.clone(), 1);
    saver.abort(Some(&original)).await.unwrap();
    saver.offer(unverified, 2);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert_eq!(
        designs.load("candidate-1").await.unwrap().unwrap(),
        original
    );
    let fresh = LiveSaver::new(&engine, "candidate-2");
    fresh.offer(original, 1);
    fresh.abort(None).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(designs.load("candidate-2").await.unwrap().is_none());
}

/// A fresh one-candidate, low-effort session, still in intake.
async fn fresh_session(sessions: &SessionStore, request: &str) {
    sessions
        .create(NewSession::demo("talk", "Talk", request).with_options(low_effort_options()))
        .await
        .unwrap();
}

/// A session past its setup card: the app's own questions were
/// asked, so the next planner turn is free to write.
async fn set_up_session(sessions: &SessionStore, request: &str) {
    fresh_session(sessions, request).await;
    sessions
        .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
}

#[test]
fn a_run_writes_one_candidate_per_canvas_per_variation() {
    let platforms = vec!["desktop web".to_owned(), "phone".to_owned()];
    let plans = candidate_plans("talk", &platforms, 2, 1);
    assert_eq!(plans.len(), 4);
    let ids: Vec<&str> = plans.iter().map(|plan| plan.design_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "talk-candidate-1",
            "talk-candidate-2",
            "talk-candidate-3",
            "talk-candidate-4"
        ]
    );
    // The canvases run in the order the user picked them, and the
    // same two concepts are drawn on each.
    assert_eq!(plans[0].viewport, design_model::Viewport::default());
    assert_eq!(plans[1].variation, 2);
    assert_eq!(plans[2].viewport.width, 390);
    assert_eq!(plans[3].variation, 2);
}

#[test]
fn one_canvas_leaves_the_candidate_numbering_alone() {
    let one = [String::new()];
    let plans = candidate_plans("talk", &one, 3, 1);
    assert_eq!(plans.len(), 3);
    assert_eq!(plans[0].design_id, "talk-candidate-1");
    assert_eq!(plans[2].variation, 3);
    // Zero variations still writes one candidate.
    assert_eq!(candidate_plans("talk", &one, 0, 1).len(), 1);
}

#[test]
fn a_second_run_numbers_after_the_first() {
    let one = [String::new()];
    let plans = candidate_plans("talk", &one, 2, 4);
    let ids: Vec<&str> = plans.iter().map(|plan| plan.design_id.as_str()).collect();
    assert_eq!(ids, ["talk-candidate-4", "talk-candidate-5"]);
    assert_eq!(plans[1].candidate_number, 5);
    assert_eq!(plans[1].variation, 2);
    // A first number of zero still starts at one.
    assert_eq!(
        candidate_plans("talk", &one, 1, 0)[0].design_id,
        "talk-candidate-1"
    );
}

#[test]
fn system_prompt_names_the_clarification_protocol() {
    let prompt = system_prompt();
    assert!(prompt.contains("The request and the answers are authoritative"));
    assert!(prompt.contains("needs_clarification"));
    assert!(prompt.contains("\"viewport\""));
}

#[tokio::test]
async fn the_planner_reads_the_users_source_files() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Writing it now.","generate":true}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let uploads = UploadStore::new(directory.path().join("uploads"));
    uploads
        .save(
            "talk",
            "spec.md",
            b"The deck is for iOS developers new to Swift.",
        )
        .await
        .unwrap();
    fresh_session(&sessions, "An intro deck.").await;
    let _ = engine(&server, &designs, &sessions)
        .with_uploads(uploads)
        .run("talk", silent_log())
        .await;
    // Planning blind is what produced questions the files already
    // answered, so the first call must carry the file.
    let planner_call = server.requests()[0].to_string();
    assert!(planner_call.contains("The user's source files follow"));
    assert!(planner_call.contains("iOS developers new to Swift"));
}

#[test]
fn an_office_file_reaches_the_prompt_as_its_text() {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    std::io::Write::write_all(
        &mut writer,
        b"<w:document><w:p><w:r><w:t>Pricing starts at 9 a month.</w:t></w:r></w:p></w:document>",
    )
    .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let attachments = Attachments {
        files: vec![UploadAttachment {
            name: "brief.docx".to_owned(),
            content_type: DOCX.to_owned(),
            bytes,
        }],
        skipped: Vec::new(),
    };
    let content = user_content_with_attachments("Go.", &attachments, false).to_string();
    assert!(content.contains("File brief.docx"));
    assert!(content.contains("Pricing starts at 9 a month."));
    assert!(!content.contains("cannot carry"));
    // A broken archive is reported, not dropped in silence.
    let broken = Attachments {
        files: vec![UploadAttachment {
            name: "brief.docx".to_owned(),
            content_type: DOCX.to_owned(),
            bytes: b"not a zip".to_vec(),
        }],
        skipped: Vec::new(),
    };
    let content = user_content_with_attachments("Go.", &broken, false).to_string();
    assert!(content.contains("could not be read"));
}

#[test]
fn a_source_file_of_unknown_type_reaches_the_prompt_as_its_text() {
    let source = Attachments {
        files: vec![UploadAttachment {
            name: "main.zig".to_owned(),
            content_type: "application/octet-stream".to_owned(),
            bytes: b"pub fn main() void {}\n".to_vec(),
        }],
        skipped: Vec::new(),
    };
    let content = user_content_with_attachments("Go.", &source, false).to_string();
    assert!(content.contains("pub fn main() void {}"));
    assert!(!content.contains("cannot carry"));
    // A binary file is still named only.
    let binary = Attachments {
        files: vec![UploadAttachment {
            name: "font.bin".to_owned(),
            content_type: "application/octet-stream".to_owned(),
            bytes: vec![0, 159, 146, 150, 0, 1],
        }],
        skipped: Vec::new(),
    };
    let content = user_content_with_attachments("Go.", &binary, false).to_string();
    assert!(content.contains("cannot carry"));
    assert_eq!(text_of(b"plain"), Some("plain"));
    assert_eq!(text_of(b"a\0b"), None);
    assert_eq!(text_of(&[0xff, 0xfe]), None);
}

#[test]
fn the_inlined_text_stops_at_the_request_budget() {
    let file = |name: &str, bytes: usize| UploadAttachment {
        name: name.to_owned(),
        content_type: "text/plain; charset=utf-8".to_owned(),
        bytes: vec![b'~'; bytes],
    };
    // Three files at the per-file cap pass the budget: the third
    // is cut at what is left, the fourth is named only, and the
    // model is told which one it is.
    let attachments = Attachments {
        files: vec![
            file("a.rs", ATTACHMENT_TEXT_LIMIT_BYTES),
            file("b.rs", ATTACHMENT_TEXT_LIMIT_BYTES),
            file("c.rs", ATTACHMENT_TEXT_LIMIT_BYTES),
            file("d.rs", 10),
        ],
        skipped: Vec::new(),
    };
    let content = user_content_with_attachments("Go.", &attachments, false);
    let text = content.to_string();
    assert!(text.contains("File a.rs"));
    assert!(text.contains("File c.rs"));
    assert!(text.contains("[cut: the file continues]"));
    assert!(text.contains("File d.rs (text/plain; charset=utf-8, 10 B): named only"));
    assert!(text.contains("no room for more text: d.rs"));
    let inlined: usize = content
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|part| part["text"].as_str())
        .map(|part| part.matches('~').count())
        .sum();
    assert_eq!(inlined, ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES);
    // A small set is inlined whole, with no note.
    let small = Attachments {
        files: vec![file("a.rs", 10), file("b.rs", 10)],
        skipped: Vec::new(),
    };
    let text = user_content_with_attachments("Go.", &small, false).to_string();
    assert!(!text.contains("named only"));
}

#[test]
fn a_run_of_finish_presses_continues_every_candidate() {
    let messages = vec![
        ChatMessage::user("A landing page.", None),
        ChatMessage::assistant("Two candidates."),
        ChatMessage::continue_request("Finish it.", "talk-candidate-1"),
        ChatMessage::continue_request("Finish it.", "talk-candidate-2"),
        // The same card pressed twice is one request.
        ChatMessage::continue_request("Finish it.", "talk-candidate-2"),
    ];
    assert_eq!(
        trailing_continue_ids(&messages),
        ["talk-candidate-1", "talk-candidate-2"]
    );
}

#[test]
fn a_continue_from_an_earlier_turn_is_that_turns_business() {
    let messages = vec![
        ChatMessage::continue_request("Finish it.", "talk-candidate-1"),
        ChatMessage::user("Make the title bigger.", None),
        ChatMessage::continue_request("Finish it.", "talk-candidate-2"),
    ];
    assert_eq!(trailing_continue_ids(&messages), ["talk-candidate-2"]);
    assert!(trailing_continue_ids(&[]).is_empty());
}

#[tokio::test]
async fn the_first_turn_asks_the_apps_own_questions_before_it_writes() {
    let server = FakeModelServer::start().await;
    // The planner wants to write at once and asks nothing.
    server.push_text(WRITE_PLAN);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    fresh_session(&sessions, "A landing page.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    // The app owns the fixed questions, so the first turn asks them
    // whatever the planner decided.
    assert!(matches!(
        outcome,
        GenerationOutcome::NeedsClarification { question_set: 1 }
    ));
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, WorkflowState::Clarifying);
    let set = sessions
        .read_question_set("talk", 1)
        .await
        .unwrap()
        .unwrap();
    // No question of the agent's own: the studio draws the app's
    // cards in this set's grid.
    assert!(set.questions.is_empty());
    assert!(set.can_proceed_with_assumptions);
    // The planner promised a write it does not get to make, so the
    // card speaks for itself instead of repeating that promise.
    let messages = sessions.messages("talk").await.unwrap();
    assert_eq!(messages[0].content, set.message);
    assert!(!messages[0].content.contains("Writing it now"));
    // Nothing was written: the model was called once, to plan.
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn the_setup_turn_fills_the_app_questions_the_request_answers() {
    let server = FakeModelServer::start().await;
    server.push_text(
            r#"{"reply":"A todo app in Zed's style.","questions":[],"suggestions":{"product_kind":"developer_tool","color_mode":"dark","audience":"newcomers","scope":"whole_app"},"generate":false,"edit":false}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    fresh_session(&sessions, "A TODO app with Zed IDE's aesthetics.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        GenerationOutcome::NeedsClarification { question_set: 1 }
    ));
    // The card opens with the answers the request gave, marked as
    // suggested. A deck axis and an unknown value are dropped.
    let options = sessions.read("talk").await.unwrap().unwrap().options;
    assert_eq!(options.product_kind.as_deref(), Some("developer_tool"));
    assert_eq!(options.color_mode.as_deref(), Some("dark"));
    assert_eq!(options.audience, None);
    assert_eq!(options.scope, None);
    assert_eq!(options.suggested, ["color_mode", "product_kind"]);
}

#[tokio::test]
async fn a_second_turn_writes_without_asking_again() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "A landing page.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
}

#[tokio::test]
async fn fresh_candidates_and_concepts_receive_the_users_follow_up_requirements() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"PLANNER_PROMISE_ONLY","generate":true}"#);
    server.push_text(r#"{"concepts":[]}"#);
    server.push_text(SAMPLE_DESIGN);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let mut options = low_effort_options();
    options.variations = Some(2);
    sessions
        .create(NewSession::demo("talk", "Talk", "A business app.").with_options(options))
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
    for content in [
        "Make another one with Apple glass design.",
        "The entire app must be semi-transparent over a blurred desktop.",
    ] {
        sessions
            .append_message("talk", ChatMessage::user(content, None))
            .await
            .unwrap();
    }
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in &requests[1..] {
        let text = request.to_string();
        assert!(text.contains("A business app."));
        assert!(text.contains("Apple glass design"));
        assert!(text.contains("semi-transparent over a blurred desktop"));
        assert!(text.contains("A later user instruction replaces"));
        assert!(!text.contains("PLANNER_PROMISE_ONLY"));
    }
}

#[tokio::test]
async fn a_planner_question_set_moves_the_session_to_clarifying() {
    let server = FakeModelServer::start().await;
    server.push_text(
            r#"{"reply":"Two things first.","questions":[{"question":"Who is it for?","options":["Developers","Buyers"]}]}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    fresh_session(&sessions, "A landing page.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        GenerationOutcome::NeedsClarification { question_set: 1 }
    ));
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, WorkflowState::Clarifying);
    let set = sessions
        .read_question_set("talk", 1)
        .await
        .unwrap()
        .unwrap();
    assert!(set.can_proceed_with_assumptions);
    assert_eq!(set.questions[0].options[1].label, "Buyers");
    let messages = sessions.messages("talk").await.unwrap();
    assert_eq!(messages[0].question_set, Some(1));
    let runs = sessions.runs("talk").await.unwrap();
    assert_eq!(runs[0].result.as_deref(), Some("asked_questions"));
    // The planner saw the request, not a brief.
    let text = server.requests()[0].to_string();
    assert!(text.contains("Request:"));
    assert!(text.contains("Candidates on the canvas: 0"));
}

#[tokio::test]
async fn a_plain_reply_keeps_the_state_and_lands_in_the_chat() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Hello. Tell me what to build."}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "Hi.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Replied));
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, WorkflowState::Clarifying);
    let messages = sessions.messages("talk").await.unwrap();
    assert_eq!(messages[0].content, "Hello. Tell me what to build.");
    assert_eq!(
        sessions.runs("talk").await.unwrap()[0].result.as_deref(),
        Some("replied")
    );
}

#[tokio::test]
async fn generation_sends_the_request_and_the_answers_not_a_brief() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "A landing page for retail investors.").await;
    engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    let text = server.requests()[1].to_string();
    assert!(text.contains("retail investors"));
    assert!(text.contains("The request and the answers are authoritative"));
    assert!(!text.contains("approved brief"));
}

#[tokio::test]
async fn a_valid_design_reply_is_saved_as_a_candidate_and_recorded_on_the_run() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "A landing page.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert!(designs.load("talk-candidate-1").await.unwrap().is_some());
    let runs = sessions.runs("talk").await.unwrap();
    assert_eq!(runs[0].result.as_deref(), Some("succeeded"));
    assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
    // The planner's reply landed in the chat before the write.
    let messages = sessions.messages("talk").await.unwrap();
    assert_eq!(messages[0].content, "Writing it now.");
    assert_eq!(
        sessions.read("talk").await.unwrap().unwrap().state,
        WorkflowState::Generating
    );
}

#[tokio::test]
async fn a_needs_clarification_reply_moves_the_session_back_to_clarifying() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(
            r#"{"needs_clarification":{"title":"One thing","message":"Which brand color?","questions":[{"id":"color","label":"Which brand color?","kind":"short_text","required":true}]}}"#,
        );
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "A landing page.").await;
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        GenerationOutcome::NeedsClarification { .. }
    ));
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, WorkflowState::Clarifying);
}

#[tokio::test]
async fn an_invalid_design_gets_fix_rounds_then_fails() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    // Low effort gives two fix rounds: three attempts, all empty.
    for _ in 0..3 {
        server.push_text("{}");
    }
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "A landing page.").await;
    let result = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await;
    assert!(result.unwrap_err().contains("fix rounds"));
    let runs = sessions.runs("talk").await.unwrap();
    assert_eq!(runs[0].result.as_deref(), Some("failed"));
}

#[test]
fn answers_count_per_request_from_the_last_write() {
    let record = |at: &str, count: usize| AnswerRecord {
        question_set: 1,
        answers: (0..count)
            .map(|index| design_model::QuestionAnswer {
                question_id: format!("q{index}"),
                values: Vec::new(),
                other_text: None,
                skipped: true,
            })
            .collect(),
        at: at.to_owned(),
    };
    let records = vec![
        record("2026-08-29T07:00:00Z", 3),
        record("2026-08-29T07:30:00Z", 2),
    ];
    let mut wrote = ChatMessage::assistant("I wrote 2 candidates.")
        .with_artifacts(vec!["talk-candidate-1".to_owned()]);
    wrote.at = Some("2026-08-29T07:10:00Z".to_owned());
    let messages = vec![ChatMessage::user("A talk.", None), wrote];
    assert_eq!(answers_since_last_write(&messages, &records), 2);
    assert_eq!(answers_since_last_write(&messages[..1], &records), 5);
}

mod edit;
