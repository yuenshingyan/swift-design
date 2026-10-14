use super::*;

#[tokio::test]
async fn a_deck_session_lists_its_decks_and_chooses_from_the_deck_store() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_deck_session(&application, "talk").await;
    let (status, body) = send(application.clone(), "GET", "/sessions/talk", None).await;
    assert_eq!(status, StatusCode::OK);
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["session"]["artifact_kind"], "deck");
    assert_eq!(view["session"]["artifact_kind"], "deck");
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/decks/talk-candidate-1",
        Some(SAMPLE_DECK),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(application.clone(), "GET", "/sessions/talk", None).await;
    let view: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(view["decks"].as_array().unwrap().len(), 1);
    assert_eq!(view["designs"].as_array().unwrap().len(), 0);
    let (status, body) = send(application.clone(), "GET", "/candidates/talk", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Choose this deck"));
    assert!(body.contains("/decks/talk-candidate-1/render"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/candidates/talk/choose",
        Some(r#"{"id":"talk-candidate-1"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application.clone(), "GET", "/decks/talk", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(application, "GET", "/designs/talk", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn renders_a_valid_deck_to_html() {
    let directory = TempDir::new().unwrap();
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/decks/render",
        Some(SAMPLE_DECK),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains("data-swift-design-width=\"1920\""));
    let (status, body) = send(
        test_application(&directory),
        "POST",
        "/decks/render",
        Some(r##"{"title":"","theme":{"name":"x","colors":{"background":"#000000","text":"#ffffff","accent":"#ff0000","muted":"#888888"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"slides":[]}"##),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body.contains("deck failed validation"));
    assert!(body.contains("deck has no slides"));
}

#[tokio::test]
async fn saving_then_fetching_a_deck_round_trips() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "talk").await;
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/decks/talk-candidate-1",
        Some(SAMPLE_DECK),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send(application.clone(), "GET", "/decks/talk-candidate-1", None).await;
    assert_eq!(status, StatusCode::OK);
    let deck: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(deck["slides"].as_array().unwrap().len(), 3);
    let (status, body) = send(application.clone(), "GET", "/decks", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"slide_count\":3"));
    // The deck is not a design.
    let (status, _) = send(
        application.clone(),
        "GET",
        "/designs/talk-candidate-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = send(
        application,
        "GET",
        "/decks/talk-candidate-1/render?slide=2",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data-swift-design-screen=\"1\""));
}

#[tokio::test]
async fn deck_writes_are_rejected_unless_the_session_is_generating() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    let (status, body) = send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("no session `talk`"));
    let (status, _) = send(
        application.clone(),
        "POST",
        "/sessions",
        Some(r#"{"id":"talk","request":"A talk."}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = send(application, "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("not allowed"));
}

#[tokio::test]
async fn presents_a_stored_deck_with_its_notes() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "talk").await;
    send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
    let (status, body) = send(application.clone(), "GET", "/decks/talk/present", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<title>Swift Design Deck Overview · presenter</title>"));
    assert!(body.contains("Open with the one-line pitch."));
    assert!(body.contains("/decks/talk/render?audience=true"));
    let (status, _) = send(
        application.clone(),
        "GET",
        "/decks/talk/present?slide=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(application, "GET", "/decks/missing/present", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_the_audience_render_carries_the_follow_script() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "talk").await;
    send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
    let (status, plain) = send(application.clone(), "GET", "/decks/talk/render", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!plain.contains("data-swift-design-channel"));
    let (status, audience) =
        send(application, "GET", "/decks/talk/render?audience=true", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(audience.contains("data-swift-design-channel=\"swift-design-presenter:talk\""));
    assert!(audience.contains("swift-design-audience-hello"));
}
