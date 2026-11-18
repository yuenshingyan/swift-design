use super::*;

#[test]
fn guard_rejects_malformed_or_reversed_json_delimiters_without_panicking() {
    for content in ["no JSON", "} before {", "{not JSON}"] {
        assert!(guard_page_patch(content, ArtifactKind::Demo, &[0]).is_err());
    }
    assert!(guard_page_patch("{}", ArtifactKind::Demo, &[0]).is_ok());
}

#[test]
fn worker_and_coordination_prompts_preserve_attachment_parts() {
    let original = sample_design();
    let snapshot = serde_json::to_value(&original).expect("test value should convert to JSON");
    let artifact_json =
        serde_json::to_string(&original).expect("test value should serialize to JSON");
    let note = "Old focused edit note.";
    let input = edit_input(&artifact_json, "Update the chosen screens.", note);
    let messages = vec![json!({ "role": "user", "content": [
        { "type": "text", "text": format!("{artifact_json}\n{note}") },
        { "type": "image_url", "image_url": { "url": "image boundary" } },
        { "type": "text", "text": "User's uploaded requirements" }
    ] })];
    let request = request_for(&original);
    let edit = EditExecution {
        original: &original,
        messages: messages.clone(),
        input: &input,
        indexes: &[0, 2],
        is_fresh: false,
        request: &request,
    };
    let worker = page_messages(&edit, &snapshot, 0)
        .expect("test page worker messages should be constructed");
    let coordinator = coordination_messages(&edit, &original)
        .expect("test coordination messages should be constructed");
    for rewritten in [&worker, &coordinator] {
        assert_eq!(rewritten[0]["content"][1], messages[0]["content"][1]);
        assert_eq!(rewritten[0]["content"][2], messages[0]["content"][2]);
        assert!(
            !rewritten[0]["content"][0]["text"]
                .as_str()
                .expect("test JSON value should be a string")
                .contains(note)
        );
    }
    assert!(
        request_text(&json!({"messages": worker})).contains("Assigned unit index (zero-based): 0")
    );
    assert!(
        request_text(&json!({"messages": coordinator})).contains("Coordinate the parallel edits.")
    );
}

#[test]
fn guard_rejects_shared_meta_changes_for_scoped_patch() {
    let original: Value =
        serde_json::from_str(SAMPLE_DOCUMENT).expect("test JSON should deserialize");
    let mut patch: Value =
        serde_json::from_str(&patch_for(ArtifactKind::Document, &original, 1, "ok"))
            .expect("test JSON should deserialize");
    patch["title"] = json!("Changed title");
    let error = guard_page_patch(&patch.to_string(), ArtifactKind::Document, &[1])
        .expect_err("scoped patch changing shared metadata should be rejected");
    assert!(error.contains("shared") || error.contains("title"));
}

#[test]
fn a_page_worker_cannot_change_shared_app_window_bounds() {
    let patch = json!({"app_window": {"left": 80, "top": 60, "width": 1280, "height": 780}});
    assert!(guard_page_patch(&patch.to_string(), ArtifactKind::Demo, &[0]).is_err());
}

#[test]
fn guard_rejects_cross_unit_changes_for_scoped_patch() {
    let original: Value =
        serde_json::from_str(SAMPLE_SOCIAL).expect("test JSON should deserialize");
    let patch = patch_for(ArtifactKind::Social, &original, 0, "wrong frame");
    let error = guard_page_patch(&patch, ArtifactKind::Social, &[1])
        .expect_err("scoped patch changing another unit should be rejected");
    assert!(error.contains("assigned") || error.contains("other units"));
}

#[test]
fn guard_rejects_insert_delete_and_duplicate_operations() {
    let original: Value = serde_json::from_str(SAMPLE_PRINT).expect("test JSON should deserialize");
    let insert_error = guard_page_patch(
        &insert_patch_for(ArtifactKind::Print, &original, 0),
        ArtifactKind::Print,
        &[0],
    )
    .expect_err("scoped patch inserting, deleting, or duplicating units should be rejected");
    assert!(insert_error.contains("insert"));

    let delete_error = guard_page_patch(
        &delete_patch_for(ArtifactKind::Print, 0),
        ArtifactKind::Print,
        &[0],
    )
    .expect_err("scoped patch inserting, deleting, or duplicating units should be rejected");
    assert!(delete_error.contains("delete") || delete_error.contains("null"));

    let duplicate_error = guard_page_patch(
        &duplicate_patch_for(ArtifactKind::Print, &original, 0),
        ArtifactKind::Print,
        &[0],
    )
    .expect_err("scoped patch inserting, deleting, or duplicating units should be rejected");
    assert!(duplicate_error.contains("assigned") || duplicate_error.contains("at most once"));
}

