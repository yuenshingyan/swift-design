use design_model::{ArtifactKind, WorkflowEvent};
use serde_json::json;

use super::*;

#[tokio::test]
async fn unsupported_intake_stops_before_setup_questions_or_a_promise_for_every_artifact() {
    for kind in ArtifactKind::ALL {
        let server = FakeModelServer::start().await;
        server.push_text(&json!({
            "reply": "I will build the executable prototype now.",
            "generate": true, "edit": true, "merge": true,
            "questions": [{"question": "Which style?", "options": ["A", "B"]}],
            "unsupported_requirements": ["Executing custom JavaScript is unsupported in this artifact"],
            "supported_alternatives": ["Provide linked HTML/CSS state screens and a controller specification, not an executable JavaScript prototype."]
        }).to_string());
        let directory = tempfile::tempdir().expect("capability test directory should be created");
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        sessions
            .create(
                NewSession::demo(
                    "talk",
                    "Prototype",
                    "Require executable JavaScript, not notes.",
                )
                .with_kind(kind)
                .with_options(low_effort_options()),
            )
            .await
            .expect("unsupported request fixture should be created");
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .expect("unsupported request should receive a capability explanation");
        assert!(matches!(outcome, GenerationOutcome::Replied), "{kind:?}");
        let messages = sessions
            .messages("talk")
            .await
            .expect("capability reply should be readable");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].content.contains("custom JavaScript"));
        assert!(messages[0].content.contains("explicit approval"));
        assert!(
            messages[0]
                .content
                .contains("Suggested alternative: Provide linked HTML/CSS state screens")
        );
        assert!(
            messages[0]
                .content
                .contains("not an executable JavaScript prototype")
        );
        assert!(!messages[0].content.contains("I will build"));
        assert_eq!(
            sessions
                .read("talk")
                .await
                .expect("session should load")
                .expect("session should exist")
                .state,
            WorkflowState::Intake
        );
        assert!(
            sessions
                .read_question_set("talk", 1)
                .await
                .expect("question lookup should succeed")
                .is_none()
        );
        assert!(
            designs
                .list()
                .await
                .expect("design listing should succeed")
                .is_empty()
        );
        assert_eq!(server.requests().len(), 1);
        let runs = sessions
            .runs("talk")
            .await
            .expect("capability run should be recorded");
        assert_eq!(runs[0].result.as_deref(), Some("replied"));
        assert!(runs[0].artifacts.is_empty());
    }
}

#[tokio::test]
async fn every_blocker_gets_a_concrete_proposal_and_partial_approval_does_not_start_work() {
    let blockers = [
        "Custom JavaScript controllers cannot execute in this renderer.",
        "The renderer loads theme fonts from Google Fonts, so strict zero-network operation cannot be guaranteed.",
        "The required native time element is not permitted.",
        "One candidate cannot have multiple independent full-workspace viewport sizes.",
    ];
    let alternatives = [
        "Provide linked HTML/CSS event and error-state screens plus a concrete keyboard, batching and recovery specification; JavaScript will not execute.",
        "Keep the existing theme and approve renderer-managed font requests; the result will not be strictly network-free.",
        "Show HKT timestamps in allowed span elements and retain original ISO timestamps in data attributes; this does not provide native time-element semantics.",
        "Keep candidate 4's viewport and shell unchanged and show narrower component specimens within it, not independent full-workspace device layouts. Separate viewport candidates need additional scope approval.",
    ];
    let server = FakeModelServer::start().await;
    for first_blocker in [0, 1] {
        server.push_text(
            &json!({
                "reply": "PROMISE_MUST_NOT_REACH_CHAT", "generate": true, "edit": true,
                "unsupported_requirements": &blockers[first_blocker..],
                "supported_alternatives": &alternatives[first_blocker..],
            })
            .to_string(),
        );
    }
    let directory = tempfile::tempdir().expect("multiple-blocker test directory should be created");
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions,
        "Only candidate 4: require executing JavaScript, zero network requests, native time elements and independent 1440, 768 and 390px full-workspace specimens."
    ).await;
    let engine = engine(&server, &designs, &sessions);
    for first_blocker in [0, 1] {
        if first_blocker == 1 {
            sessions.append_message("talk", ChatMessage::user(
                "I approve static screens and controller specifications instead of JavaScript. Keep the other requirements unchanged.", None
            )).await.expect("partial scope approval should be appended");
        }
        assert!(matches!(
            engine
                .clone()
                .run("talk", silent_log())
                .await
                .expect("blockers should receive actionable proposals"),
            GenerationOutcome::Replied
        ));
        let messages = sessions
            .messages("talk")
            .await
            .expect("proposal reply should load");
        let reply = &messages
            .last()
            .expect("proposal reply should exist")
            .content;
        for (blocker, alternative) in blockers[first_blocker..]
            .iter()
            .zip(&alternatives[first_blocker..])
        {
            assert!(reply.contains(blocker));
            assert!(reply.contains(alternative));
        }
        assert_eq!(
            reply.matches("Suggested alternative:").count(),
            4 - first_blocker
        );
        assert!(reply.contains("not equivalent implementations"));
        assert!(reply.contains("Any remaining mandatory blockers must be resolved"));
        assert!(reply.contains("explicit approval"));
        assert!(
            designs
                .list()
                .await
                .expect("candidate listing should succeed")
                .is_empty()
        );
        assert_eq!(server.requests().len(), first_blocker + 1);
    }
    assert!(
        server.requests()[1]
            .to_string()
            .contains("Keep the other requirements unchanged")
    );
}

