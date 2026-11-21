use super::*;

#[tokio::test]
async fn repair_retries_a_protected_patch_and_preserves_other_draft_work() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    let candidate = ReviewingCandidate::new(&fake);
    candidate.seed(&original, "Add a timeline screen.").await;
    let id = "talk-candidate-1";
    let mut draft = original.clone();
    let mut added = draft.screens[0].clone();
    draft.screens[0].notes = Some("Completed launch changes in the retained draft".into());
    added.name = "Timeline".into();
    added.html = "<h1>Unique added timeline draft</h1>".into();
    draft.screens.push(added.clone());
    candidate
        .designs
        .keep_review_draft(id, &draft, Some(&original))
        .await
        .expect("draft should checkpoint");
    let mut repair = ChatMessage::user("Fix the remaining issues in this review draft.", Some(id));
    repair.is_review_draft = true;
    candidate
        .sessions
        .append_message("talk", repair)
        .await
        .expect("repair turn should be recorded");
    fake.push_text(r#"{"unsupported_requirements":[]}"#.into());
    fake.push_text(
        json!({
            "instruction":"Fix the timeline", "screens":[original.screens.len()],
            "minimum_new_screens":1, "requirements":["One timeline screen"]
        })
        .to_string(),
    );
    let mut protected = original.screens[1].clone();
    protected.html = "<h1>Unrequested change to saved screen</h1>".into();
    fake.push_text(json!({"screens":[{"index":1,"screen":protected}]}).to_string());
    added.html = "<h1>Verified timeline content</h1>".into();
    fake.push_text(
        json!({"screens":[{"index":original.screens.len(),"screen":added}]}).to_string(),
    );
    candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect("the verified draft repair should save");
    let saved = candidate
        .designs
        .load(id)
        .await
        .expect("candidate lookup should succeed")
        .expect("the same candidate should still exist");
    assert_eq!(
        &saved.screens[..original.screens.len()],
        &draft.screens[..original.screens.len()]
    );
    assert_eq!(saved.screens.last(), Some(&added));
    assert!(
        candidate
            .designs
            .load_review_draft(id)
            .await
            .expect("draft lookup should succeed")
            .is_none()
    );
    assert!(fake.requests()[1].to_string().contains("Timeline"));
    let repair_request = fake.requests()[2].to_string();
    assert!(repair_request.contains("Unique added timeline draft"));
    assert!(!repair_request.contains("Completed launch changes in the retained draft"));
    assert!(repair_request.contains(&format!(
        "Editable existing screen indexes (zero-based): [0, {}]",
        original.screens.len()
    )));
    let retry = fake.requests()[3].to_string();
    assert!(retry.contains("Repair draft cannot replace protected screen 2"));
    assert_eq!(
        fake.requests().len(),
        4,
        "verified drafts do not need a separate model review"
    );
}