#[test]
fn focused_worker_json_shows_one_index_and_omits_markup_for_regeneration() {
    let original: Value = serde_json::from_str(SAMPLE_DECK).expect("test JSON should deserialize");
    let focused = focused_worker_json(&original, ArtifactKind::Deck, 2, false)
        .expect("worker fixture should produce focused JSON");
    let focused_json: Value = serde_json::from_str(&focused).expect("test JSON should deserialize");
    assert_eq!(
        focused_json["slides"]
            .as_array()
            .expect("test JSON value should be an array")
            .len(),
        1
    );
    assert_eq!(focused_json["slides"][0]["index"], json!(2));
    assert!(focused.contains("html"));
    assert!(focused.contains("css"));

    let fresh = focused_worker_json(&original, ArtifactKind::Deck, 2, true)
        .expect("worker fixture should produce focused JSON");
    assert!(fresh.contains("index"));
    assert!(!fresh.contains("<"));
    assert!(!fresh.contains(".slide"));
}

#[test]
fn all_kinds_use_their_patch_vocabulary_with_actual_fixtures() {
    let cases = [
        (
            ArtifactKind::Demo,
            serde_json::from_str::<Value>(SAMPLE_DESIGN).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Deck,
            serde_json::from_str::<Value>(SAMPLE_DECK).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Document,
            serde_json::from_str::<Value>(SAMPLE_DOCUMENT).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Social,
            serde_json::from_str::<Value>(SAMPLE_SOCIAL).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Print,
            serde_json::from_str::<Value>(SAMPLE_PRINT).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Mailing,
            serde_json::from_str::<Value>(SAMPLE_MAILING).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Campaign,
            serde_json::from_str::<Value>(SAMPLE_CAMPAIGN).expect("test JSON should deserialize"),
        ),
        (
            ArtifactKind::Artwork,
            serde_json::from_str::<Value>(SAMPLE_ARTWORK).expect("test JSON should deserialize"),
        ),
    ];

    for (kind, artifact) in cases {
        let focused = focused_worker_json(&artifact, kind, 0, false)
            .expect("worker fixture should produce focused JSON");
        let focused_json: Value =
            serde_json::from_str(&focused).expect("test JSON should deserialize");
        assert!(focused_json.get(unit_key(kind)).is_some(), "{kind:?}");
        assert_eq!(
            focused_json[unit_key(kind)]
                .as_array()
                .expect("test JSON value should be an array")
                .len(),
            1,
            "{kind:?}"
        );
        assert_eq!(
            focused_json[unit_key(kind)][0]["index"],
            json!(0),
            "{kind:?}"
        );

        let patch = patch_for(kind, &artifact, 0, "changed");
        guard_page_patch(&patch, kind, &[0])
            .expect("test patch should stay within the assigned page scope");
        assert!(patch.contains(unit_key(kind)), "{kind:?}");
        assert!(patch.contains(singular_key(kind)), "{kind:?}");
    }
}

#[test]
fn merge_worker_results_preserves_original_order_from_out_of_order_workers() {
    let original = sample_document();
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let second = <design_model::Document as ParallelEditable>::apply_edit(
        &original,
        &patch_for(ArtifactKind::Document, &original_json, 1, "second first"),
    )
    .expect("test edit should apply to its baseline");
    let first = <design_model::Document as ParallelEditable>::apply_edit(
        &original,
        &patch_for(ArtifactKind::Document, &original_json, 0, "first second"),
    )
    .expect("test edit should apply to its baseline");

    let merged = merge_worker_results(original.clone(), vec![(1, second), (0, first)])
        .expect("test worker results should merge without conflicts");
    assert_eq!(merged.pages[0].notes.as_deref(), Some("first second"));
    assert_eq!(merged.pages[1].notes.as_deref(), Some("second first"));
    assert_eq!(merged.pages[2], original.pages[2]);
}

#[tokio::test]
async fn multi_unit_edit_runs_workers_before_one_serial_coordination_call() {
    let original = sample_document();
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let fake = ParallelFakeModel::start(None).await;
    fake.auto_worker_patches(ArtifactKind::Document, &original_json, "worker");
    fake.push_text(patch_for(
        ArtifactKind::Document,
        &original_json,
        0,
        "coordinated zero",
    ));
    let engine = test_engine(fake.configuration());

    let edited = succeeded(request_edit_for(&engine, &original, &[1, 0], false).await);

    assert_eq!(edited.pages[0].notes.as_deref(), Some("coordinated zero"));
    assert_eq!(fake.state.worker_count.load(Ordering::SeqCst), 2);
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 1);
    let requests = fake.requests();
    assert!(request_text(&requests[0]).contains("Assigned unit index (zero-based):"));
    assert!(request_text(&requests[1]).contains("Assigned unit index (zero-based):"));
    assert!(request_text(&requests[2]).contains("Coordinate the parallel edits."));
}

