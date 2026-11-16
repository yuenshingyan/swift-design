use super::*;

fn sample() -> Design {
    serde_json::from_str(include_str!("../../../../../fixtures/sample-design.json")).unwrap()
}

#[test]
fn partial_json_handles_quoted_braces_trailing_escapes_and_incomplete_unicode() {
    assert_eq!(
        prefix_json(r#"{"html":"Text { ["#).unwrap()["html"],
        "Text { ["
    );
    assert_eq!(prefix_json("{\"html\":\"Text\\").unwrap()["html"], "Text");
    assert!(prefix_json(r#"{"html":"\u12"#).is_none());
    assert!(prefix_json("{]").is_none());
    assert!(prefix_json("no JSON").is_none());
    assert!(prefix_json("{\"key\":").is_none());
}

#[test]
fn generation_preview_renders_before_the_first_html_string_finishes() {
    let mut design = sample();
    design.screens[0].html = "<div><h1>Growing now</h1></div>".into();
    let raw = serde_json::to_string(&design).unwrap();
    let end = raw.find("Growing now").unwrap() + "Growing now".len();
    let draft = preview_design(&raw[..end]).unwrap();
    assert_eq!(draft.screens.len(), 1);
    assert_eq!(draft.screens[0].html, "<div><h1>Growing now</h1></div>");
    assert!(draft.validate().is_empty());
    assert!(crate::generation::artifacts::parse_design(&raw[..end]).is_err());
    assert!(preview_design("{\"screens\":[{\"html\":\"<h1>Missing metadata").is_none());
}

#[test]
fn unsafe_middle_screens_are_not_removed_and_renumbered() {
    let mut design = sample();
    design.screens[0].html = "<script>unsafe()</script>".into();
    assert!(preview_design(&serde_json::to_string(&design).unwrap()).is_none());
}

#[test]
fn patch_preview_keeps_prior_corrections_and_untouched_screens() {
    let mut baseline = sample();
    baseline.screens[1].html = "<p>Previously corrected</p>".into();
    let draft = preview_patch(
        &baseline,
        r#"{"screens":[{"index":0,"screen":{"html":"<div>Writing"#,
    )
    .unwrap();
    assert_eq!(draft.screens[0].html, "<div>Writing</div>");
    assert_eq!(draft.screens[1], baseline.screens[1]);
    assert_eq!(draft.screens.len(), baseline.screens.len());
    assert!(draft.validate().is_empty());
    let unsafe_patch =
        r#"{"screens":[{"index":0,"screen":{"html":"<img src=\"x\" onerror=\"bad()\">"}}]}"#;
    assert_eq!(preview_patch(&baseline, unsafe_patch).unwrap(), baseline);
}

#[test]
fn worker_previews_merge_progress_and_ignore_other_indexes_and_shared_fields() {
    let original = sample();
    let seen = Arc::new(Mutex::new(Vec::<Design>::new()));
    let collected = Arc::clone(&seen);
    let sink: Arc<TextSink> = Arc::new(move |text| {
        collected
            .lock()
            .unwrap()
            .push(serde_json::from_str(text).unwrap());
    });
    let board = PreviewBoard::default();
    let first = worker_sink(original.clone(), 0, Arc::clone(&board), Arc::clone(&sink));
    let second = worker_sink(original.clone(), 1, Arc::clone(&board), Arc::clone(&sink));
    first(
        r#"{"title":"Unauthorized shared edit","screens":[{"index":0,"screen":{"html":"<h1>First"#,
    );
    second(r#"{"screens":[{"index":1,"screen":{"html":"<h1>Second"#);
    let results = seen.lock().unwrap();
    assert_eq!(results.len(), 2);
    let latest = results.last().unwrap();
    assert_eq!(latest.title, original.title);
    assert_eq!(latest.screens[0].html, "<h1>First</h1>");
    assert_eq!(latest.screens[1].html, "<h1>Second</h1>");
    drop(results);
    let outside = worker_sink(original, 2, board, sink);
    outside(r#"{"screens":[{"index":0,"screen":{"html":"<h1>Wrong index"#);
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn tweak_adapter_uses_latest_merged_design_not_saved_original() {
    let mut baseline = sample();
    baseline.screens[1].html = "<h1>Already edited</h1>".into();
    let seen = Arc::new(Mutex::new(Vec::<Design>::new()));
    let collected = Arc::clone(&seen);
    let sink = patch_sink(
        &baseline,
        Arc::new(move |text| {
            collected
                .lock()
                .unwrap()
                .push(serde_json::from_str(text).unwrap());
        }),
    );
    sink(r#"{"screens":[{"index":0,"screen":{"html":"<div>Tweaking"#);
    let designs = seen.lock().unwrap();
    assert_eq!(designs[0].screens[1], baseline.screens[1]);
    assert_eq!(designs[0].screens[0].html, "<div>Tweaking</div>");
}

#[test]
fn continuation_shows_only_the_requested_sequential_insertions() {
    let screens = preview_continuation_screens(
        2,
        r#"{"screens":[{"index":2,"insert":true,"screen":{"html":"<h1>New screen"#,
    )
    .unwrap();
    assert_eq!(screens.len(), 1);
    assert_eq!(screens[0].html, "<h1>New screen</h1>");
    assert!(
        preview_continuation_screens(
            2,
            r#"{"screens":[{"index":0,"insert":true,"screen":{"html":"<h1>Wrong index"#
        )
        .unwrap()
        .is_empty()
    );
    assert!(preview_continuation_screens(2, "not JSON").is_none());
}