#[tokio::test]
async fn repair_reports_a_protected_target_before_requesting_a_patch() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    let candidate = ReviewingCandidate::new(&fake);
    candidate.seed(&original, "Improve screen 1.").await;
    let id = "talk-candidate-1";
    let mut draft = original.clone();
    draft.screens[0].notes = Some("Unverified change".into());
    candidate
        .designs
        .keep_review_draft(id, &draft, Some(&original))
        .await
        .expect("draft should checkpoint");
    let mut repair = ChatMessage::user("Repair screen 2 in this draft.", Some(id));
    repair.is_review_draft = true;
    candidate
        .sessions
        .append_message("talk", repair)
        .await
        .expect("repair turn should be recorded");
    fake.push_text(r#"{"unsupported_requirements":[]}"#.into());
    fake.push_text(r#"{"instruction":"Repair screen 2","screens":[1],"minimum_new_screens":0,"requirements":["Repair screen 2"]}"#.into());
    let error = candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect_err("protected target must stop before writing");
    assert!(error.contains("screen 2") && error.contains("unchanged in this draft"));
    assert_eq!(fake.requests().len(), 2);
    let kept = candidate
        .designs
        .load_review_draft(id)
        .await
        .expect("draft lookup should succeed");
    assert_eq!(kept, Some(draft));
    let saved = candidate
        .designs
        .load(id)
        .await
        .expect("saved lookup should succeed");
    assert_eq!(saved, Some(original));
}

#[tokio::test]
async fn an_unavailable_layout_audit_preserves_an_unverified_draft_not_a_success() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    let mut candidate = ReviewingCandidate::new(&fake);
    let calls = Arc::new(AtomicUsize::new(0));
    let runner: DemoDomRunner = Arc::new(move |design, _| {
        let result = if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let coverage: Vec<_> = (0..design.screens.len()).collect();
            Ok(format!(
                "<html data-swift-design-findings=\"[]\" data-swift-design-audited-screens=\"{}\">",
                json!(coverage)
            ))
        } else {
            Err("Browser audit unavailable; retry when Chrome is ready".into())
        };
        Box::pin(async move { result })
    });
    candidate.engine = candidate.engine.with_demo_dom_runner(runner);
    candidate.seed(&original, "Update screen 1.").await;
    let before = tokio::fs::read(candidate.path())
        .await
        .expect("test data should be readable");
    fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
    fake.push_text(
        json!({"instruction":"Update screen 1", "screens":[0], "minimum_new_screens":0,
        "requirements":["Updated design"]})
        .to_string(),
    );
    let mut edited = original.screens[0].clone();
    edited.html = "<h1>Draft for the original candidate</h1>".into();
    fake.push_text(json!({"screens":[{"index":0,"screen":edited}]}).to_string());
    let error = candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect_err("unavailable layout audit should prevent verified completion");
    assert!(error.contains("Needs review"));
    assert!(error.contains("Browser audit unavailable"));
    assert!(!error.contains("Rendered layout verified"));
    assert_eq!(
        tokio::fs::read(candidate.path())
            .await
            .expect("test data should be readable"),
        before
    );
    assert!(
        candidate
            .designs
            .load("talk-candidate-2")
            .await
            .expect("candidate lookup should succeed")
            .is_none()
    );
    let draft = candidate
        .designs
        .load_review_draft("talk-candidate-1")
        .await
        .expect("draft lookup should succeed")
        .expect("same-candidate review draft should exist");
    assert_eq!(draft.screens[0], edited);
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn stopping_during_layout_review_keeps_the_completed_edit_on_its_candidate() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    let mut candidate = ReviewingCandidate::new(&fake);
    let reviewing = Arc::new(tokio::sync::Notify::new());
    let calls = AtomicUsize::new(0);
    let on_review = Arc::clone(&reviewing);
    let runner: DemoDomRunner = Arc::new(move |design, _| {
        let after_edit = calls.fetch_add(1, Ordering::SeqCst) > 0;
        let on_review = Arc::clone(&on_review);
        Box::pin(async move {
            if after_edit {
                on_review.notify_one();
                return std::future::pending().await;
            }
            let coverage: Vec<_> = (0..design.screens.len()).collect();
            Ok(format!(
                "<html data-swift-design-findings=\"[]\" data-swift-design-audited-screens=\"{}\">",
                json!(coverage)
            ))
        })
    });
    candidate.engine = candidate.engine.with_demo_dom_runner(runner);
    candidate.seed(&original, "Update only screen 1.").await;
    fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
    fake.push_text(json!({"instruction":"Update screen 1", "screens":[0], "minimum_new_screens":0, "requirements":["Updated design"]}).to_string());
    let mut edited = original.screens[0].clone();
    edited.html = "<h1>Keep the draft after stopping</h1>".into();
    fake.push_text(json!({"screens":[{"index":0,"screen":edited}]}).to_string());
    let engine = candidate.engine.clone();
    let task = tokio::spawn(async move { engine.run("talk", log_sink()).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), reviewing.notified())
        .await
        .expect("the completed draft should reach layout review");
    task.abort();
    assert!(
        task.await
            .expect_err("the test should cancel the review")
            .is_cancelled()
    );
    assert_eq!(
        candidate
            .designs
            .load("talk-candidate-1")
            .await
            .expect("original should load"),
        Some(original)
    );
    let draft = candidate
        .designs
        .load_review_draft("talk-candidate-1")
        .await
        .expect("draft should load")
        .expect("cancellation should retain the checkpoint");
    assert_eq!(draft.screens[0], edited);
    assert!(
        candidate
            .designs
            .load("talk-candidate-2")
            .await
            .expect("candidate lookup should succeed")
            .is_none()
    );
}