#[tokio::test]
async fn parallel_workers_are_limited_before_coordination() {
    let mut original = sample_design();
    while original.screens.len() < 5 {
        original.screens.push(original.screens[0].clone());
    }
    let original_json = serde_json::to_value(&original).expect("test value should convert to JSON");
    let barrier = Arc::new(Barrier::new(5));
    let fake = ParallelFakeModel::start(Some(Arc::clone(&barrier))).await;
    fake.auto_worker_patches(ArtifactKind::Demo, &original_json, "worker");
    fake.push_text(patch_for(
        ArtifactKind::Demo,
        &original_json,
        0,
        "coordinator",
    ));
    let engine = test_engine(fake.configuration());
    let edit =
        tokio::spawn(
            async move { request_edit_for(&engine, &original, &[0, 1, 2, 3, 4], false).await },
        );

    fake.wait_for_workers(4).await;
    assert_eq!(fake.state.worker_count.load(Ordering::SeqCst), 4);
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        fake.state.active_workers.load(Ordering::SeqCst),
        PARALLEL_EDIT_LIMIT
    );
    barrier.wait().await;
    let _ = succeeded(
        tokio::time::timeout(Duration::from_secs(2), edit)
            .await
            .expect("test operation should complete before the timeout")
            .expect("test operation should succeed within the timeout"),
    );

    assert_eq!(
        fake.state.max_active_workers.load(Ordering::SeqCst),
        PARALLEL_EDIT_LIMIT
    );
}

#[tokio::test]
async fn a_systemic_edit_saves_only_after_coordination_and_reports_the_final_changes() {
    let mut original = sample_design();
    while original.screens.len() < 5 {
        original.screens.push(original.screens[0].clone());
    }
    let barrier = Arc::new(Barrier::new(PARALLEL_EDIT_LIMIT + 1));
    let fake = ParallelFakeModel::start(Some(Arc::clone(&barrier))).await;
    fake.push_text(
        r#"{"reply":"Editing the workspace.","edit":true,"unsupported_requirements":[]}"#
            .to_owned(),
    );
    fake.push_text(EDIT_PLAN.to_owned());
    fake.auto_worker_patches(
        ArtifactKind::Demo,
        &serde_json::to_value(&original).expect("test value should convert to JSON"),
        "worker",
    );
    fake.push_text(r#"{"title":"Coordinated workspace","screens":[]}"#.to_owned());
    fake.push_text(
        r#"{"coverage":[{"complete":true,"reason":"Screens updated"}],"reference_error":null}"#
            .to_owned(),
    );
    let candidate = ReviewingCandidate::new(&fake);
    candidate.seed(&original, "Update all screens.").await;
    fake.watch_baseline(&candidate.path()).await;
    let engine = candidate.engine.clone();
    let run = tokio::spawn(async move { engine.run("talk", log_sink()).await });
    fake.wait_for_workers(PARALLEL_EDIT_LIMIT).await;
    let mut pending = ChatMessage::user("QUEUED_ONLY_CHANGE", Some("talk-candidate-1"));
    pending.is_queued = true;
    candidate
        .sessions
        .append_message("talk", pending)
        .await
        .expect("test chat message should be appended");
    assert_eq!(
        candidate
            .designs
            .load("talk-candidate-1")
            .await
            .expect("test record should load successfully")
            .expect("saved test record should exist"),
        original
    );
    assert!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .expect("test history should be listed")
            .is_empty()
    );
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 0);
    barrier.wait().await;
    let outcome = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("test operation should complete before the timeout")
        .expect("test operation should succeed within the timeout")
        .expect("test result should contain the expected value");
    let GenerationOutcome::Edited { reports, .. } = outcome else {
        panic!("expected an edit report");
    };
    let saved = candidate
        .designs
        .load("talk-candidate-1")
        .await
        .expect("test record should load successfully")
        .expect("saved test record should exist");
    assert_eq!(saved.title, "Coordinated workspace");
    for (index, screen) in saved.screens.iter().enumerate() {
        assert_eq!(
            screen.notes.as_deref(),
            Some(format!("worker {index}").as_str())
        );
        assert!(reports[0].contains(&format!("Updated notes only for screen {}", index + 1)));
    }
    assert!(reports[0].contains("Shared settings changed: title"));
    assert_eq!(
        candidate
            .designs
            .history("talk-candidate-1")
            .await
            .expect("test history should be listed")
            .len(),
        1
    );
    assert!(
        !fake
            .state
            .baseline_changed_during_requests
            .load(Ordering::SeqCst)
    );
    assert_eq!(fake.state.coordinator_count.load(Ordering::SeqCst), 1);
    let messages = candidate
        .sessions
        .messages("talk")
        .await
        .expect("test chat messages should be readable");
    assert!(
        candidate
            .sessions
            .has_queued_messages("talk")
            .await
            .expect("queued-message state should be readable")
    );
    assert!(
        fake.requests()
            .iter()
            .all(|body| !request_text(body).contains("QUEUED_ONLY_CHANGE"))
    );
    let report = messages
        .last()
        .expect("test collection should contain a last item");
    assert!(report.content.contains(&reports[0]));
    let history = candidate
        .designs
        .history("talk-candidate-1")
        .await
        .expect("test history should be listed");
    assert_eq!(
        report
            .snapshot_stamps
            .as_ref()
            .expect("optional test value should be present")["talk-candidate-1"],
        history[0].stamp
    );
    verify_queued_edit_baseline(&candidate, &fake, &saved).await;
}
