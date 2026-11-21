use super::*;

#[tokio::test]
async fn navigation_repair_retries_from_the_best_draft_and_stops_at_ten() {
    for finish in [true, false] {
        let fake = ParallelFakeModel::start(None).await;
        let original = sample_design();
        let candidate = ReviewingCandidate::new(&fake);
        candidate
            .seed(&original, "Update screen 1 and keep navigation working.")
            .await;
        fake.watch_baseline(&candidate.path()).await;
        fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
        fake.push_text(r#"{"instruction":"Update the launch link","screens":[0],"minimum_new_screens":0,"requirements":["Launch link"]}"#.into());
        let mut screen = original.screens[0].clone();
        screen.html = "<h1>Best draft</h1><a href='#screen-999'>Open</a>".into();
        fake.push_text(json!({"screens":[{"index":0,"screen":screen}]}).to_string());
        for _ in 0..if finish { 1 } else { 10 } {
            screen.html = "<h1>Rejected draft</h1><a href='#screen-999'>Open</a>".into();
            fake.push_text(json!({"screens":[{"index":0,"screen":screen}]}).to_string());
        }
        if finish {
            screen.html = "<h1>Repaired</h1><a href='#screen-2'>Open</a>".into();
            fake.push_text(json!({"screens":[{"index":0,"screen":screen}]}).to_string());
        }
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        let id = "talk-candidate-1";
        let saved = candidate
            .designs
            .load(id)
            .await
            .expect("read snapshot")
            .expect("snapshot exists");
        let recovery = candidate
            .designs
            .load_review_draft(id)
            .await
            .expect("read recovery");
        let requests = fake.requests();
        assert_eq!(requests.len(), if finish { 5 } else { 13 });
        for request in &requests[3..] {
            let prompt = request_text(request);
            assert!(prompt.contains("Best draft"));
            assert!(!prompt.contains("Rejected draft"));
            assert!(prompt.contains("Repair only these verified issues"));
        }
        if finish {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(saved.screens[0], screen);
            assert_eq!(saved.screens[1..], original.screens[1..]);
            assert!(recovery.is_none());
        } else {
            let error = result.expect_err("unresolved links must not save");
            assert!(
                error.contains("10-correction-round limit") && error.contains("Screen 1"),
                "{error}"
            );
            assert_eq!(saved, original);
            assert!(
                recovery.expect("keep the best failed draft").screens[0]
                    .html
                    .contains("Best draft")
            );
        }
        assert!(
            !fake
                .state
                .baseline_changed_during_requests
                .load(Ordering::SeqCst)
        );
    }
}

#[tokio::test]
async fn browser_probe_failure_is_sent_to_the_writer_and_rechecked_before_saving() {
    let fake = ParallelFakeModel::start(None).await;
    let original = sample_design();
    let mut candidate = ReviewingCandidate::new(&fake);
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let coverage = json!((0..design.screens.len()).collect::<Vec<_>>());
        let checks = if design.screens[0].html.contains("Broken menu") {
            json!([{"screen":0,"group":"menu","clicks":1,"keys":1,"focus":1,"failures":1,"issues":["Action did not open panel"]}])
        } else {
            json!([])
        };
        let report = format!(
            "<html data-swift-design-findings=\"[]\" data-swift-design-audited-screens=\"{coverage}\" data-swift-design-prototype-checks=\"{}\">",
            checks.to_string().replace('"', "&quot;")
        );
        Box::pin(async move { Ok(report) })
    });
    candidate.engine = candidate.engine.with_demo_dom_runner(runner);
    candidate
        .seed(&original, "Update only the menu on screen 1.")
        .await;
    fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
    fake.push_text(r#"{"instruction":"Update menu","screens":[0],"minimum_new_screens":0,"requirements":["Menu interaction"]}"#.into());
    for html in ["<h1>Broken menu</h1>", "<h1>Repaired menu</h1>"] {
        let mut screen = original.screens[0].clone();
        screen.html = html.into();
        fake.push_text(json!({"screens":[{"index":0,"screen":screen}]}).to_string());
    }
    candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect("repaired probe should pass");
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .expect("read snapshot")
        .expect("snapshot exists");
    assert!(saved.screens[0].html.contains("Repaired menu"));
    assert_eq!(saved.screens[1..], original.screens[1..]);
    let requests = fake.requests();
    assert_eq!(requests.len(), 4);
    assert!(request_text(&requests[3]).contains("Action did not open panel"));
}