#[tokio::test]
async fn unsupported_edits_merges_and_direct_actions_preserve_saved_candidates() {
    for action in ["edit", "merge", "regenerate", "continue", "write"] {
        let server = FakeModelServer::start().await;
        server.push_text(&json!({
            "reply": "PROMISE_MUST_NOT_REACH_CHAT",
            "generate": action == "write", "edit": action == "edit", "merge": action == "merge",
            "questions": [{"question": "Which style?", "options": ["A", "B"]}],
            "unsupported_requirements": ["Custom keyboard handlers cannot execute in the preview"],
            "supported_alternatives": ["Use clickable links between event-state screens and document keyboard handling; custom key handling will not execute."]
        }).to_string());
        let directory =
            tempfile::tempdir().expect("candidate capability test directory should be created");
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        let mut original: design_model::Design =
            serde_json::from_str(SAMPLE_DESIGN).expect("candidate fixture should deserialize");
        original.outline = vec!["Hero".into(), "Details".into(), "More".into()];
        for id in ["talk-candidate-1", "talk-candidate-2"] {
            designs
                .save(id, &original)
                .await
                .expect("baseline candidate should be saved");
        }
        set_up_session(
            &sessions,
            "Require a working keyboard-controlled prototype, not a specification.",
        )
        .await;
        let answers = (0..crate::planner::ANSWERED_QUESTION_LIMIT)
            .map(|index| design_model::QuestionAnswer {
                question_id: format!("style-{index}"),
                values: vec!["Compact".into()],
                other_text: None,
                skipped: false,
            })
            .collect();
        sessions
            .record_answers("talk", 1, answers)
            .await
            .expect("answered-question budget should be exhausted");
        sessions
            .apply("talk", WorkflowEvent::GenerationStarted)
            .await
            .expect("fixture should enter generating");
        if action != "write" {
            sessions
                .apply("talk", WorkflowEvent::GenerationSucceeded)
                .await
                .expect("fixture should enter reviewing");
            let message = match action {
                "regenerate" => ChatMessage::regenerate_request(
                    "[screen 1] Require executable keyboard handlers.",
                    "talk-candidate-1",
                ),
                "continue" => ChatMessage::continue_request(
                    "Finish with executable keyboard handlers.",
                    "talk-candidate-1",
                ),
                _ => ChatMessage::user("Require executable keyboard handlers.", None)
                    .with_pinned(vec!["talk-candidate-1".into(), "talk-candidate-2".into()]),
            };
            sessions
                .append_message("talk", message)
                .await
                .expect("explicit request should be appended");
        }
        let candidate_path = directory.path().join("designs/talk-candidate-1.json");
        let before = tokio::fs::read(&candidate_path)
            .await
            .expect("saved candidate bytes should be readable");
        let outcome = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .expect("unsupported action should receive an explanation");
        assert!(matches!(outcome, GenerationOutcome::Replied), "{action}");
        assert_eq!(server.requests().len(), 1, "{action}");
        assert_eq!(
            tokio::fs::read(&candidate_path)
                .await
                .expect("saved candidate should remain readable"),
            before,
            "{action}"
        );
        assert_eq!(
            designs
                .load("talk-candidate-2")
                .await
                .expect("second candidate should load"),
            Some(original)
        );
        assert_eq!(
            designs
                .list()
                .await
                .expect("candidate listing should succeed")
                .len(),
            2
        );
        let session = sessions
            .read("talk")
            .await
            .expect("session should load")
            .expect("session should exist");
        assert!(
            session.state.can_take_turn(),
            "{action}: {:?}",
            session.state
        );
        let messages = sessions.messages("talk").await.expect("chat should load");
        assert!(
            !messages
                .iter()
                .any(|message| message.content.contains("PROMISE_MUST_NOT_REACH_CHAT"))
        );
        let request = server.requests()[0].to_string();
        assert!(request.contains("not a specification"), "{action}");
        assert!(request.contains("Actual artifact rules"), "{action}");
    }
}

