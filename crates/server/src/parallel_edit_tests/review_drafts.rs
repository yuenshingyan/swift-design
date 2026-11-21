use design_model::Design;

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
    fake.push_text(r#"{"coverage":[{"complete":true,"reason":"The timeline is present"}]}"#.into());
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
    let requests = fake.requests();
    let review = requests[4]["messages"][1]["content"]
        .as_str()
        .expect("review should receive artifact evidence");
    let (browser, baselines) = review
        .split_once("\nRetained draft before repair:\n")
        .expect("review must receive the draft that the repair started from");
    let (retained, baselines) = baselines
        .split_once("\nBefore artifact:\n")
        .expect("review must also receive the saved version");
    let (accepted, proposed) = baselines
        .split_once("\nAfter artifact:\n")
        .expect("review must receive the proposed repair");
    assert!(browser.contains("Browser prototype results"));
    assert_eq!(
        serde_json::from_str::<Design>(retained).expect("valid retained artifact"),
        draft
    );
    assert_eq!(
        serde_json::from_str::<Design>(accepted).expect("valid saved artifact"),
        original
    );
    assert_eq!(
        serde_json::from_str::<Design>(proposed).expect("valid proposed artifact"),
        saved
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
            let coverage: Vec<_> = (0..design.screens.len()).collect();
            let (findings, probes) = if kind == "broken_interaction" {
                (
                    json!([]),
                    json!([{"screen":0,"group":"menu","clicks":1,"keys":1,"focus":1,"failures":1,"issues":["Menu cannot open"]}]),
                )
            } else {
                (
                    json!([{"screen":0,"kind":kind,"node":"div.metrics","detail":"Review the measured group"}]),
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
        let runs_polish = is_unusable || matches!(kind, "overlap" | "overflow");
        if runs_polish {
            fake.push_text(r#"{"screens":[]}"#.into());
        }
        if !is_unusable {
            fake.push_text(
                json!({"coverage":[{"complete":true,"reason":"Requested design present"}]})
                    .to_string(),
            );
        }
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        let messages = candidate
            .sessions
            .messages("talk")
            .await
            .expect("test chat messages should be readable");
        if is_unusable {
            let error = result.expect_err("unusable edits should not replace the current snapshot");
            assert!(
                error.contains("Cannot save an unusable demo edit"),
                "{error}"
            );
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
            assert!(report.contains("Layout warnings on the edited screens"));
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
            3 + usize::from(runs_polish) + usize::from(!is_unusable),
            "{kind}"
        );
    }
}
