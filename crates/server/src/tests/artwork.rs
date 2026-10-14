use super::*;

#[tokio::test]
async fn an_artwork_session_lists_its_artworks_and_chooses_from_the_artwork_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_artwork_session(&application, "launch").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "artwork");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/artworks/launch-candidate-1",
        Some(SAMPLE_ARTWORK),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["artworks"].as_array().unwrap().len(), 1);
    assert_eq!(view["artworks"][0]["size"], "thumbnail");
    assert_eq!(view["artworks"][0]["cover_count"], 2);
    assert_eq!(view["campaigns"].as_array().unwrap().len(), 0);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this artwork"));
    assert!(body.contains("/artworks/launch-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/launch/choose",
        Some(r#"{"id":"launch-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/artworks/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/campaigns/launch", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_artwork_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/artworks/render",
        Some(SAMPLE_ARTWORK),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"1280\""));
    assert!(body.contains("data-swift-design-height=\"720\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/artworks/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"covers":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("artwork failed validation"));
    assert!(body.contains("artwork has no covers"));
}

#[tokio::test]
async fn saving_then_fetching_an_artwork_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/artworks/launch-candidate-1",
        Some(SAMPLE_ARTWORK),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/artworks/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let artwork: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(artwork["covers"].as_array().unwrap().len(), 2);
    let (status, body) = send(application.clone(), "GET", "/artworks", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"cover_count\":2"));
    // The artwork is not a campaign.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/campaigns/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/artworks/launch-candidate-1/render?cover=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
    let (status, _) = send(
        application,
        "GET",
        "/artworks/launch-candidate-1/render?cover=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
