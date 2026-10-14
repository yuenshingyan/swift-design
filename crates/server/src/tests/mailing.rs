use super::*;

#[tokio::test]
async fn a_mailing_session_lists_its_mailings_and_chooses_from_the_mailing_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_mailing_session(&application, "launch").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "mailing");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/mailings/launch-candidate-1",
        Some(SAMPLE_MAILING),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/launch", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["mailings"].as_array().unwrap().len(), 1);
    assert_eq!(view["mailings"][0]["format"], "standard");
    assert_eq!(view["mailings"][0]["email_count"], 2);
    assert_eq!(view["prints"].as_array().unwrap().len(), 0);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this mailing"));
    assert!(body.contains("/mailings/launch-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/launch/choose",
        Some(r#"{"id":"launch-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/mailings/launch", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/prints/launch", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_mailing_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/mailings/render",
        Some(SAMPLE_MAILING),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"600\""));
    assert!(body.contains("data-swift-design-height=\"1200\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/mailings/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"emails":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("mailing failed validation"));
    assert!(body.contains("mailing has no emails"));
}

#[tokio::test]
async fn saving_then_fetching_a_mailing_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/mailings/launch-candidate-1",
        Some(SAMPLE_MAILING),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/mailings/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mailing: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(mailing["emails"].as_array().unwrap().len(), 2);
    let (status, body) = send(application.clone(), "GET", "/mailings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"email_count\":2"));
    // The mailing is not a print.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/prints/launch-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/mailings/launch-candidate-1/render?email=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
    let (status, _) = send(
        application,
        "GET",
        "/mailings/launch-candidate-1/render?email=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn one_email_serves_as_email_client_html() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/mailings/launch",
        Some(SAMPLE_MAILING),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/mailings/launch/emails/2.html",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains("<!--[if mso]>"));
    assert!(!body.contains("var("));
    let (status, _) = send(
        application.clone(),
        "GET",
        "/mailings/launch/emails/9.html",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
