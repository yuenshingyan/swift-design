use super::*;

#[tokio::test]
async fn corrections_repair_latest_drafts_until_pass_stall_or_budget() {
    for (missing, expected_error) in [
        (vec![3, 2, 1, 0], None),
        (vec![4, 3, 2, 1], Some("three-correction-round limit")),
        (vec![2, 2], Some("coverage did not improve")),
        (vec![1, 2], Some("coverage did not improve")),
    ] {
        let fake = ParallelFakeModel::start(None).await;
        let original = sample_design();
        let candidate = ReviewingCandidate::new(&fake);
        candidate
            .seed(
                &original,
                "Redesign only screen 1; provide designs or component specifications for states.",
            )
            .await;
        fake.watch_baseline(&candidate.path()).await;
        fake.push_text(r#"{"edit":true}"#.into());
        fake.push_text(json!({
            "instruction": "Redesign screen 1 with visible controls and concrete state specifications.",
            "screens": [0], "minimum_new_screens": 0,
            "requirements": ["Rendered controls", "Responsive layouts", "State specifications", "Preserve navigation"]
        }).to_string());
        for (round, count) in missing.iter().enumerate() {
            let mut screen = original.screens[0].clone();
            screen.html = format!("<h1>Draft {round}</h1><p>Concrete state design</p>");
            let mut patch = json!({"screens": [{"index": 0, "screen": screen}]});
            if round == 0 {
                let mut theme = json!(original.theme);
                theme["colors"]["accent"] = json!("#8dd8bc");
                patch["theme"] = theme;
            }
            fake.push_text(patch.to_string());
            let coverage: Vec<_> = (0..4)
                .map(|index| {
                    json!({
                        "complete": index >= *count,
                        "reason": format!("Complete checklist item {index}; review round {round}")
                    })
                })
                .collect();
            fake.push_text(json!({"coverage": coverage, "reference_error": null}).to_string());
        }
        let before = tokio::fs::read(candidate.path()).await.unwrap();
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        if let Some(expected) = expected_error {
            let error = result.unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("remaining issues"));
            assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
            assert!(
                candidate
                    .designs
                    .history("talk-candidate-1")
                    .await
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert!(result.is_ok(), "{result:?}");
            let saved = candidate
                .designs
                .load("talk-candidate-1")
                .await
                .unwrap()
                .unwrap();
            assert!(saved.screens[0].html.contains("Draft 3"));
            assert_eq!(json!(saved.theme)["colors"]["accent"], "#8dd8bc");
            assert_eq!(saved.screens[1..], original.screens[1..]);
        }
        assert!(
            !fake
                .state
                .baseline_changed_during_requests
                .load(Ordering::SeqCst)
        );
        let requests = fake.requests();
        assert_eq!(requests.len(), 2 + missing.len() * 2);
        // Planning, writing, corrections, and reviews use the same boundary.
        for request in &requests[1..] {
            let text = request_text(request);
            assert!(text.contains("Demo acceptance contract"));
            assert!(text.contains("designs or component specifications"));
            assert!(text.contains("do not demand executable behavior"));
            assert!(text.contains("Do not replace requested high-fidelity desktop/mobile layouts"));
        }
        for round in 1..missing.len() {
            let edit = request_text(&requests[2 + round * 2]);
            assert!(edit.contains(&format!("Draft {}", round - 1)));
            assert!(edit.contains("Repair the supplied latest draft"));
            assert!(edit.contains("Complete checklist item"));
            let review = requests[3 + round * 2]["messages"][1]["content"]
                .as_str()
                .unwrap();
            let baseline = review
                .split_once("Before artifact:\n")
                .unwrap()
                .1
                .split_once("\nAfter artifact:\n")
                .unwrap()
                .0;
            assert_eq!(
                serde_json::from_str::<Value>(baseline).unwrap(),
                json!(original)
            );
        }
    }
}

#[tokio::test]
async fn correction_cannot_change_an_unselected_original_screen() {
    let fake = ParallelFakeModel::start(None).await;
    let original = sample_design();
    let candidate = ReviewingCandidate::new(&fake);
    candidate.seed(&original, "Only edit screen 1.").await;
    fake.push_text(r#"{"edit":true}"#.into());
    fake.push_text(
        json!({"instruction":"Only edit screen 1", "screens":[0],
        "minimum_new_screens":0, "requirements":["Visible retry action"]})
        .to_string(),
    );
    fake.push_text(json!({"screens":[{"index":0,"screen":original.screens[0]}]}).to_string());
    fake.push_text(
        json!({"coverage":[{"complete":false,"reason":"Draw the retry action"}]}).to_string(),
    );
    let mut unrelated = original.screens[1].clone();
    unrelated.html = "<h1>Unrequested change</h1>".into();
    fake.push_text(json!({"screens":[{"index":1,"screen":unrelated}]}).to_string());
    let before = tokio::fs::read(candidate.path()).await.unwrap();
    let error = candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .unwrap_err();
    assert!(error.contains("unselected screen changed"), "{error}");
    assert_eq!(fake.requests().len(), 5);
    assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
    assert!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unsupported_executable_requirements_stop_without_correction_or_save() {
    let fake = ParallelFakeModel::start(None).await;
    let original = sample_design();
    let candidate = ReviewingCandidate::new(&fake);
    candidate
        .seed(
            &original,
            "Require executable date validation, not a specification.",
        )
        .await;
    fake.push_text(r#"{"edit":true}"#.into());
    fake.push_text(
        json!({"instruction":"Executable date validation", "screens":[0],
        "minimum_new_screens":0, "requirements":["Executable date validation"]})
        .to_string(),
    );
    fake.push_text(json!({"screens":[{"index":0,"screen":original.screens[0]}]}).to_string());
    fake.push_text(
        json!({"coverage":[{"complete":false,"reason":"Renderer forbids editable date inputs"}],
        "unsupported_requirements":["Executable date validation"], "reference_error":null})
        .to_string(),
    );
    let before = tokio::fs::read(candidate.path()).await.unwrap();
    let error = candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .unwrap_err();
    assert!(error.contains("unsupported executable requirements"));
    assert!(error.contains("approve a design/specification alternative"));
    assert_eq!(fake.requests().len(), 4);
    assert_eq!(tokio::fs::read(candidate.path()).await.unwrap(), before);
    assert!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .unwrap()
            .is_empty()
    );
}
