#![allow(clippy::unwrap_used)]

use tempfile::TempDir;

use super::*;

fn queued(content: &str) -> ChatMessage {
    let mut message = ChatMessage::user(content, Some("talk-candidate-1"));
    message.is_queued = true;
    message
}

#[test]
fn queued_messages_round_trip_and_old_messages_default_to_not_queued() {
    let message =
        queued("Make the header smaller.").with_pinned(vec!["talk-candidate-2".to_owned()]);
    let encoded = serde_json::to_string(&message).unwrap();
    assert_eq!(
        serde_json::from_str::<ChatMessage>(&encoded).unwrap(),
        message
    );
    let old: ChatMessage =
        serde_json::from_str(r#"{"role":"user","content":"Old prompt"}"#).unwrap();
    assert!(!old.is_queued);
    assert!(!serde_json::to_string(&old).unwrap().contains("is_queued"));
}

#[tokio::test]
async fn pending_prompts_are_persisted_and_hidden_from_the_current_turn() {
    let directory = TempDir::new().unwrap();
    let root = directory.path().join("sessions");
    let store = SessionStore::new(root.clone());
    store
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .unwrap();
    store
        .append_message("talk", ChatMessage::user("Original", None))
        .await
        .unwrap();
    store
        .append_message("talk", queued("Next one"))
        .await
        .unwrap();
    store
        .append_message("talk", queued("Next two"))
        .await
        .unwrap();
    let reloaded = SessionStore::new(root);
    assert!(reloaded.has_queued_messages("talk").await.unwrap());
    assert_eq!(reloaded.messages("talk").await.unwrap().len(), 3);
    assert_eq!(reloaded.messages_for_run("talk").await.unwrap().len(), 1);
}

#[tokio::test]
async fn activation_keeps_fifo_targets_actions_and_original_timestamps() {
    let directory = TempDir::new().unwrap();
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .unwrap();
    let mut first =
        queued("[screen 2] Write it anew.").with_pinned(vec!["talk-candidate-2".to_owned()]);
    first.is_regenerate = true;
    store.append_message("talk", first).await.unwrap();
    store
        .append_message("talk", queued("Second prompt"))
        .await
        .unwrap();
    store
        .append_message("talk", ChatMessage::assistant("Previous turn saved."))
        .await
        .unwrap();
    let timestamp = store.messages("talk").await.unwrap()[0].at.clone();
    let first = store.activate_next_message("talk").await.unwrap().unwrap();
    assert!(!first.is_queued);
    assert!(first.is_regenerate);
    assert_eq!(first.at, timestamp);
    assert_eq!(first.design.as_deref(), Some("talk-candidate-1"));
    assert_eq!(first.pinned, vec!["talk-candidate-2"]);
    assert_eq!(
        store
            .messages_for_run("talk")
            .await
            .unwrap()
            .last()
            .unwrap(),
        &first
    );
    store
        .append_message("talk", ChatMessage::assistant("First prompt saved."))
        .await
        .unwrap();
    assert_eq!(
        store
            .activate_next_message("talk")
            .await
            .unwrap()
            .unwrap()
            .content,
        "Second prompt"
    );
    assert!(store.activate_next_message("talk").await.unwrap().is_none());
}

#[tokio::test]
async fn concurrent_queue_appends_keep_every_prompt() {
    let directory = TempDir::new().unwrap();
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..20 {
        let store = store.clone();
        tasks.spawn(async move {
            store
                .append_message("talk", queued(&format!("Prompt {index}")))
                .await
                .unwrap();
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    assert_eq!(store.messages("talk").await.unwrap().len(), 20);
    assert!(store.messages_for_run("talk").await.unwrap().is_empty());
}

#[tokio::test]
async fn answered_prompts_resume_before_followups_without_duplicate_messages() {
    let directory = TempDir::new().unwrap();
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "Original request"))
        .await
        .unwrap();
    store
        .append_message(
            "talk",
            ChatMessage::user("Current prompt", Some("talk-candidate-1")),
        )
        .await
        .unwrap();
    store
        .append_message("talk", queued("Later prompt"))
        .await
        .unwrap();
    store
        .append_message("talk", ChatMessage::assistant_questions("Which color?", 1))
        .await
        .unwrap();
    store.queue_answered_turn("talk").await.unwrap();
    store.queue_answered_turn("talk").await.unwrap();
    assert!(store.has_requested_resume("talk").await.unwrap());
    assert_eq!(store.messages("talk").await.unwrap().len(), 3);
    let resumed = store.activate_next_message("talk").await.unwrap().unwrap();
    assert_eq!(resumed.content, "Current prompt");
    assert!(!resumed.is_resume);
    assert!(!store.has_requested_resume("talk").await.unwrap());
    assert_eq!(
        store
            .activate_next_message("talk")
            .await
            .unwrap()
            .unwrap()
            .content,
        "Later prompt"
    );
}

#[tokio::test]
async fn an_implicit_setup_request_can_resume_after_its_answers() {
    let directory = TempDir::new().unwrap();
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "Original request"))
        .await
        .unwrap();
    store
        .append_message("talk", queued("Later prompt"))
        .await
        .unwrap();
    store.queue_answered_turn("talk").await.unwrap();
    assert_eq!(
        store
            .activate_next_message("talk")
            .await
            .unwrap()
            .unwrap()
            .content,
        "Original request"
    );
}

#[test]
fn rapid_runs_sort_by_numeric_counter_not_lexical_counter() {
    assert!(run_sort_key("1790000000-9") < run_sort_key("1790000000-10"));
    assert!(run_sort_key("1790000000-100") < run_sort_key("1790000001-1"));
    assert_eq!(run_sort_key("legacy"), (0, 0));
}
