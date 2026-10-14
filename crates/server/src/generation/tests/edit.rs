use super::*;

#[test]
fn fresh_app_candidates_require_shared_window_bounds_but_landing_pages_do_not() {
    let mut options = low_effort_options();
    assert!(!options.should_have_app_window());
    options.product_kind = Some("business_app".to_owned());
    assert!(options.should_have_app_window());
    options.scope = Some("landing_page".to_owned());
    assert!(!options.should_have_app_window());
    options.scope = Some("whole_app".to_owned());
    options.product_kind = Some("content_site".to_owned());
    assert!(!options.should_have_app_window());
}

#[test]
fn a_focused_design_edit_shows_only_the_named_screens_and_their_findings() {
    let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    design.screens.push(design.screens[0].clone());
    design.app_window = Some(AppWindow {
        left: 80,
        top: 60,
        width: 1280,
        height: 780,
    });
    let focused = focused_design_json(&design, &[1], false).unwrap();
    let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
    assert_eq!(value["screen_count"], design.screens.len());
    assert_eq!(value["screens"].as_array().unwrap().len(), 1);
    assert_eq!(value["screens"][0]["index"], 1);
    assert_eq!(value["app_window"]["width"], 1280);
    assert_eq!(value["app_window"]["height"], 780);
    let request = SessionRequest {
        request: "A landing page.".to_owned(),
        conversation: String::new(),
        kind: design_model::ArtifactKind::Demo,
        answers: Vec::new(),
        options: low_effort_options(),
    };
    let findings = vec!["screens[1] h1 (0/1): overflow: shorten".to_owned()];
    let input = EditInput {
        instruction: "[screen 2, node 0/1 <h1>: x] Fix it.",
        artifact_json: &focused,
        note: "Only screen 2 is shown.\n",
        findings: &findings,
        conversation: "Conversation, oldest first:\nuser: earlier ask\n",
    };
    let prompt = edit_prompt(&request, &input);
    assert!(prompt.contains("Only screen 2 is shown."));
    assert!(prompt.contains("- screens[1] h1 (0/1): overflow: shorten"));
    assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
    assert!(prompt.contains("Apply only the change asked below."));
    let bare = EditInput {
        conversation: "",
        ..input
    };
    assert!(!edit_prompt(&request, &bare).contains("Apply only the change asked below."));
}

#[tokio::test]
async fn a_finish_pressed_mid_run_is_a_late_continue() {
    let server = FakeModelServer::start().await;
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    design.outline = (1..=6).map(|number| format!("Screen {number}")).collect();
    designs.save("talk-candidate-1", &design).await.unwrap();
    designs.save("talk-candidate-2", &design).await.unwrap();
    set_up_session(&sessions, "A landing page.").await;
    let session = sessions.read("talk").await.unwrap().unwrap();
    let context = GenerationContext {
        request: SessionRequest {
            request: session.request.clone(),
            conversation: String::new(),
            kind: session.artifact_kind,
            answers: Vec::new(),
            options: session.options.clone(),
        },
        options: session.options.clone(),
        session_id: "talk".to_owned(),
    };
    let engine = engine(&server, &designs, &sessions);
    let press = |id: &str| ChatMessage::continue_request("Finish it.", id);
    sessions
        .append_message("talk", press("talk-candidate-1"))
        .await
        .unwrap();
    let started = vec!["talk-candidate-1".to_owned()];
    // Nothing new: the running continue is the only request.
    assert!(
        engine
            .late_continue_ids(&context, &started)
            .await
            .is_empty()
    );
    // The second Finish arrives while the first runs.
    sessions
        .append_message("talk", press("talk-candidate-2"))
        .await
        .unwrap();
    assert_eq!(
        engine.late_continue_ids(&context, &started).await,
        vec!["talk-candidate-2".to_owned()]
    );
}

#[test]
fn a_progress_group_takes_shares_as_they_start() {
    let reported = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink: ProgressSink = {
        let reported = Arc::clone(&reported);
        Arc::new(move |percent| reported.lock().unwrap().push(percent))
    };
    let group = ProgressGroup {
        shares: Arc::new(std::sync::Mutex::new(Vec::new())),
        sink: Some(sink),
        design_sink: None,
        base: 0,
        span: 100,
    };
    let first = group.share("a");
    first(1.0);
    // A second share halves the mean: the bar steps back for the
    // late arrival instead of hiding it.
    let second = group.share("b");
    second(0.0);
    second(0.5);
    assert_eq!(*reported.lock().unwrap(), vec![100, 50, 75]);
}

