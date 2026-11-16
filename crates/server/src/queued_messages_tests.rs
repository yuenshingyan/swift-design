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
    let encoded = serde_json::to_string(&message).expect("test value should serialize to JSON");
    assert_eq!(
        serde_json::from_str::<ChatMessage>(&encoded).expect("test JSON should deserialize"),
        message
    );
    let old: ChatMessage = serde_json::from_str(r#"{"role":"user","content":"Old prompt"}"#)
        .expect("test JSON should deserialize");
    assert!(!old.is_queued);
    assert!(
        !serde_json::to_string(&old)
            .expect("test value should serialize to JSON")
            .contains("is_queued")
    );
}

#[tokio::test]
async fn pending_prompts_are_persisted_and_hidden_from_the_current_turn() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let root = directory.path().join("sessions");
    let store = SessionStore::new(root.clone());
    store
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .expect("test fixture should be created");
    store
        .append_message("talk", ChatMessage::user("Original", None))
        .await
        .expect("test chat message should be appended");
    store
        .append_message("talk", queued("Next one"))
        .await
        .expect("test chat message should be appended");
    store
        .append_message("talk", queued("Next two"))
        .await
        .expect("test chat message should be appended");
    let reloaded = SessionStore::new(root);
    assert!(
        reloaded
            .has_queued_messages("talk")
            .await
            .expect("queued-message state should be readable")
    );
    assert_eq!(
        reloaded
            .messages("talk")
            .await
            .expect("test chat messages should be readable")
            .len(),
        3
    );
    assert_eq!(
        reloaded
            .messages_for_run("talk")
            .await
            .expect("chat messages for the test run should be readable")
            .len(),
        1
    );
}

#[tokio::test]
async fn activation_keeps_fifo_targets_actions_and_original_timestamps() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .expect("test fixture should be created");
    let mut first =
        queued("[screen 2] Write it anew.").with_pinned(vec!["talk-candidate-2".to_owned()]);
    first.is_regenerate = true;
    store
        .append_message("talk", first)
        .await
        .expect("test chat message should be appended");
    store
        .append_message("talk", queued("Second prompt"))
        .await
        .expect("test chat message should be appended");
    store
        .append_message("talk", ChatMessage::assistant("Previous turn saved."))
        .await
        .expect("test chat message should be appended");
    let timestamp = store
        .messages("talk")
        .await
        .expect("test chat messages should be readable")[0]
        .at
        .clone();
    let first = store
        .activate_next_message("talk")
        .await
        .expect("next queued test message should activate")
        .expect("queued test message should be activated");
    assert!(!first.is_queued);
    assert!(first.is_regenerate);
    assert_eq!(first.at, timestamp);
    assert_eq!(first.design.as_deref(), Some("talk-candidate-1"));
    assert_eq!(first.pinned, vec!["talk-candidate-2"]);
    assert_eq!(
        store
            .messages_for_run("talk")
            .await
            .expect("chat messages for the test run should be readable")
            .last()
            .expect("test collection should contain a last item"),
        &first
    );
    store
        .append_message("talk", ChatMessage::assistant("First prompt saved."))
        .await
        .expect("test chat message should be appended");
    assert_eq!(
        store
            .activate_next_message("talk")
            .await
            .expect("next queued test message should activate")
            .expect("queued test message should be activated")
            .content,
        "Second prompt"
    );
    assert!(
        store
            .activate_next_message("talk")
            .await
            .expect("next queued test message should activate")
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_queue_appends_keep_every_prompt() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .expect("test fixture should be created");
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..20 {
        let store = store.clone();
        tasks.spawn(async move {
            store
                .append_message("talk", queued(&format!("Prompt {index}")))
                .await
                .expect("test chat message should be appended");
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("test operation should return its expected result");
    }
    assert_eq!(
        store
            .messages("talk")
            .await
            .expect("test chat messages should be readable")
            .len(),
        20
    );
    assert!(
        store
            .messages_for_run("talk")
            .await
            .expect("chat messages for the test run should be readable")
            .is_empty()
    );
}

#[tokio::test]
async fn answered_prompts_resume_before_followups_without_duplicate_messages() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "Original request"))
        .await
        .expect("test fixture should be created");
    store
        .append_message(
            "talk",
            ChatMessage::user("Current prompt", Some("talk-candidate-1")),
        )
        .await
        .expect("test chat message should be appended");
    store
        .append_message("talk", queued("Later prompt"))
        .await
        .expect("test chat message should be appended");
    store
        .append_message("talk", ChatMessage::assistant_questions("Which color?", 1))
        .await
        .expect("test chat message should be appended");
    store
        .queue_answered_turn("talk")
        .await
        .expect("answered test turn should be queued");
    store
        .queue_answered_turn("talk")
        .await
        .expect("answered test turn should be queued");
    assert!(
        store
            .has_requested_resume("talk")
            .await
            .expect("test resume-request state should be readable")
    );
    assert_eq!(
        store
            .messages("talk")
            .await
            .expect("test chat messages should be readable")
            .len(),
        3
    );
    let resumed = store
        .activate_next_message("talk")
        .await
        .expect("next queued test message should activate")
        .expect("queued test message should be activated");
    assert_eq!(resumed.content, "Current prompt");
    assert!(!resumed.is_resume);
    assert!(
        !store
            .has_requested_resume("talk")
            .await
            .expect("test resume-request state should be readable")
    );
    assert_eq!(
        store
            .activate_next_message("talk")
            .await
            .expect("next queued test message should activate")
            .expect("queued test message should be activated")
            .content,
        "Later prompt"
    );
}

#[tokio::test]
async fn an_implicit_setup_request_can_resume_after_its_answers() {
    let directory = TempDir::new().expect("temporary test directory should be created");
    let store = SessionStore::new(directory.path().to_path_buf());
    store
        .create(NewSession::demo("talk", "Talk", "Original request"))
        .await
        .expect("test fixture should be created");
    store
        .append_message("talk", queued("Later prompt"))
        .await
        .expect("test chat message should be appended");
    store
        .queue_answered_turn("talk")
        .await
        .expect("answered test turn should be queued");
    assert_eq!(
        store
            .activate_next_message("talk")
            .await
            .expect("next queued test message should activate")
            .expect("queued test message should be activated")
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
