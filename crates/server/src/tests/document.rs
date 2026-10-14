use super::*;

#[tokio::test]
async fn a_document_session_lists_its_documents_and_chooses_from_the_document_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_document_session(&application, "report").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/report", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "document");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/documents/report-candidate-1",
        Some(SAMPLE_DOCUMENT),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/report", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["documents"].as_array().unwrap().len(), 1);
    assert_eq!(view["documents"][0]["paper"], "a4");
    assert_eq!(view["decks"].as_array().unwrap().len(), 0);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/report", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this document"));
    assert!(body.contains("/documents/report-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/report/choose",
        Some(r#"{"id":"report-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/documents/report", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/decks/report", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_document_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/documents/render",
        Some(SAMPLE_DOCUMENT),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"794\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/documents/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"pages":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("document failed validation"));
    assert!(body.contains("document has no pages"));
}

#[tokio::test]
async fn saving_then_fetching_a_document_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "report").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/documents/report-candidate-1",
        Some(SAMPLE_DOCUMENT),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/documents/report-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let document: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(document["pages"].as_array().unwrap().len(), 3);
    let (status, body) = send(application.clone(), "GET", "/documents", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"page_count\":3"));
    // The document is not a deck.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/decks/report-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application.clone(),
        "GET",
        "/documents/report-candidate-1/render?page=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
    let (status, _) = send(
        application,
        "GET",
        "/documents/report-candidate-1/render?page=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
