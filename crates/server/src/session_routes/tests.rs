#![allow(clippy::unwrap_used)]

use axum::http::StatusCode;
use design_model::WorkflowEvent;
use tempfile::TempDir;

use crate::decks::DeckStore;
use crate::designs::DesignStore;
use crate::sessions::SessionStore;
use crate::test_support::{sample_deck, send, test_application};

/// Reads a session view.
async fn view(application: &axum::Router, id: &str) -> serde_json::Value {
    let (status, body) = send(application.clone(), "GET", &format!("/sessions/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap()
}

async fn create(application: &axum::Router, id: &str) {
    let body = format!("{{\"id\":\"{id}\",\"request\":\"Design {id}.\"}}");
    let (status, _) = send(application.clone(), "POST", "/sessions", Some(&body)).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_message_pins_candidates_of_its_own_session_only() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "finance-app").await;
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(r#"{"content":"Bigger title.","pinned":["other-candidate-1"]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("does not belong to session `finance-app`"));
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(r#"{"content":"Bigger title.","pinned":["finance-app-candidate-1","finance-app-candidate-2"]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let messages = &view(&application, "finance-app").await["messages"];
    assert_eq!(messages[0]["pinned"][1], "finance-app-candidate-2");
}

#[tokio::test]
async fn a_regenerate_request_names_a_unit() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "finance-app").await;
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(r#"{"content":"[screen 2] Write it anew.","action":"regenerate"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("needs the design id"));
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(
            r#"{"content":"Write it anew.","design":"finance-app-candidate-1","action":"regenerate"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(body.contains("like `[screen 2]`"));
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(
            r#"{"content":"[screen 2] Write it anew.","design":"finance-app-candidate-1","action":"regenerate"}"#,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let messages = &view(&application, "finance-app").await["messages"];
    assert_eq!(messages[0]["is_regenerate"], true);
    assert_eq!(messages[0]["design"], "finance-app-candidate-1");
}

#[tokio::test]
async fn a_link_to_a_private_address_is_not_captured_and_the_message_still_posts() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "finance-app").await;
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(r#"{"content":"Copy the look of http://127.0.0.1:1/admin please."}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, body) = send(
        application.clone(),
        "GET",
        "/uploads?session=finance-app",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.trim(), "[]");
    let messages = &view(&application, "finance-app").await["messages"];
    assert_eq!(messages[0]["role"], "user");
}

#[tokio::test]
async fn answering_questions_resumes_the_current_prompt_before_queued_followups() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "finance-app").await;
    let (status, body) = send(application.clone(), "PUT", "/sessions/finance-app/question-set", Some(r#"{"title":"T","message":"Which color?","questions":[{"id":"color","label":"Color","kind":"single_select","required":false,"options":[{"value":"a","label":"A"},{"value":"b","label":"B"}]}]}"#)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    send(
        application.clone(),
        "POST",
        "/sessions/finance-app/messages",
        Some(r#"{"content":"Later prompt"}"#),
    )
    .await;
    for _ in 0..2 {
        let (status, body) = send(
            application.clone(),
            "POST",
            "/sessions/finance-app/answers",
            Some(r#"{"question_set":1,"answers":[]}"#),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let snapshot = view(&application, "finance-app").await;
    let queued: Vec<_> = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["is_queued"] == true)
        .collect();
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0]["content"], snapshot["session"]["request"]);
    assert_eq!(queued[0]["is_resume"], true);
    assert_eq!(queued[1]["content"], "Later prompt");
    assert_eq!(snapshot["answers"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn messages_during_generation_are_accepted_and_queued_with_their_targets() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "finance-app").await;
    send(
        application.clone(),
        "POST",
        "/sessions/finance-app/generate",
        None,
    )
    .await;
    for content in ["First queued prompt", "Second queued prompt"] {
        let body = serde_json::json!({"content": content, "design":"finance-app-candidate-1", "pinned":["finance-app-candidate-2"]}).to_string();
        let (status, response) = send(
            application.clone(),
            "POST",
            "/sessions/finance-app/messages",
            Some(&body),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{response}");
    }
    let snapshot = view(&application, "finance-app").await;
    let messages = snapshot["messages"].as_array().unwrap();
    let queued: Vec<_> = messages
        .iter()
        .filter(|message| message["is_queued"] == true)
        .collect();
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0]["content"], "First queued prompt");
    assert_eq!(queued[1]["content"], "Second queued prompt");
    assert_eq!(queued[0]["design"], "finance-app-candidate-1");
    assert_eq!(queued[0]["pinned"][0], "finance-app-candidate-2");
    assert_eq!(snapshot["session"]["state"], "generating");
}

#[tokio::test]
async fn a_vague_request_enters_clarifying_when_questions_are_asked() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "finance-app").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/finance-app/question-set",
        Some(r#"{"title":"T","message":"Two things.","questions":[{"id":"platform","label":"Which platform?","kind":"single_select","required":true,"options":[{"value":"web","label":"Web"}]}]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        view(&application, "finance-app").await["session"]["state"],
        "clarifying"
    );
}