#[tokio::test]
async fn annotation_edit_sends_two_marked_images_and_rejects_an_out_of_scope_patch() {
    if crate::screenshots::find_chrome().is_none() {
        return;
    }
    use crate::design::annotations::AnnotationSubmission;
    let mut original = sample_design();
    original.theme.fonts.heading = "Arial".into();
    original.theme.fonts.body = "Arial".into();
    original.theme.fonts.mono = "Courier New".into();
    let fake = ParallelFakeModel::start(None).await;
    let mut candidate = ReviewingCandidate::new(&fake);
    candidate.engine = GenerationEngine::new(
        ModelConfiguration {
            model: "gpt-6-test".into(),
            ..fake.configuration()
        },
        candidate.designs.clone(),
        candidate.sessions.clone(),
        None,
        "http://127.0.0.1:3000".into(),
        ChangeNotifier::new(),
    )
    .with_demo_dom_runner(clean_demo_dom_runner());
    candidate.seed(&original, "Update the prototype.").await;
    let input: AnnotationSubmission = serde_json::from_value(json!({
        "viewport":original.viewport,"theme":original.theme,"app_window":original.app_window,
        "expected":{"0":original.screens[0],"2":original.screens[2]},
        "marks":[{"screen":0,"bounds":[1000,2000,3000,4000],"comment":"Raise the title"},
                 {"screen":2,"bounds":[5000,6000,7000,8000],"comment":"Tighten the summary"}]
    }))
    .expect("the editor payload should deserialize");
    let batch = input
        .bind(&original)
        .expect("valid marked screens should bind");
    let mut message = ChatMessage::user("Update the marked regions.", Some("talk-candidate-1"));
    message.annotations = Some(batch);
    candidate
        .sessions
        .append_message("talk", message)
        .await
        .expect("annotation turn should be recorded");
    fake.push_text(r#"{"unsupported_requirements":[]}"#.into());
    let mut protected = original.screens[1].clone();
    protected.notes = Some("Must never be saved".into());
    fake.push_text(json!({"screens":[{"index":1,"screen":protected}]}).to_string());
    let mut first = original.screens[0].clone();
    first.css = Some("h1 { margin-top: 0; }".into());
    let mut third = original.screens[2].clone();
    third.css = Some("p { line-height: 1.2; }".into());
    fake.push_text(
        json!({"screens":[{"index":0,"screen":first},{"index":2,"screen":third}]}).to_string(),
    );
    candidate
        .engine
        .run("talk", log_sink())
        .await
        .expect("scoped annotation edit should save");
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .expect("saved demo should load")
        .expect("candidate should exist");
    assert_eq!(saved.screens, [first, original.screens[1].clone(), third]);
    assert_eq!(saved.theme, original.theme);
    let requests = fake.requests();
    assert_eq!(requests.len(), 3);
    let parts = requests[1]["messages"][1]["content"]
        .as_array()
        .expect("writer should receive text and two rendered PNGs");
    assert_eq!(
        parts
            .iter()
            .filter(|part| part["image_url"]["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("data:image/png;base64,iVBORw0KGgo")))
            .count(),
        2
    );
    let text = parts[0]["text"]
        .as_str()
        .expect("writer should receive the exact screen data and notes");
    assert!(text.contains(
        "Box 1 on screen 1 (left 10.00%, top 20.00%, right 30.00%, bottom 40.00%): Raise the title"
    ));
    assert!(text.contains("Box 2 on screen 3") && text.contains("Tighten the summary"));
    assert!(
        requests[2]
            .to_string()
            .contains("replace only the specified screens")
    );
}

#[tokio::test]
async fn usable_edits_replace_the_current_snapshot_but_unusable_edits_do_not() {
    for kind in [
        "alignment",
        "alignment_unmeasured",
        "minor_overflow",
        "contrast",
        "overlap",
        "overflow",
        "empty",
        "broken_interaction",
    ] {
        let original = sample_design();
        let fake = ParallelFakeModel::start(None).await;
        let mut candidate = ReviewingCandidate::new(&fake);
        let runner: DemoDomRunner = Arc::new(move |design, _| {
            if kind == "overlap" && !design.screens[0].html.contains("Useful updated design") {
                return Box::pin(async { Err("Starting layout could not be measured".into()) });
            }
            let coverage: Vec<_> = (0..design.screens.len()).collect();
            let (findings, probes) = if kind == "broken_interaction" {
                (
                    json!([]),
                    json!([{"screen":0,"group":"menu","clicks":1,"keys":1,"focus":1,"failures":1,"issues":["Menu cannot open"]}]),
                )
            } else {
                (
                    json!([
                        {"screen":0,"kind":kind,"node":"div.metrics","detail":"Review the measured group"},
                        {"screen":1,"kind":"contrast","node":"p","detail":"Existing low contrast"}
                    ]),
                    json!([]),
                )
            };
            let dom = format!(
                "<html data-swift-design-findings=\"{}\" data-swift-design-audited-screens=\"{}\" data-swift-design-prototype-checks=\"{}\">",
                findings.to_string().replace('"', "&quot;"),
                json!(coverage),
                probes.to_string().replace('"', "&quot;")
            );
            Box::pin(async move { Ok(dom) })
        });
        candidate.engine = candidate.engine.with_demo_dom_runner(runner);
        candidate.seed(&original, "Update only screen 1.").await;
        let before = tokio::fs::read(candidate.path())
            .await
            .expect("test data should be readable");
        fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
        fake.push_text(
            json!({"instruction":"Update screen 1", "screens":[0],
            "minimum_new_screens":0, "requirements":["Updated screen"]})
            .to_string(),
        );
        let mut screen = original.screens[0].clone();
        screen.html = "<h1>Useful updated design</h1>".into();
        fake.push_text(json!({"screens":[{"index":0,"screen":screen}]}).to_string());
        let is_unusable = matches!(kind, "empty" | "broken_interaction");
        if is_unusable {
            for _ in 0..10 {
                fake.push_text(r#"{"screens":[]}"#.into());
            }
        }
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        let messages = candidate
            .sessions
            .messages("talk")
            .await
            .expect("test chat messages should be readable");
        if is_unusable {
            let error = result.expect_err("unusable edits should not replace the current snapshot");
            assert!(error.contains("10-correction-round limit"), "{error}");
            assert!(error.contains("Needs review"), "{error}");
            assert!(error.contains("talk-candidate-1"));
            assert!(
                candidate
                    .designs
                    .load("talk-candidate-2")
                    .await
                    .expect("candidate lookup should succeed")
                    .is_none()
            );
            assert_eq!(
                tokio::fs::read(candidate.path())
                    .await
                    .expect("test data should be readable"),
                before
            );
            assert!(
                candidate
                    .designs
                    .history("talk-candidate-1")
                    .await
                    .expect("test history should be listed")
                    .is_empty()
            );
            let draft = candidate
                .designs
                .load_review_draft("talk-candidate-1")
                .await
                .expect("test draft should load successfully")
                .expect("retained test draft should exist");
            assert_eq!(draft.screens[0], screen);
            assert_eq!(draft.screens[1..], original.screens[1..]);
            assert!(messages.iter().any(|message| {
                message
                    .content
                    .contains("requirement completion are not verified")
                    && message.artifacts == ["talk-candidate-1"]
            }));
        } else {
            assert!(result.is_ok(), "{result:?}");
            let saved = candidate
                .designs
                .load("talk-candidate-1")
                .await
                .expect("test record should load successfully")
                .expect("saved test record should exist");
            assert_eq!(saved.screens[0], screen);
            assert_eq!(saved.screens[1..], original.screens[1..]);
            let report = &messages
                .last()
                .expect("test collection should contain a last item")
                .content;
            assert!(
                report
                    .contains("Layout warnings on the edited screens (the preview may look fine)")
            );
            assert!(
                report.contains(&format!("Screen 1 ({}):", screen.name)),
                "{report}"
            );
            assert!(!report.contains("Screen 2 ("), "{report}");
            assert!(!report.contains("screens[0]"), "{report}");
            assert!(!report.contains("Retry edit"), "{report}");
            assert!(
                report.contains("were not independently verified"),
                "{report}"
            );
            if kind == "overlap" {
                assert!(report.contains("text may overlap"), "{report}");
            }
            let snapshots = candidate
                .designs
                .history("talk-candidate-1")
                .await
                .expect("the existing snapshot history should be readable");
            assert_eq!(snapshots.len(), 1);
            let restored = candidate
                .designs
                .restore("talk-candidate-1", &snapshots[0].stamp)
                .await
                .expect("the previous snapshot should be restorable")
                .expect("the previous snapshot should exist");
            assert_eq!(restored, original);
            assert!(!report.contains("Rendered layout verified"));
            assert!(
                candidate
                    .designs
                    .load_review_draft("talk-candidate-1")
                    .await
                    .expect("draft lookup should succeed")
                    .is_none()
            );
            assert!(
                candidate
                    .designs
                    .load("talk-candidate-2")
                    .await
                    .expect("test record should load successfully")
                    .is_none()
            );
        }
        assert_eq!(
            fake.requests().len(),
            if is_unusable { 13 } else { 3 },
            "{kind}"
        );
    }
}
