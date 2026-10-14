use super::*;

#[tokio::test]
async fn a_print_session_lists_its_prints_and_chooses_from_the_print_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_print_session(&application, "poster").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/poster", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "print");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/prints/poster-candidate-1",
        Some(SAMPLE_PRINT),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/poster", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["prints"].as_array().unwrap().len(), 1);
    assert_eq!(view["prints"][0]["size"], "a4");
    assert_eq!(view["prints"][0]["orientation"], "portrait");
    assert_eq!(view["prints"][0]["sheet_count"], 2);
    assert_eq!(view["socials"].as_array().unwrap().len(), 0);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/poster", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this print piece"));
    assert!(body.contains("/prints/poster-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/poster/choose",
        Some(r#"{"id":"poster-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/prints/poster", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/socials/poster", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_print_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/prints/render",
        Some(SAMPLE_PRINT),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"794\""));
    assert!(body.contains("data-swift-design-height=\"1123\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/prints/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"sheets":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("print failed validation"));
    assert!(body.contains("print has no sheets"));
}

#[tokio::test]
async fn saving_then_fetching_a_print_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "poster").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/prints/poster-candidate-1",
        Some(SAMPLE_PRINT),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/prints/poster-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let print: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(print["sheets"].as_array().unwrap().len(), 2);
    let (status, body) = send(application.clone(), "GET", "/prints", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"sheet_count\":2"));
    // The print is not a social.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/socials/poster-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/prints/poster-candidate-1/render?sheet=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
    let (status, _) = send(
        application,
        "GET",
        "/prints/poster-candidate-1/render?sheet=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
