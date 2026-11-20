use super::*;

#[tokio::test]
async fn theme_review_has_comparison_evidence_and_failure_reaches_follow_up_chat() {
    for accepted in [true, false] {
        let original = sample_design();
        let fake = ParallelFakeModel::start(None).await;
        fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
        fake.push_text(r#"{"instruction":"Use a light theme without changing content or navigation.","screens":[],"minimum_new_screens":0,"requirements":["Light theme","Preserve existing content and navigation","Leave other candidates unchanged"]}"#.into());
        fake.auto_worker_patches(ArtifactKind::Demo, &json!(original), "worker");
        let mut theme = json!(original.theme);
        theme["colors"]["background"] = json!("#ffffff");
        fake.push_text(json!({"theme": theme}).to_string());
        let findings = if accepted {
            vec![]
        } else {
            vec![json!({
                "id": "missing-navigation-link", "reason": "Restore the missing navigation link", "screens": []
            })]
        };
        let review = json!({"coverage": [
            {"complete": true},
            {"complete": accepted, "findings": findings},
            {"complete": true}
        ], "reference_error": null})
        .to_string();
        fake.push_text(review.clone());
        if !accepted {
            fake.push_text(json!({"theme": theme}).to_string());
            fake.push_text(review);
        }
        let candidate = ReviewingCandidate::new(&fake);
        candidate.seed(&original, "I want a light-theme.").await;
        candidate
            .designs
            .save("talk-candidate-2", &original)
            .await
            .expect("test data should be saved successfully");
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        assert_eq!(result.is_ok(), accepted, "{result:?}");
        assert_eq!(
            fake.state.worker_count.load(Ordering::SeqCst),
            original.screens.len() * if accepted { 1 } else { 2 }
        );
        let requests = fake.requests();
        let review = requests
            .last()
            .expect("test collection should contain a last item");
        let text = review["messages"][1]["content"]
            .as_str()
            .expect("test JSON value should be a string");
        assert!(text.contains("Pending save target (server-enforced): talk-candidate-1"));
        assert!(!text.contains("\nRetained draft before repair:\n"));
        let (before, after) = text
            .split_once("Before artifact:\n")
            .expect("test value should contain the expected separator")
            .1
            .split_once("\nAfter artifact:\n")
            .expect("test value should contain the expected separator");
        assert_eq!(
            serde_json::from_str::<Value>(before).expect("test JSON should deserialize"),
            json!(original)
        );
        let proposed: design_model::Design =
            serde_json::from_str(after).expect("test JSON should deserialize");
        assert_eq!(json!(proposed.theme), theme);
        assert_eq!(proposed.screens[0].html, original.screens[0].html);
        let saved = candidate
            .designs
            .load("talk-candidate-1")
            .await
            .expect("test record should load successfully")
            .expect("saved test record should exist");
        assert_eq!(saved, if accepted { proposed } else { original.clone() });
        assert_eq!(
            candidate
                .designs
                .load("talk-candidate-2")
                .await
                .expect("test record should load successfully")
                .expect("saved test record should exist"),
            original
        );
        assert!(
            candidate
                .designs
                .history("talk-candidate-2")
                .await
                .expect("test history should be listed")
                .is_empty()
        );
        if !accepted {
            let error = result.expect_err("failed theme review should reject the edit");
            assert!(error.contains("Preserve existing content and navigation"));
            assert!(
                candidate
                    .designs
                    .history("talk-candidate-1")
                    .await
                    .expect("test history should be listed")
                    .is_empty()
            );
            for event in [
                design_model::WorkflowEvent::RunFailed,
                design_model::WorkflowEvent::Recovered {
                    to: design_model::WorkflowState::Reviewing,
                },
            ] {
                candidate
                    .sessions
                    .apply("talk", event)
                    .await
                    .expect("test workflow transition should apply");
            }
            candidate
                .sessions
                .append_message("talk", ChatMessage::user("WTF?", Some("talk-candidate-1")))
                .await
                .expect("test chat message should be appended");
            fake.push_text(
                r#"{"reply":"The recorded preservation check rejected the edit.","unsupported_requirements":[]}"#.into(),
            );
            candidate
                .engine
                .clone()
                .run("talk", log_sink())
                .await
                .expect("test generation run should complete successfully");
            let requests = fake.requests();
            let follow_up = request_text(
                requests
                    .last()
                    .expect("test collection should contain a last item"),
            );
            assert!(follow_up.contains(&error));
            assert!(follow_up.contains("Recorded session run outcomes"));
        }
    }
}

#[tokio::test]
async fn extension_requires_additions_and_feature_coverage_before_saving() {
    const PASS: &str = r#"{"coverage":[{"complete":true,"reason":"Connected page present"}],"reference_error":null}"#;
    const FAIL: &str = r#"{"coverage":[{"complete":false,"findings":[{"id":"missing-notification-control","reason":"Add the notification control","screens":[]}]}],"reference_error":null}"#;
    for (adds_page, coverage, retry_coverage, should_save) in [
        (false, PASS, None, false),
        (true, FAIL, Some(FAIL), false),
        (true, FAIL, Some(PASS), true),
        (true, r#"{"coverage":[]}"#, None, false),
        (true, "not JSON", None, false),
        (
            true,
            r#"{"coverage":[],"reference_error":"Upload the missing screenshot"}"#,
            None,
            false,
        ),
        (true, PASS, None, true),
    ] {
        let original = sample_design();
        let fake = ParallelFakeModel::start(None).await;
        fake.push_text(r#"{"edit":true,"unsupported_requirements":[]}"#.into());
        fake.push_text(r#"{"instruction":"Extend candidate 4: modify Task details and add connected Notifications.","screens":[],"minimum_new_screens":1,"requirements":["Task discussion and connected Notifications"]}"#.into());
        fake.auto_worker_patches(
            ArtifactKind::Demo,
            &serde_json::to_value(&original).expect("test value should convert to JSON"),
            "worker",
        );
        let mut screen = original.screens[0].clone();
        screen.html = format!(
            "<a href='#screen-{}'>Notifications</a><p>Discussion</p>",
            original.screens.len() + 1
        );
        let mut operations = vec![json!({"index": 0, "screen": screen})];
        if adds_page {
            screen.name = "Notifications".into();
            screen.html = "<a href='#screen-1'>Task</a>".into();
            operations
                .push(json!({"index": original.screens.len(), "insert": true, "screen": screen}));
        }
        let mut worker_design = original.clone();
        if adds_page {
            worker_design.screens.push(
                serde_json::from_value(operations[1]["screen"].clone())
                    .expect("test JSON value should deserialize"),
            );
        }
        fake.auto_worker_patches(ArtifactKind::Demo, &json!(worker_design), "worker");
        fake.push_text(json!({"screens": operations}).to_string());
        fake.push_text(coverage.into());
        if let Some(retry) = retry_coverage {
            fake.push_text(json!({"screens": [operations[0].clone()]}).to_string());
            fake.push_text(retry.into());
        }
        let candidate = ReviewingCandidate::new(&fake);
        candidate
            .seed(
                &original,
                "Extend the demo with discussions and Notifications.",
            )
            .await;
        candidate
            .sessions
            .append_message(
                "talk",
                ChatMessage::user("Only do this in candidate 4.", Some("talk-candidate-1")),
            )
            .await
            .expect("test chat message should be appended");
        let before = tokio::fs::read(candidate.path())
            .await
            .expect("test data should be readable");
        let result = candidate.engine.clone().run("talk", log_sink()).await;
        assert_eq!(result.is_ok(), should_save, "{result:?}");
        let planning = request_text(&fake.requests()[1]);
        assert!(planning.contains("discussions and Notifications"));
        assert!(planning.contains("Only do this in candidate 4"));
        assert_eq!(
            fake.state.worker_count.load(Ordering::SeqCst),
            original.screens.len()
                + if retry_coverage.is_some() {
                    worker_design.screens.len()
                } else {
                    0
                }
        );
        assert_eq!(
            fake.state.coordinator_count.load(Ordering::SeqCst),
            if retry_coverage.is_some() { 2 } else { 1 }
        );
        if retry_coverage.is_some() {
            assert!(
                fake.requests()
                    .iter()
                    .any(|request| request_text(request).contains("Add the notification control"))
            );
        }
        if should_save {
            let saved = candidate
                .designs
                .load("talk-candidate-1")
                .await
                .expect("test record should load successfully")
                .expect("saved test record should exist");
            assert_eq!(saved.screens.len(), original.screens.len() + 1);
            assert!(saved.screens[0].html.contains("Discussion"));
        } else {
            let draft = candidate
                .designs
                .load_review_draft("talk-candidate-1")
                .await
                .expect("draft lookup should succeed");
            assert_eq!(
                draft.is_some(),
                adds_page,
                "only a scope-valid changed draft should survive an unsuccessful review"
            );
            if let Some(draft) = draft {
                assert_eq!(draft.screens.len(), original.screens.len() + 1);
            }
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
            assert!(
                !candidate
                    .sessions
                    .messages("talk")
                    .await
                    .expect("test chat messages should be readable")
                    .iter()
                    .any(|message| message.content.starts_with(EDIT_REPORT_PREFIX))
            );
        }
    }
}

pub(super) async fn verify_queued_edit_baseline(
    candidate: &ReviewingCandidate,
    fake: &ParallelFakeModel,
    baseline: &design_model::Design,
) {
    candidate
        .sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .expect("test workflow transition should apply");
    candidate
        .sessions
        .activate_next_message("talk")
        .await
        .expect("next queued test message should activate")
        .expect("queued test message should be activated");
    fake.watch_baseline(&candidate.path()).await;
    fake.push_text(
        r#"{"reply":"Applying the queued change.","edit":true,"unsupported_requirements":[]}"#
            .to_owned(),
    );
    fake.push_text(EDIT_PLAN.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(baseline).expect("test value should convert to JSON"),
        "queued worker",
    );
    fake.push_text(r#"{"title":"Queued workspace","screens":[]}"#.to_owned());
    fake.push_text(
        r#"{"coverage":[{"complete":true,"reason":"Screens updated"}],"reference_error":null}"#
            .to_owned(),
    );
    candidate
        .engine
        .clone()
        .run("talk", log_sink())
        .await
        .expect("test generation run should complete successfully");
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .expect("test record should load successfully")
        .expect("saved test record should exist");
    assert_eq!(saved.title, "Queued workspace");
    let messages = candidate
        .sessions
        .messages("talk")
        .await
        .expect("test chat messages should be readable");
    let stamp = &messages
        .last()
        .expect("test collection should contain a last item")
        .snapshot_stamps
        .as_ref()
        .expect("optional test value should be present")["talk-candidate-1"];
    let restored = candidate
        .designs
        .restore("talk-candidate-1", stamp)
        .await
        .expect("test snapshot should restore successfully")
        .expect("restored test snapshot should exist");
    assert_eq!(&restored, baseline);
}

#[tokio::test]
pub(super) async fn an_unrepaired_layout_after_parallel_editing_does_not_replace_the_saved_candidate()
 {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(
        r#"{"reply":"Editing selected screens.","edit":true,"unsupported_requirements":[]}"#
            .to_owned(),
    );
    fake.push_text(EDIT_PLAN.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(&original).expect("test value should convert to JSON"),
        "worker",
    );
    fake.push_text(r#"{"title":"Unverified layout","screens":[]}"#.to_owned());
    fake.push_text("{}".to_owned());
    let mut candidate = ReviewingCandidate::new(&fake);
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let coverage: Vec<usize> = (0..design.screens.len()).collect();
        let findings = if design.title == "Unverified layout" {
            "[{&quot;screen&quot;:0,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;clipped text&quot;}]"
        } else {
            "[]"
        };
        let dom = format!(
            "<html data-swift-design-findings=\"{findings}\" data-swift-design-audited-screens=\"{}\">",
            serde_json::to_string(&coverage).expect("test value should serialize to JSON")
        );
        Box::pin(async move { Ok(dom) })
    });
    candidate.engine = candidate.engine.with_demo_dom_runner(runner);
    candidate.seed(&original, "Update every screen.").await;
    let before = tokio::fs::read(candidate.path())
        .await
        .expect("test data should be readable");
    assert!(
        candidate
            .engine
            .clone()
            .run("talk", log_sink())
            .await
            .is_err()
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
    assert!(
        !candidate
            .sessions
            .messages("talk")
            .await
            .expect("test chat messages should be readable")
            .iter()
            .any(|message| message.content.starts_with("Saved edit report:"))
    );
}

#[tokio::test]
pub(super) async fn coordination_failure_leaves_the_real_saved_candidate_and_history_unchanged() {
    let original = sample_design();
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(
        r#"{"reply":"Editing selected screens.","edit":true,"unsupported_requirements":[]}"#
            .to_owned(),
    );
    fake.push_text(EDIT_PLAN.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(&original).expect("test value should convert to JSON"),
        "worker",
    );
    fake.push_text(r#"{"theme":null,"screens":[]}"#.to_owned());
    let candidate = ReviewingCandidate::new(&fake);
    candidate
        .seed(&original, "[screen 1] [screen 2] Update these screens.")
        .await;
    let before = tokio::fs::read(candidate.path())
        .await
        .expect("test data should be readable");
    fake.watch_baseline(&candidate.path()).await;
    assert!(
        candidate
            .engine
            .clone()
            .run("talk", log_sink())
            .await
            .is_err()
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
    assert!(
        !fake
            .state
            .baseline_changed_during_requests
            .load(Ordering::SeqCst)
    );
    let messages = candidate
        .sessions
        .messages("talk")
        .await
        .expect("test chat messages should be readable");
    assert!(
        !messages
            .iter()
            .any(|message| { message.content.starts_with(EDIT_REPORT_PREFIX) })
    );
}

#[tokio::test]
pub(super) async fn workers_read_identical_baseline_even_when_responses_are_out_of_order() {
    let original = sample_social();
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(ArtifactKind::Social, &original_json, "worker");
    fake.push_text(patch_for(
        ArtifactKind::Social,
        &original_json,
        0,
        "coordinated",
    ));
    let engine = test_engine(fake.configuration());

    succeeded(request_edit_for(&engine, &original, &[2, 0, 1], false).await);

    let worker_prompts: Vec<String> = fake
        .requests()
        .into_iter()
        .map(|request| request_text(&request))
        .filter(|prompt| prompt.contains("Assigned unit index (zero-based):"))
        .collect();
    assert_eq!(worker_prompts.len(), 3);
    for prompt in worker_prompts {
        assert!(prompt.contains(&original.title));
        assert!(prompt.contains(original.frames[0].notes.as_deref().unwrap_or("")));
        assert!(prompt.contains(original.frames[1].notes.as_deref().unwrap_or("")));
        assert!(prompt.contains(original.frames[2].notes.as_deref().unwrap_or("")));
    }
}

#[tokio::test]
pub(super) async fn worker_failure_prevents_coordination() {
    let original = sample_print();
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(patch_for(ArtifactKind::Print, &original_json, 0, "ok"));
    fake.push_text(insert_patch_for(ArtifactKind::Print, &original_json, 1));
    let engine = test_engine(fake.configuration());

    let result = request_edit_for(&engine, &original, &[0, 1], false).await;

    assert!(result.is_err());
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
pub(super) async fn single_unit_edit_uses_the_old_single_request_path() {
    let original = sample_campaign();
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let fake = ParallelFakeModel::start(None).await;
    fake.push_text(patch_for(
        ArtifactKind::Campaign,
        &original_json,
        0,
        "single",
    ));
    let engine = test_engine(fake.configuration());

    let edited = succeeded(request_edit_for(&engine, &original, &[0], false).await);

    assert_eq!(edited.ads[0].notes.as_deref(), Some("single"));
    assert_eq!(fake.state.worker_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
    assert_eq!(fake.requests().len(), 1);
    assert!(!request_text(&fake.requests()[0]).contains("Assigned unit index (zero-based):"));
}

#[tokio::test]
pub(super) async fn scoped_coordination_cannot_change_unselected_units_or_shared_values() {
    let original = sample_mailing();
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(ArtifactKind::Mailing, &original_json, "worker");
    let mut bad_coordination: Value = serde_json::from_str(&patch_for(
        ArtifactKind::Mailing,
        &original_json,
        0,
        "coordinator zero",
    ))
    .expect("test JSON should deserialize");
    bad_coordination["title"] = json!("Bad shared change");
    fake.push_text(bad_coordination.to_string());
    let engine = test_engine(fake.configuration());

    let result = request_edit_for(&engine, &original, &[0, 1], false).await;

    assert!(result.is_err());
    assert!(fake.state.coordinator_count.load(Ordering::SeqCst) >= 1);
}

pub(super) fn assert_fixture_edit<T: ParallelEditable>(original: T) {
    let baseline = serde_json::to_value(&original).expect("test value should convert to JSON");
    let edited = T::apply_edit(&original, &patch_for(T::KIND, &baseline, 0, "changed"))
        .expect("test edit should apply to its baseline");
    let mut returned = serde_json::to_value(edited).expect("test value should convert to JSON");
    returned["title"] = json!("Worker metadata must be ignored");
    let returned =
        serde_json::from_value::<T>(returned).expect("test JSON value should deserialize");
    let merged = merge_worker_results(original, vec![(0, returned)])
        .expect("test worker results should merge without conflicts");
    let merged = serde_json::to_value(merged).expect("test value should convert to JSON");
    let (_, plural) = unit_names(T::KIND);
    assert_eq!(merged[plural][0]["notes"], json!("changed"));
    assert_eq!(
        &merged[plural]
            .as_array()
            .expect("test JSON value should be an array")[1..],
        &baseline[plural]
            .as_array()
            .expect("test JSON value should be an array")[1..]
    );
    for (key, value) in baseline
        .as_object()
        .expect("test JSON value should be an object")
    {
        if key != plural {
            assert_eq!(&merged[key], value);
        }
    }
}

#[test]
pub(super) fn all_kinds_apply_fixture_patches_and_merge_only_the_assigned_units() {
    assert_fixture_edit(sample_design());
    assert_fixture_edit(sample_deck());
    assert_fixture_edit(sample_document());
    assert_fixture_edit(sample_social());
    assert_fixture_edit(sample_print());
    assert_fixture_edit(sample_mailing());
    assert_fixture_edit(sample_campaign());
    assert_fixture_edit(sample_artwork());
}

#[test]
pub(super) fn merging_rejects_duplicate_and_out_of_range_worker_results() {
    let original = sample_design();
    assert!(
        merge_worker_results(
            original.clone(),
            vec![(0, original.clone()), (0, original.clone())]
        )
        .is_err()
    );
    let mut shorter = original.clone();
    shorter.screens.pop();
    assert!(
        merge_worker_results(
            shorter.clone(),
            vec![(original.screens.len() - 1, original.clone())]
        )
        .is_err()
    );
    assert!(
        merge_worker_results(
            original.clone(),
            vec![(original.screens.len() - 1, shorter)]
        )
        .is_err()
    );
}

#[tokio::test]
pub(super) async fn regeneration_workers_do_not_receive_the_original_markup() {
    let original = sample_deck();
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(
        ArtifactKind::Deck,
        &serde_json::to_value(&original).expect("test value should convert to JSON"),
        "regenerated",
    );
    fake.push_text(r#"{"slides":[]}"#.to_owned());
    let engine = test_engine(fake.configuration());
    succeeded(request_edit_for(&engine, &original, &[0, 2], true).await);
    for body in fake.requests() {
        if !request_text(&body).contains("Assigned unit index (zero-based):") {
            continue;
        }
        let prompt = body["messages"][0]["content"]
            .as_str()
            .expect("test JSON value should be a string");
        let (_, artifact) = prompt
            .split_once("Here is the edit baseline:\n")
            .expect("test value should contain the expected separator");
        let (artifact, _) = artifact
            .split_once("\nApply the requested change.")
            .expect("test value should contain the expected separator");
        let focused: Value = serde_json::from_str(artifact).expect("test JSON should deserialize");
        assert_eq!(focused["slides"][0]["slide"]["html"], json!(""));
        assert!(focused["slides"][0]["slide"].get("css").is_none());
        assert!(prompt.contains("Write the assigned unit anew"));
    }
}
