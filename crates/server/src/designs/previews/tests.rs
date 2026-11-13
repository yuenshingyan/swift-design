use axum::body::to_bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;

use super::*;
use crate::designs::{RenderQuery, get_design, get_preview, list_designs, render_stored_design};
use crate::history::HistoryStore;

fn sample() -> Design {
    serde_json::from_str(include_str!("../../../../../fixtures/sample-design.json")).unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn live_preview_never_changes_saved_design_history_or_default_render() {
    let directory = tempfile::tempdir().unwrap();
    let store = DesignStore::new(directory.path().join("designs"))
        .with_history(HistoryStore::new(directory.path().join("history")));
    let original = sample();
    store.save("demo", &original).await.unwrap();
    let notifier = ChangeNotifier::new();
    let preview = store.start_preview("demo", &notifier, None);
    let mut draft = original.clone();
    draft.title = "Writing a temporary design".into();
    preview.show(&draft);
    assert_eq!(store.load("demo").await.unwrap(), Some(original.clone()));
    assert!(store.history("demo").await.unwrap().is_empty());
    let saved: Design = serde_json::from_str(
        &body(get_design(State(store.clone()), Path("demo".into())).await).await,
    )
    .unwrap();
    assert_eq!(saved, original);
    let saved_render = body(
        render_stored_design(
            State(store.clone()),
            Path("demo".into()),
            Query(RenderQuery {
                editable: false,
                preview: false,
                screen: None,
            }),
        )
        .await,
    )
    .await;
    assert!(!saved_render.contains("Live draft — unverified"));
    assert!(!saved_render.contains("Writing a temporary design"));
    let rendered = render_stored_design(
        State(store.clone()),
        Path("demo".into()),
        Query(RenderQuery {
            editable: true,
            preview: true,
            screen: Some(1),
        }),
    )
    .await;
    assert_eq!(rendered.headers()["cache-control"], "no-store");
    let rendered = body(rendered).await;
    assert!(rendered.contains("Live draft — unverified"));
    assert!(!rendered.contains("swift-design-html"));
    let listed: Vec<serde_json::Value> =
        serde_json::from_str(&body(list_designs(State(store.clone())).await).await).unwrap();
    assert_eq!(listed[0]["title"], draft.title);
    drop(preview);
    assert!(store.preview("demo").is_none());
    assert_eq!(store.load("demo").await.unwrap(), Some(original));
}

#[tokio::test]
async fn new_draft_is_listed_without_creating_a_saved_file_and_disappears_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let store = DesignStore::new(directory.path().join("not-created"));
    let preview = store.start_preview("demo", &ChangeNotifier::new(), None);
    preview.show(&sample());
    assert_eq!(
        get_design(State(store.clone()), Path("demo".into()))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = get_preview(State(store.clone()), Path("demo".into())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let value: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(value["design"]["title"], sample().title);
    assert!(!directory.path().join("not-created").exists());
    let listed: Vec<serde_json::Value> =
        serde_json::from_str(&body(list_designs(State(store.clone())).await).await).unwrap();
    assert_eq!(listed.len(), 1);
    let clone = preview.clone();
    drop(preview);
    assert!(store.preview("demo").is_some());
    drop(clone);
    let response = get_preview(State(store.clone()), Path("demo".into())).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(body(list_designs(State(store)).await).await, "[]");
}

#[test]
fn stale_owner_cannot_publish_or_clear_a_newer_preview() {
    let store = DesignStore::new(std::path::PathBuf::new());
    let notifier = ChangeNotifier::new();
    let changes = notifier.subscribe();
    let old = store.start_preview("demo", &notifier, None);
    old.show(&sample());
    assert_eq!(*changes.borrow(), 1);
    let new = store.start_preview("demo", &notifier, None);
    assert_eq!(*changes.borrow(), 2);
    let mut newer = sample();
    newer.title = "Newer".into();
    new.show(&newer);
    old.show(&sample());
    drop(old);
    assert_eq!(store.preview("demo").unwrap().design, newer);
    assert_eq!(*changes.borrow(), 3);
    new.show(&newer);
    assert_eq!(*changes.borrow(), 3);
    drop(new);
    assert_eq!(*changes.borrow(), 4);
    assert!(store.preview("demo").is_none());
}

#[test]
fn unsafe_or_out_of_scope_updates_leave_the_current_preview_unchanged() {
    let store = DesignStore::new(std::path::PathBuf::new());
    let original = sample();
    let preview = store.start_preview(
        "demo",
        &ChangeNotifier::new(),
        Some((original.clone(), vec![0])),
    );
    preview.show(&original);
    let mut unsafe_draft = original.clone();
    unsafe_draft.screens[0].html = "<script>alert(1)</script>".into();
    preview.show(&unsafe_draft);
    let mut outside = original.clone();
    outside.screens[1].name = "Unselected change".into();
    preview.show(&outside);
    assert_eq!(store.preview("demo").unwrap().design, original);
    let mut allowed = original.clone();
    allowed.screens[0].html = "<h1>Allowed edit</h1>".into();
    preview.show(&allowed);
    assert_eq!(store.preview("demo").unwrap().design, allowed);
}

#[tokio::test]
async fn cancelling_a_run_drops_its_preview() {
    let store = DesignStore::new(std::path::PathBuf::new());
    let running_store = store.clone();
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let preview = running_store.start_preview("demo", &ChangeNotifier::new(), None);
        preview.show(&sample());
        ready.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(preview);
    });
    waiting.await.unwrap();
    assert!(store.preview("demo").is_some());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(store.preview("demo").is_none());
}

#[test]
fn preview_parsing_is_throttled() {
    let store = DesignStore::new(std::path::PathBuf::new());
    let preview = store.start_preview("demo", &ChangeNotifier::new(), None);
    assert!(preview.should_update());
    assert!(!preview.should_update());
    *preview.0.last_attempt.lock().unwrap() = Instant::now() - Duration::from_millis(201);
    assert!(preview.should_update());
}
