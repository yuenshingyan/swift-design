use super::*;

#[tokio::test]
async fn health_reports_ok() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(test_application(&directory), "GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn renders_a_valid_design_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/designs/render",
        Some(SAMPLE_DESIGN),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("<h1>Swift Design</h1>"));
}

#[tokio::test]
async fn reports_every_validation_error_for_an_invalid_design() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/designs/render",
        Some(&invalid_sample_design()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["error"]["message"], "design failed validation");
    assert_eq!(response["error"]["details"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn design_writes_are_rejected_unless_the_session_is_generating() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    // No session at all: 409.
    let (status, body) = send(
        application.clone(),
        "PUT",
        "/designs/overview",
        Some(SAMPLE_DESIGN),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("no session `overview`"));
    // Intake session: still 409.
    send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"overview","request":"An overview."}"#),
    )
    .await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/designs/overview",
        Some(SAMPLE_DESIGN),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // After generate the session is generating: 204.
    send(
        application.clone(),
        "POST",
        "/sessions/overview/generate",
        None,
    )
    .await;
    let (status, _) = send(application, "PUT", "/designs/overview", Some(SAMPLE_DESIGN)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn saving_then_fetching_a_design_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "overview").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/designs/overview",
        Some(SAMPLE_DESIGN),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(application, "GET", "/designs/overview", None).await;
    assert_eq!(status, StatusCode::OK);
    let fetched: serde_json::Value = serde_json::from_str(&body).unwrap();
    let original: serde_json::Value = serde_json::from_str(SAMPLE_DESIGN).unwrap();
    assert_eq!(fetched, original);
}

#[tokio::test]
async fn lists_saved_designs_sorted_by_id() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    for id in ["zebra", "alpha"] {
        open_generating_session(&application, id).await;
        let (status, _) = send(
            application.clone(),
            "PUT",
            &format!("/designs/{id}"),
            Some(SAMPLE_DESIGN),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (status, body) = send(application, "GET", "/designs", None).await;
    assert_eq!(status, StatusCode::OK);
    let summaries: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(summaries[0]["id"], "alpha");
    assert_eq!(summaries[1]["id"], "zebra");
    assert_eq!(summaries[0]["screen_count"], 3);
    assert!(summaries[0]["viewport"].is_object());
}

#[tokio::test]
async fn rejects_an_invalid_design_id() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "PUT",
        "/designs/Bad_Id",
        Some(SAMPLE_DESIGN),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("invalid design id"));
}

#[tokio::test]
async fn fetching_a_missing_design_returns_404() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "GET",
        "/designs/missing",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("no design with id `missing`"));
}

#[tokio::test]
async fn deleting_a_design_removes_it() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "gone").await;
    send(
        application.clone(),
        "PUT",
        "/designs/gone",
        Some(SAMPLE_DESIGN),
    )
    .await;
    let (status, _) = send(application.clone(), "DELETE", "/designs/gone", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(application.clone(), "GET", "/designs/gone", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(application, "DELETE", "/designs/gone", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn saving_an_invalid_design_reports_every_error() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "bad").await;
    let (status, body) = send(
        application,
        "PUT",
        "/designs/bad",
        Some(&invalid_sample_design()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["error"]["details"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn chooser_shows_every_candidate_and_records_the_choice() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "talk").await;
    for id in ["talk-candidate-1", "talk-candidate-2"] {
        send(
            application.clone(),
            "PUT",
            &format!("/designs/{id}"),
            Some(SAMPLE_DESIGN),
        )
        .await;
    }
    let (status, body) = send(application.clone(), "GET", "/candidates/talk", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("/designs/talk-candidate-1/render"));
    assert!(body.contains("talk-candidate-1 · midnight"));
    let (status, body) = send(
        application.clone(),
        "POST",
        "/candidates/talk/choose",
        Some(r#"{"id":"talk-candidate-2"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"id\":\"talk\""));
    let (_, body) = send(application, "GET", "/sessions/talk", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["chosen_design"], "talk-candidate-2");
}

#[tokio::test]
async fn choosing_a_foreign_design_is_rejected() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "other").await;
    send(
        application.clone(),
        "PUT",
        "/designs/other",
        Some(SAMPLE_DESIGN),
    )
    .await;
    let (status, body) = send(
        application,
        "POST",
        "/candidates/talk/choose",
        Some(r#"{"id":"other"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("is not a candidate of `talk`"));
}

#[tokio::test]
async fn screen_images_need_a_design_a_screen_and_chrome() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/designs/missing/screens/1.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("no design with id"));
    open_generating_session(&application, "shots").await;
    send(
        application.clone(),
        "PUT",
        "/designs/shots",
        Some(SAMPLE_DESIGN),
    )
    .await;
    let (status, body) = send(application, "GET", "/designs/shots/screens/9.png", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("has no screen 9"));
}
