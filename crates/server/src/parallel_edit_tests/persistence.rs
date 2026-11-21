use super::*;

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
    for _ in 0..10 {
        fake.push_text(r#"{"screens":[]}"#.into());
    }
    let mut candidate = ReviewingCandidate::new(&fake);
    let runner: DemoDomRunner = Arc::new(|design, _| {
        let coverage: Vec<usize> = (0..design.screens.len()).collect();
        let findings = if design.title == "Unverified layout" {
            "[{&quot;screen&quot;:0,&quot;kind&quot;:&quot;empty&quot;,&quot;detail&quot;:&quot;no visible content&quot;}]"
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
    for _ in 0..5 {
        fake.push_text(r#"{"theme":[],"screens":[]}"#.into());
    }
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
