use super::*;

#[tokio::test]
async fn a_campaign_session_lists_its_campaigns_and_chooses_from_the_campaign_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_campaign_session(&application, "launch").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "campaign");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/campaigns/launch-candidate-1",
        Some(SAMPLE_CAMPAIGN),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["campaigns"].as_array().unwrap().len(), 1);
    assert_eq!(view["campaigns"][0]["size"], "medium_rectangle");
    assert_eq!(view["campaigns"][0]["ad_count"], 2);
    assert_eq!(view["mailings"].as_array().unwrap().len(), 0);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this campaign"));
    assert!(body.contains("/campaigns/launch-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/launch/choose",
        Some(r#"{"id":"launch-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/campaigns/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/mailings/launch", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_campaign_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/campaigns/render",
        Some(SAMPLE_CAMPAIGN),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"300\""));
    assert!(body.contains("data-swift-design-height=\"250\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/campaigns/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"ads":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("campaign failed validation"));
    assert!(body.contains("campaign has no ads"));
}

#[tokio::test]
async fn saving_then_fetching_a_campaign_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/campaigns/launch-candidate-1",
        Some(SAMPLE_CAMPAIGN),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/campaigns/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let campaign: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(campaign["ads"].as_array().unwrap().len(), 2);
    let (status, body) = send(application.clone(), "GET", "/campaigns", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ad_count\":2"));
    // The campaign is not a mailing.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/mailings/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/campaigns/launch-candidate-1/render?ad=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
    let (status, _) = send(
        application,
        "GET",
        "/campaigns/launch-candidate-1/render?ad=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