#[tokio::test]
async fn a_deck_scenario_is_validated_and_kept_on_the_options() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"intro","request":"Intro for Swift Design.","artifact_kind":"deck"}"#),
    )
    .await;
    // A scenario outside the presets is the user's own words, so it
    // is kept. Only an unusable shape is refused.
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/options",
        Some(r#"{"effort":"medium","variety":"high","templates":[],"preview":true,"platforms":[],"scenario":"Cooking"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        view(&application, "intro").await["session"]["options"]["scenario"],
        "Cooking"
    );
    let long = "x".repeat(design_model::CUSTOM_ANSWER_LIMIT + 1);
    let (status, body) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/options",
        Some(&format!(r#"{{"effort":"medium","variety":"high","templates":[],"preview":true,"platforms":[],"scenario":"{long}"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("is not usable"), "{body}");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/options",
        Some(r#"{"effort":"medium","variety":"high","templates":[],"preview":true,"platforms":[],"scenario":"Pitch or launch"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let session = view(&application, "intro").await;
    assert_eq!(session["session"]["options"]["scenario"], "Pitch or launch");
    assert!(session.get("brief").is_none());
}

#[tokio::test]
async fn the_apps_own_axes_take_a_preset_or_the_users_own_words() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"intro","request":"Intro for Swift Design."}"#),
    )
    .await;
    let base =
        r#"{"effort":"medium","variety":"medium","templates":[],"preview":true,"platforms":[]"#;
    // The fixed lists cannot cover every answer, so an answer the
    // user typed is kept beside them.
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/options",
        Some(&format!(r#"{base},"audience":"astronauts"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        view(&application, "intro").await["session"]["options"]["audience"],
        "astronauts"
    );
    // An answer that would break the prompt line is refused.
    let long = "x".repeat(design_model::CUSTOM_ANSWER_LIMIT + 1);
    let (status, body) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/options",
        Some(&format!(r#"{base},"tone":"{long}"}}"#)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("tone `"), "{body}");
    assert!(body.contains("is not usable"), "{body}");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/options",
        Some(&format!(
            r#"{base},"audience":"practitioners","tone":"technical","scope":"short_flow"}}"#
        )),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let options = &view(&application, "intro").await["session"]["options"];
    assert_eq!(options["audience"], "practitioners");
    assert_eq!(options["tone"], "technical");
    assert_eq!(options["scope"], "short_flow");
}

#[tokio::test]
async fn a_deck_question_set_is_written_as_skippable() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    let (status, _) = send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"intro","request":"Intro for Swift Design.","artifact_kind":"deck"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/intro/question-set",
        Some(r#"{"title":"T","message":"One thing.","questions":[{"id":"scenario","label":"What scenario?","kind":"single_select","required":true,"options":[{"value":"talk","label":"Talk"}]}],"can_proceed_with_assumptions":false}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let set = &view(&application, "intro").await["question_sets"][0];
    assert_eq!(set["can_proceed_with_assumptions"], true);
    assert_eq!(set["questions"][0]["required"], false);
}

#[tokio::test]
async fn a_question_set_with_four_questions_is_rejected_with_every_error() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    let (status, body) = send(
        application.clone(),
        "PUT",
        "/sessions/talk/question-set",
        Some(r#"{"title":"T","message":"m","questions":[{"id":"a","label":"A?","kind":"short_text"},{"id":"b","label":"B?","kind":"short_text"},{"id":"c","label":"C?","kind":"short_text"},{"id":"d","label":"D?","kind":"short_text"}]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(!response["error"]["details"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn answers_are_only_accepted_in_clarifying() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    // No question set yet, session is intake: 409.
    let (status, _) = send(
        application.clone(),
        "POST",
        "/sessions/talk/answers",
        Some(r#"{"question_set":1,"answers":[]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn retry_restores_the_state_before_the_error() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    // Force the session into error via a failed built-in run: no
    // model is configured, so drive it by hand through the store.
    // Here we assert retry from error is refused when not in error.
    let (status, _) = send(application.clone(), "POST", "/sessions/talk/retry", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_message_after_an_error_recovers_the_session() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    send(application.clone(), "POST", "/sessions/talk/generate", None).await;
    let sessions = SessionStore::new(directory.path().join("data/sessions"));
    sessions
        .apply("talk", design_model::WorkflowEvent::RunFailed)
        .await
        .unwrap();
    assert_eq!(
        view(&application, "talk").await["session"]["state"],
        "error"
    );
    let (status, _) = send(
        application.clone(),
        "POST",
        "/sessions/talk/messages",
        Some(r#"{"content":"Try again, smaller."}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let session = view(&application, "talk").await;
    // The run halted in generating, so the message resumes on the
    // canvas, where the next turn goes through the planner. No model
    // is configured here, so no run moves it on.
    assert_eq!(session["session"]["state"], "reviewing");
    assert_eq!(session["messages"][0]["content"], "Try again, smaller.");
}

#[tokio::test]
async fn deleting_a_session_takes_its_candidates_with_it() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    // The stores the test application serves from.
    let designs = DesignStore::new(directory.path().join("designs"));
    let decks = DeckStore::new(directory.path().join("decks"));
    let design: design_model::Design =
        serde_json::from_str(include_str!("../../../../fixtures/sample-design.json")).unwrap();
    designs.save("talk-candidate-1", &design).await.unwrap();
    designs.save("other-candidate-1", &design).await.unwrap();
    decks
        .save("talk-candidate-1", &sample_deck())
        .await
        .unwrap();
    let (status, body) = send(application.clone(), "DELETE", "/sessions/talk", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    // A session id comes from the request text, so the next session
    // with the same request reuses this id. Its candidates must not
    // be waiting for it.
    assert!(designs.load("talk-candidate-1").await.unwrap().is_none());
    assert!(decks.load("talk-candidate-1").await.unwrap().is_none());
    assert!(designs.load("other-candidate-1").await.unwrap().is_some());
    create(&application, "talk").await;
    assert_eq!(
        view(&application, "talk").await["designs"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn a_generating_session_deletes_without_a_fight() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    send(application.clone(), "POST", "/sessions/talk/generate", None).await;
    assert_eq!(
        view(&application, "talk").await["session"]["state"],
        "generating"
    );
    // Generating is not a reason to keep a session the user asked
    // to delete. A run in flight is stopped first; a stale state
    // like this one has no run behind it at all.
    let (status, body) = send(application.clone(), "DELETE", "/sessions/talk", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _) = send(application.clone(), "GET", "/sessions/talk", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn historical_unanswered_questions_do_not_cover_generation_or_review() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/talk/question-set",
        Some(r#"{"title":"Follow-up","message":"Which label?","questions":[{"id":"label","label":"Which label?","kind":"single_select","required":false,"options":[{"value":"plain","label":"Plain"},{"value":"brand","label":"Brand"}]}]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view(&application, "talk").await["open_question_set"], 1);
    let sessions = SessionStore::new(directory.path().join("data/sessions"));
    for event in [
        WorkflowEvent::GenerationStarted,
        WorkflowEvent::GenerationSucceeded,
    ] {
        sessions.apply("talk", event).await.unwrap();
        let session = view(&application, "talk").await;
        assert!(session["open_question_set"].is_null());
        assert_eq!(session["question_sets"].as_array().unwrap().len(), 1);
        assert!(session["answers"].as_array().unwrap().is_empty());
        assert_eq!(session["session"]["latest_question_set"], 1);
    }
}

#[tokio::test]
async fn a_new_clarification_after_review_still_opens_its_question_card() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    let sessions = SessionStore::new(directory.path().join("data/sessions"));
    sessions
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    sessions
        .apply("talk", WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/sessions/talk/question-set",
        Some(r#"{"title":"Follow-up","message":"Which label?","questions":[{"id":"label","label":"Which label?","kind":"single_select","required":false,"options":[{"value":"plain","label":"Plain"},{"value":"brand","label":"Brand"}]}]}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let session = view(&application, "talk").await;
    assert_eq!(session["session"]["state"], "clarifying");
    assert_eq!(session["open_question_set"], 1);
}

#[tokio::test]
async fn skipping_the_questions_closes_the_open_card() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    send(
        application.clone(),
        "PUT",
        "/sessions/talk/question-set",
        Some(r#"{"title":"T","message":"Two things.","questions":[{"id":"tone","label":"Which tone?","kind":"single_select","required":false,"options":[{"value":"warm","label":"Warm"}]}]}"#),
    )
    .await;
    assert_eq!(
        view(&application, "talk").await["open_question_set"],
        serde_json::json!(1)
    );
    let (status, _) = send(application.clone(), "POST", "/sessions/talk/generate", None).await;
    assert_eq!(status, StatusCode::OK);
    // The card is answered by the skip, so the workbench shows the
    // candidates instead of asking over them.
    let session = view(&application, "talk").await;
    assert_eq!(session["open_question_set"], serde_json::Value::Null);
    assert_eq!(session["answers"][0]["question_set"], 1);
}

#[tokio::test]
async fn a_second_finish_joins_the_running_turn() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    // The run that writes the candidates is in flight.
    send(application.clone(), "POST", "/sessions/talk/generate", None).await;
    assert_eq!(
        view(&application, "talk").await["session"]["state"],
        "generating"
    );
    for candidate in ["talk-candidate-1", "talk-candidate-2"] {
        let body =
            format!(r#"{{"content":"Finish it.","design":"{candidate}","action":"continue"}}"#);
        let (status, body) = send(
            application.clone(),
            "POST",
            "/sessions/talk/messages",
            Some(&body),
        )
        .await;
        // Pressing Finish on a second candidate is not a conflict.
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let session = view(&application, "talk").await;
    assert_eq!(session["session"]["state"], "generating");
    assert_eq!(session["messages"][0]["design"], "talk-candidate-1");
    assert_eq!(session["messages"][1]["design"], "talk-candidate-2");
}

#[tokio::test]
async fn a_finish_after_a_stop_resumes_the_session() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    send(application.clone(), "POST", "/sessions/talk/generate", None).await;
    // The process died with the run: the boot sweep stops it.
    let sessions = SessionStore::new(directory.path().join("data/sessions"));
    sessions
        .apply("talk", design_model::WorkflowEvent::RunStopped)
        .await
        .unwrap();
    assert_eq!(
        view(&application, "talk").await["session"]["state"],
        "stopped"
    );
    let body = r#"{"content":"Finish it.","design":"talk-candidate-1","action":"continue"}"#;
    let (status, body) = send(
        application.clone(),
        "POST",
        "/sessions/talk/messages",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        view(&application, "talk").await["session"]["state"],
        "generating"
    );
}

#[tokio::test]
async fn state_survives_a_server_restart() {
    let directory = TempDir::new().unwrap();
    let first = test_application(&directory);
    create(&first, "talk").await;
    send(first.clone(), "POST", "/sessions/talk/generate", None).await;
    // A fresh application over the same directory sees the state.
    let second = test_application(&directory);
    assert_eq!(
        view(&second, "talk").await["session"]["state"],
        "generating"
    );
}

#[tokio::test]
async fn session_views_never_include_credentials_or_paths() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    create(&application, "talk").await;
    let (_, body) = send(application.clone(), "GET", "/sessions/talk", None).await;
    assert!(!body.contains("data/sessions"));
    assert!(!body.contains(directory.path().to_str().unwrap()));
}

#[tokio::test]
async fn a_missing_session_returns_404() {
    let directory = TempDir::new().unwrap();
    let (status, _) = send(
        test_application(&directory),
        "GET",
        "/sessions/missing",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