#[tokio::test]
async fn missing_required_screen_is_repaired_before_the_edit_can_save() {
    let fake = ParallelFakeModel::start(None).await;
    let original = sample_design();
    let candidate = ReviewingCandidate::new(&fake);
    candidate
        .seed(&original, "Add a standalone detail screen.")
        .await;
    fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
    fake.push_text(r#"{"instruction":"Append Detail","screens":[],"minimum_new_screens":1,"requirements":["A standalone detail screen"]}"#.into());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &json!(original),
        "completed original screen",
    );
    fake.push_text(r#"{"screens":[]}"#.into());
    let mut detail = original.screens[0].clone();
    detail.name = "Detail".into();
    detail.html = "<h1>Detail</h1><a href='#screen-1'>Back</a>".into();
    fake.push_text(
        json!({"screens":[{"index":original.screens.len(),"insert":true,"screen":detail}]})
            .to_string(),
    );
    candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect("adding the required screen should save");
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .expect("read snapshot")
        .expect("snapshot exists");
    assert_eq!(saved.screens.last(), Some(&detail));
    for (screen, before) in saved.screens.iter().zip(&original.screens) {
        assert_eq!(screen.html, before.html);
        assert_eq!(screen.name, before.name);
    }
    let requests = fake.requests();
    let correction = request_text(requests.last().expect("correction was sent"));
    assert!(correction.contains("still needs 1 required separate screen"));
    assert!(correction.contains("completed original screen"));
}

#[tokio::test]
async fn desktop_repair_reuses_retained_work_without_publishing_new_mobile_screens() {
    let fake = ParallelFakeModel::start(None).await;
    let mut original = sample_design();
    original.screens[1].name = "Existing · 390px mobile".into();
    let candidate = ReviewingCandidate::new(&fake);
    let id = "talk-candidate-1";
    candidate.seed(&original, "Redesign Task Details.").await;
    let mut retained = original.clone();
    retained.screens[1].html = "<h1>Unapproved mobile change</h1>".into();
    let mobile_index = original.screens.len();
    let desktop_index = mobile_index + 1;
    retained.screens[0].html = format!(
        "<h1>Desktop Task</h1><a href='#screen-{}'>Details</a><a href='#screen-{}'>Mobile specimen</a>",
        desktop_index + 1,
        mobile_index + 1
    );
    retained.screens.push(design_model::Screen {
        name: "Task · Mobile".into(),
        html: "<h1>Unapproved mobile addition</h1>".into(),
        css: None,
        notes: None,
    });
    retained.screens.push(design_model::Screen {
        name: "Task · Desktop details".into(),
        html: "<h1>Desktop details</h1><a href='#screen-1'>Back</a>".into(),
        css: None,
        notes: None,
    });
    candidate
        .designs
        .keep_review_draft(id, &retained, Some(&original))
        .await
        .expect("persist retained draft");
    let mut message = ChatMessage::user(
        "Desktop-only draft repair.\nPreserve existing mobile screens.",
        Some(id),
    );
    message.is_review_draft = true;
    candidate
        .sessions
        .append_message("talk", message)
        .await
        .expect("queue repair");
    fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
    fake.push_text(r#"{"instruction":"Keep the completed desktop details","screens":[],"minimum_new_screens":1,"requirements":["Desktop details"]}"#.into());
    fake.push_text(r#"{"screens":[]}"#.into());
    let mut corrected = retained.screens[0].clone();
    corrected.html = format!(
        "<h1>Desktop Task</h1><a href='#screen-{}'>Details</a>",
        mobile_index + 1
    );
    fake.push_text(json!({"screens":[{"index":0,"screen":corrected}]}).to_string());
    candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect("desktop repair should save");
    let saved = candidate
        .designs
        .load(id)
        .await
        .expect("read snapshot")
        .expect("snapshot exists");
    assert_eq!(saved.screens.len(), original.screens.len() + 1);
    assert_eq!(saved.screens[1], original.screens[1]);
    assert_eq!(saved.screens.last(), retained.screens.last());
    assert_eq!(saved.screens[0], corrected);
    let repair = request_text(&fake.requests()[3]);
    assert!(repair.contains("internal link has no destination"));
    assert!(repair.contains(&format!("#screen-{}", desktop_index + 1)));
}