#[tokio::test]
async fn an_edit_on_a_preview_is_not_a_continue() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the hero.","edit":true}"#);
    server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    design.outline = (1..=6).map(|number| format!("Screen {number}")).collect();
    assert!(design.is_preview());
    designs.save("talk-candidate-1", &design).await.unwrap();
    set_up_session(&sessions, "A landing page.").await;
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    sessions
        .append_message(
            "talk",
            ChatMessage::user("[screen 1] Tighten the hero.", Some("talk-candidate-1")),
        )
        .await
        .unwrap();
    engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    // The planner ran and the patch applied; nothing was continued.
    let planner = server.requests()[0].to_string();
    assert!(planner.contains("You plan software demos"));
    let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
    assert!(edited.screens[0].html.contains("Tighter"));
    assert!(edited.is_preview());
}

#[tokio::test]
async fn a_chat_request_with_a_design_open_patches_that_design() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening the hero.","edit":true}"#);
    server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    designs.save("talk-candidate-1", &design).await.unwrap();
    set_up_session(&sessions, "A landing page.").await;
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    sessions
        .append_message(
            "talk",
            ChatMessage::user("[screen 1] Tighten the hero.", Some("talk-candidate-1")),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Edited { reports, .. } = outcome else {
        panic!("expected a saved edit report");
    };
    assert!(reports[0].contains("Modified screen 1"));
    let messages = sessions.messages("talk").await.unwrap();
    let completion = messages.last().unwrap();
    assert!(completion.content.starts_with(EDIT_REPORT_PREFIX));
    assert!(completion.content.contains("Modified screen 1"));
    assert!(!completion.content.contains("I wrote"));
    assert_eq!(completion.artifacts, ["talk-candidate-1"]);
    let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
    assert!(edited.screens[0].html.contains("Tighter"));
    let text = server.requests()[1].to_string();
    assert!(text.contains("Apply this change: [screen 1] Tighten the hero."));
    let planner = server.requests()[0].to_string();
    assert!(planner.contains("Artifacts named this turn: talk-candidate-1"));
}

#[tokio::test]
async fn a_message_with_pinned_candidates_patches_each_of_them() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Tightening both.","edit":true}"#);
    // Each edit takes a patch reply and one fix-round reply.
    for _ in 0..4 {
        server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
    }
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    designs.save("talk-candidate-1", &design).await.unwrap();
    designs.save("talk-candidate-2", &design).await.unwrap();
    designs.save("talk-candidate-3", &design).await.unwrap();
    set_up_session(&sessions, "A landing page.").await;
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-3".to_owned()];
    sessions
        .append_message(
            "talk",
            ChatMessage::user(
                "[candidate 1] [candidate 3] [screen 1] Tighten the hero.",
                None,
            )
            .with_pinned(pinned),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Edited {
        design_ids,
        reports,
    } = outcome
    else {
        panic!("expected saved edit reports");
    };
    assert_eq!(
        design_ids,
        vec!["talk-candidate-1".to_owned(), "talk-candidate-3".to_owned()]
    );
    assert_eq!(reports.len(), 2);
    assert!(reports[0].contains("Candidate `talk-candidate-1`"));
    assert!(reports[1].contains("Candidate `talk-candidate-3`"));
    assert!(
        reports
            .iter()
            .all(|report| report.contains("Modified screen 1"))
    );
    let messages = sessions.messages("talk").await.unwrap();
    assert_eq!(messages.last().unwrap().artifacts, design_ids);
    for id in ["talk-candidate-1", "talk-candidate-3"] {
        let edited = designs.load(id).await.unwrap().unwrap();
        assert!(edited.screens[0].html.contains("Tighter"), "{id}");
    }
    let untouched = designs.load("talk-candidate-2").await.unwrap().unwrap();
    assert!(!untouched.screens[0].html.contains("Tighter"));
    let planner = server.requests()[0].to_string();
    assert!(planner.contains("Artifacts named this turn: talk-candidate-1, talk-candidate-3"));
}

