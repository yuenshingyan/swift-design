use super::*;

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
    let before = tokio::fs::read(candidate.path()).await.unwrap();
    fake.push_text(r#"{"edit":true}"#.into());
    fake.push_text(
        json!({"instruction":"Update screen 1", "screens":[0], "minimum_new_screens":0,
        "requirements":["Updated design"]})
        .to_string(),
    );
    fake.push_text(json!({"screens":[{"index":0,"screen":original.screens[0]}]}).to_string());
    let error = candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .unwrap_err();
    assert!(error.contains("Needs review"));
    assert!(error.contains("Browser audit unavailable"));
    assert!(!error.contains("Rendered layout verified"));
    assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
    assert!(
        candidate
            .designs
            .load("talk-candidate-2")
            .await
            .unwrap()
            .unwrap()
            .title
            .starts_with("Needs review")
    );
    assert_eq!(fake.requests().len(), 3);
}

#[tokio::test]
async fn advisory_polish_saves_with_review_notes_but_serious_defects_preserve_a_separate_draft() {
    for kind in [
        "alignment",
        "alignment_unmeasured",
        "minor_overflow",
        "contrast",
        "overflow",
    ] {
        let original = sample_design();
        let fake = ParallelFakeModel::start(None).await;
        let mut candidate = ReviewingCandidate::new(&fake);
        let runner: DemoDomRunner = Arc::new(move |design, _| {
            let coverage: Vec<_> = (0..design.screens.len()).collect();
            let findings = json!([{"screen":0,"kind":kind,"node":"div.metrics","detail":"Review the measured group"}]);
            let dom = format!(
                "<html data-swift-design-findings=\"{}\" data-swift-design-audited-screens=\"{}\">",
                findings.to_string().replace('"', "&quot;"),
                json!(coverage)
            );
            Box::pin(async move { Ok(dom) })
        });
        candidate.engine = candidate.engine.with_demo_dom_runner(runner);
        candidate.seed(&original, "Update only screen 1.").await;
        let before = tokio::fs::read(candidate.path()).await.unwrap();
        fake.push_text(r#"{"edit":true}"#.into());
        fake.push_text(
            json!({"instruction":"Update screen 1", "screens":[0],
            "minimum_new_screens":0, "requirements":["Updated screen"]})
            .to_string(),
        );
        let mut screen = original.screens[0].clone();
        screen.html = "<h1>Useful updated design</h1>".into();
        fake.push_text(json!({"screens":[{"index":0,"screen":screen}]}).to_string());
        let serious = kind == "overflow";
        fake.push_text(if serious {
            "{}".into()
        } else {
            json!({"coverage":[{"complete":true,"reason":"Requested design present"}]}).to_string()
        });
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        let messages = candidate.sessions.messages("talk").await.unwrap();
        if serious {
            let error = result.unwrap_err();
            assert!(error.contains("Needs review"), "{error}");
            assert!(error.contains("talk-candidate-2"));
            assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
            assert!(
                candidate
                    .designs
                    .history("talk-candidate-1")
                    .await
                    .unwrap()
                    .is_empty()
            );
            let draft = candidate
                .designs
                .load("talk-candidate-2")
                .await
                .unwrap()
                .unwrap();
            assert!(draft.title.starts_with("Needs review"));
            assert_eq!(draft.screens[0], screen);
            assert_eq!(draft.screens[1..], original.screens[1..]);
            assert!(messages.iter().any(|message| {
                message
                    .content
                    .contains("requirement completion are not verified")
                    && message.artifacts == ["talk-candidate-2"]
            }));
        } else {
            assert!(result.is_ok(), "{result:?}");
            let saved = candidate
                .designs
                .load("talk-candidate-1")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(saved.screens[0], screen);
            assert_eq!(saved.screens[1..], original.screens[1..]);
            let report = &messages.last().unwrap().content;
            assert!(report.contains("Needs review — advisory layout findings"));
            assert!(!report.contains("Rendered layout verified"));
            assert!(
                candidate
                    .designs
                    .load("talk-candidate-2")
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(fake.requests().len(), 4);
    }
}
