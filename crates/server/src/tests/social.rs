use super::*;

#[tokio::test]
async fn a_social_session_lists_its_socials_and_chooses_from_the_social_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_social_session(&application, "launch").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "social");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/socials/launch-candidate-1",
        Some(SAMPLE_SOCIAL),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["socials"].as_array().unwrap().len(), 1);
    assert_eq!(view["socials"][0]["format"], "portrait");
    assert_eq!(view["socials"][0]["frame_count"], 3);
    assert_eq!(view["documents"].as_array().unwrap().len(), 0);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this social post"));
    assert!(body.contains("/socials/launch-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/launch/choose",
        Some(r#"{"id":"launch-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/socials/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/documents/launch", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_social_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/socials/render",
        Some(SAMPLE_SOCIAL),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"1080\""));
    assert!(body.contains("data-swift-design-height=\"1350\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/socials/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"frames":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("social failed validation"));
    assert!(body.contains("social has no frames"));
}

#[tokio::test]
async fn saving_then_fetching_a_social_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/socials/launch-candidate-1",
        Some(SAMPLE_SOCIAL),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/socials/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let social: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(social["frames"].as_array().unwrap().len(), 3);
    let (status, body) = send(application.clone(), "GET", "/socials", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"frame_count\":3"));
    // The social is not a document.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/documents/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/socials/launch-candidate-1/render?frame=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
    let (status, _) = send(
        application,
        "GET",
        "/socials/launch-candidate-1/render?frame=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