/// A reviewing session with `count` saved candidates of `design`.
async fn reviewing_session_with(
    sessions: &SessionStore,
    designs: &DesignStore,
    design: &design_model::Design,
    count: usize,
) {
    for number in 1..=count {
        designs
            .save(&format!("talk-candidate-{number}"), design)
            .await
            .unwrap();
    }
    set_up_session(sessions, "A landing page.").await;
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_merge_of_two_pinned_candidates_writes_a_new_one() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    server.push_text(SAMPLE_DESIGN);
    // The polish round, when Chrome can measure: no change.
    server.push_text(r#"{"screens":[]}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    reviewing_session_with(&sessions, &designs, &design, 2).await;
    let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-2".to_owned()];
    sessions
        .append_message(
            "talk",
            ChatMessage::user(
                "[candidate 1] [candidate 2] Hero from 1, pricing from 2.",
                None,
            )
            .with_pinned(pinned),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Wrote { design_ids } = outcome else {
        panic!("expected a write");
    };
    assert_eq!(design_ids, vec!["talk-candidate-3".to_owned()]);
    assert!(designs.load("talk-candidate-3").await.unwrap().is_some());
    let text = server.requests()[1].to_string();
    assert!(text.contains("Combine these candidates into one design"));
    assert!(text.contains("Hero from 1, pricing from 2."));
    assert!(text.contains("Candidate 1:"));
    assert!(text.contains("Candidate 2:"));
    assert!(!text.contains("This is candidate"));
}

#[tokio::test]
async fn a_merge_across_canvases_is_refused() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    reviewing_session_with(&sessions, &designs, &design, 1).await;
    let mut phone = design.clone();
    phone.viewport = design_model::Viewport::for_platform("phone");
    designs.save("talk-candidate-2", &phone).await.unwrap();
    let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-2".to_owned()];
    sessions
        .append_message(
            "talk",
            ChatMessage::user("[candidate 1] [candidate 2] Combine them.", None)
                .with_pinned(pinned),
        )
        .await
        .unwrap();
    let error = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap_err();
    assert!(error.contains("different canvases"), "{error}");
    assert!(designs.load("talk-candidate-3").await.unwrap().is_none());
}

#[tokio::test]
async fn a_merge_with_one_pin_is_an_edit() {
    let server = FakeModelServer::start().await;
    server.push_text(r#"{"reply":"Changing it.","merge":true}"#);
    server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:64px;}"}}]}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    reviewing_session_with(&sessions, &designs, &design, 1).await;
    sessions
        .append_message(
            "talk",
            ChatMessage::user("[candidate 1] [screen 1] Tighten the hero.", None)
                .with_pinned(vec!["talk-candidate-1".to_owned()]),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Edited { design_ids, .. } = outcome else {
        panic!("expected an edit");
    };
    assert_eq!(design_ids, vec!["talk-candidate-1".to_owned()]);
    let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
    assert!(edited.screens[0].html.contains("Tighter"));
}

#[tokio::test]
async fn a_regenerated_screen_is_written_without_its_old_markup() {
    let server = FakeModelServer::start().await;
    // No planner turn: the request names its screen itself.
    server.push_text(r#"{"screens":[{"index":0,"screen":{"name":"Hero","html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:64px;}"}}]}"#);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let mut design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    design.screens[0].html = "<h1>Old hero markup</h1>".to_owned();
    reviewing_session_with(&sessions, &designs, &design, 1).await;
    sessions
        .append_message(
            "talk",
            ChatMessage::regenerate_request(
                "[screen 1] Write this screen anew.",
                "talk-candidate-1",
            ),
        )
        .await
        .unwrap();
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    assert!(matches!(outcome, GenerationOutcome::Edited { .. }));
    let text = server.requests()[0].to_string();
    assert!(text.contains("Write screen 1 of"));
    assert!(text.contains("anew"));
    assert!(!text.contains("Old hero markup"));
    let edited = designs.load("talk-candidate-1").await.unwrap().unwrap();
    assert!(edited.screens[0].html.contains("Fresh"));
    assert_eq!(edited.screens.len(), design.screens.len());
}

#[tokio::test]
async fn a_later_run_numbers_after_the_candidates_the_session_has() {
    let server = FakeModelServer::start().await;
    server.push_text(WRITE_PLAN);
    server.push_text(SAMPLE_DESIGN);
    let directory = tempfile::tempdir().unwrap();
    let designs = DesignStore::new(directory.path().join("designs"));
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let design: design_model::Design = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    reviewing_session_with(&sessions, &designs, &design, 2).await;
    sessions
        .append_message("talk", ChatMessage::user("Another take.", None))
        .await
        .unwrap();
    let outcome = engine(&server, &designs, &sessions)
        .run("talk", silent_log())
        .await
        .unwrap();
    let GenerationOutcome::Wrote { design_ids } = outcome else {
        panic!("expected a write");
    };
    assert_eq!(design_ids, vec!["talk-candidate-3".to_owned()]);
}

#[test]
fn the_edit_targets_are_the_pins_then_the_open_artifact_then_the_chosen_one() {
    let pinned = vec![
        ChatMessage::user("Bigger.", Some("talk-candidate-1")).with_pinned(vec![
            "talk-candidate-2".to_owned(),
            "talk-candidate-3".to_owned(),
        ]),
    ];
    assert_eq!(
        edit_targets(&pinned, Some("talk-candidate-1")),
        vec!["talk-candidate-2".to_owned(), "talk-candidate-3".to_owned()]
    );
    let open = vec![ChatMessage::user("Bigger.", Some("talk-candidate-1"))];
    assert_eq!(
        edit_targets(&open, Some("talk-candidate-2")),
        vec!["talk-candidate-1".to_owned()]
    );
    let plain = vec![
        ChatMessage::user("Bigger.", None),
        ChatMessage::assistant("Done."),
    ];
    assert_eq!(
        edit_targets(&plain, Some("talk-candidate-2")),
        vec!["talk-candidate-2".to_owned()]
    );
    assert!(edit_targets(&plain, None).is_empty());
}
