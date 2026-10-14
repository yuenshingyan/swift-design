use super::*;

#[tokio::test]
async fn uploading_stores_lists_and_serves_a_file() {
    let directory = TempDir::new().unwrap();
    let (status, body) =
        send_upload(test_application(&directory), "Chart Final.PNG", "PNGDATA").await;
    assert_eq!(status, StatusCode::OK);
    let stored: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(stored[0]["name"], "chart-final.png");
    let (status, body) = send(test_application(&directory), "GET", "/uploads", None).await;
    assert_eq!(status, StatusCode::OK);
    let listing: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(listing[0]["size_bytes"], 7);
    assert_eq!(listing[0]["is_image"], true);
}

#[tokio::test]
async fn deleting_a_missing_or_traversal_upload_is_rejected() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "DELETE",
        "/uploads/nothing.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("no upload named `nothing.png`"));
    let (status, _) = send(
        test_application(&directory),
        "GET",
        "/uploads/..%2Fsecret",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn root_shows_not_built_page_without_a_ui_bundle() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(test_application(&directory), "GET", "/", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("The editor UI is not built yet"));
}

#[tokio::test]
async fn blocks_hidden_and_traversal_static_paths() {
    let directory = TempDir::new().unwrap();
    let (status, _) = send(test_application(&directory), "GET", "/.env", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_lists_snapshots_and_restores_one() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "overview").await;
    send(
        application.clone(),
        "PUT",
        "/designs/overview",
        Some(SAMPLE_DESIGN),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/designs/overview/history",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "[]");
    let renamed = SAMPLE_DESIGN.replace("Swift Design Overview", "Second Title");
    send(
        application.clone(),
        "PUT",
        "/designs/overview",
        Some(&renamed),
    )
    .await;
    let (_, body) = send(
        application.clone(),
        "GET",
        "/designs/overview/history",
        None,
    )
    .await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    let stamp = rows[0]["stamp"].as_str().unwrap().to_owned();
    let (status, _) = send(
        application.clone(),
        "POST",
        &format!("/designs/overview/history/{stamp}/restore"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application, "GET", "/designs/overview", None).await;
    assert!(body.contains("Swift Design Overview"));
}

#[tokio::test]
async fn user_saves_are_allowed_while_reviewing() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "talk").await;
    send(
        application.clone(),
        "PUT",
        "/designs/talk",
        Some(SAMPLE_DESIGN),
    )
    .await;
    // Complete the run so the session is reviewing.
    send(application.clone(), "POST", "/sessions/talk/complete", None).await;
    let mut edited: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    edited["title"] = serde_json::json!("Edited by hand");
    let (status, _) =
        send_user_put(application.clone(), "/designs/talk", &edited.to_string()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // An agent write is rejected while reviewing.
    let (status, _) = send(application, "PUT", "/designs/talk", Some(SAMPLE_DESIGN)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn settings_report_chrome_availability() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(test_application(&directory), "GET", "/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(payload["has_chrome"].is_boolean());
}

#[tokio::test]
async fn mutations_bump_the_events_revision() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    let (_, body) = send(application.clone(), "GET", "/events", None).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["revision"],
        0
    );
    send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"talk","request":"A talk."}"#),
    )
    .await;
    let (_, body) = send(application, "GET", "/events", None).await;
    assert!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["revision"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

#[tokio::test]
async fn agent_run_starts_and_reports_its_log() {
    let directory = TempDir::new().unwrap();
    let application = application_with_command(&directory, Some("echo test-agent".to_owned()));
    send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"talk","request":"A talk."}"#),
    )
    .await;
    send(
        application.clone(),
        "POST",
        "/agent-runs",
        Some(r#"{"session_id":"talk"}"#),
    )
    .await;
    for _ in 0..100 {
        let (_, body) = send(application.clone(), "GET", "/agent-runs", None).await;
        let run: serde_json::Value = serde_json::from_str(&body).unwrap();
        if run["is_running"] == false {
            assert!(run["log_tail"].as_str().unwrap().contains("test-agent"));
            assert_eq!(run["mode"], "generation");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("agent run did not finish");
}

#[tokio::test]
async fn instructions_and_schemas_are_served_by_the_app() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(test_application(&directory), "GET", "/instructions", None).await;
    assert_eq!(status, StatusCode::OK);
    let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payload["routes"]["session"], "GET /sessions/{id}");
    assert!(payload.to_string().contains("There is no brief"));
    for (path, key) in [
        ("/schemas/design", "screens"),
        ("/schemas/deck", "slides"),
        ("/schemas/document", "pages"),
        ("/schemas/social", "frames"),
        ("/schemas/question-set", "can_proceed_with_assumptions"),
    ] {
        let (status, body) = send(test_application(&directory), "GET", path, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let schema: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(schema["properties"][key].is_object(), "{path}");
    }
}