#[tokio::test]
async fn explicit_approval_can_resume_generation_but_retry_alone_cannot() {
    let server = FakeModelServer::start().await;
    let unsupported = r#"{"generate":true,"unsupported_requirements":["Executable JavaScript is unsupported"],"supported_alternatives":["Provide linked HTML/CSS screens and concrete behavior specifications instead of running JavaScript."]}"#;
    server.push_text(unsupported);
    server.push_text(unsupported);
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().expect("approval test directory should be created");
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(
        &sessions,
        "A timeline with executable JavaScript is mandatory.",
    )
    .await;
    let engine = engine(&server, &designs, &sessions);
    for message in [None, Some("Retry. I still require executable JavaScript.")] {
        if let Some(message) = message {
            sessions
                .append_message("talk", ChatMessage::user(message, None))
                .await
                .expect("retry request should be appended");
        }
        assert!(matches!(
            engine
                .clone()
                .run("talk", silent_log())
                .await
                .expect("rejection should be explained"),
            GenerationOutcome::Replied
        ));
        assert!(
            designs
                .list()
                .await
                .expect("candidate listing should succeed")
                .is_empty()
        );
    }
    sessions.append_message("talk", ChatMessage::user("I approve static HTML/CSS screens and concrete behavior specifications instead. No executable JavaScript is required.", None)).await
        .expect("explicit scope approval should be appended");
    let outcome = engine
        .run("talk", silent_log())
        .await
        .expect("approved supported alternative should generate");
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert_eq!(server.requests().len(), 4);
    let approved_request = server.requests()[2].to_string();
    assert!(approved_request.contains("I approve static HTML/CSS"));
    assert!(approved_request.contains("executable JavaScript is mandatory"));
}

#[tokio::test]
async fn source_code_for_review_is_not_treated_as_an_execution_requirement() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"I can show JavaScript source for review without executing it.","unsupported_requirements":[]}"#);
    let directory = tempfile::tempdir().expect("source review test directory should be created");
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(
        &sessions,
        "Show escaped JavaScript source for review; do not run it in the preview.",
    )
    .await;
    assert!(matches!(
        engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .expect("source-only discussion should be supported"),
        GenerationOutcome::Replied
    ));
    let messages = sessions
        .messages("talk")
        .await
        .expect("source-only reply should load");
    assert_eq!(
        messages[0].content,
        "I can show JavaScript source for review without executing it."
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_supported_explicit_write_proceeds_after_its_capability_check() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"unsupported_requirements":[]}"#);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().expect("explicit write test directory should be created");
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    set_up_session(&sessions, "A landing page with supported HTML/CSS.").await;
    sessions
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .expect("explicit write should enter generating");
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .expect("supported explicit write should generate");
    assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
    assert_eq!(server.requests().len(), 2);
    assert!(
        server.requests()[0]
            .to_string()
            .contains("This is a capability check, not a generation request")
    );
    assert_eq!(
        designs
            .list()
            .await
            .expect("candidate listing should succeed")
            .len(),
        1
    );
}

#[tokio::test]
async fn missing_capability_verdicts_stop_without_a_promise_or_generation() {
    for direct in [false, true] {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"PROMISE_MUST_NOT_REACH_CHAT","generate":true}"#);
        let directory =
            tempfile::tempdir().expect("malformed verdict test directory should be created");
        let designs = DesignStore::new(directory.path().join("designs"));
        let sessions = SessionStore::new(directory.path().join("sessions"));
        set_up_session(&sessions, "A landing page.").await;
        if direct {
            sessions
                .apply("talk", WorkflowEvent::GenerationStarted)
                .await
                .expect("explicit write should enter generating");
        }
        let error = engine(&server, &designs, &sessions)
            .run("talk", silent_log())
            .await
            .expect_err("missing capability verdict should fail closed");
        assert!(error.contains("Cannot check request capabilities"));
        assert_eq!(server.requests().len(), 1);
        assert!(
            designs
                .list()
                .await
                .expect("candidate listing should succeed")
                .is_empty()
        );
        assert!(
            !sessions
                .messages("talk")
                .await
                .expect("chat should load")
                .iter()
                .any(|message| message.content.contains("PROMISE_MUST_NOT_REACH_CHAT"))
        );
    }
}
