#![allow(clippy::unwrap_used)]

use super::*;

use crate::test_support::{sample_deck, sample_document, sample_social, send, test_application};

fn sample_design() -> design_model::Design {
    serde_json::from_str(include_str!("../../../../fixtures/sample-design.json")).unwrap()
}

#[test]
fn a_save_request_needs_exactly_one_source() {
    let both = SaveRequest {
        design_id: Some("a".to_owned()),
        deck_id: Some("b".to_owned()),
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name: "x".to_owned(),
    };
    assert!(template_source(&both).is_err());
    let none = SaveRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name: "x".to_owned(),
    };
    assert!(template_source(&none).is_err());
    let deck = SaveRequest {
        design_id: None,
        deck_id: Some("talk".to_owned()),
        document_id: None,
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name: "x".to_owned(),
    };
    assert_eq!(
        template_source(&deck).unwrap(),
        TemplateSource::Deck("talk".to_owned())
    );
    let document = SaveRequest {
        design_id: None,
        deck_id: None,
        document_id: Some("report".to_owned()),
        social_id: None,
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name: "x".to_owned(),
    };
    assert_eq!(
        template_source(&document).unwrap(),
        TemplateSource::Document("report".to_owned())
    );
    let social = SaveRequest {
        design_id: None,
        deck_id: None,
        document_id: None,
        social_id: Some("launch".to_owned()),
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name: "x".to_owned(),
    };
    assert_eq!(
        template_source(&social).unwrap(),
        TemplateSource::Social("launch".to_owned())
    );
    let document_and_social = SaveRequest {
        design_id: None,
        deck_id: None,
        document_id: Some("report".to_owned()),
        social_id: Some("launch".to_owned()),
        print_id: None,
        mailing_id: None,
        campaign_id: None,
        artwork_id: None,
        name: "x".to_owned(),
    };
    assert!(template_source(&document_and_social).is_err());
}

#[tokio::test]
async fn a_template_saves_from_a_social() {
    let directory = tempfile::tempdir().unwrap();
    let socials = SocialStore::new(directory.path().join("socials"));
    socials.save("launch", &sample_social()).await.unwrap();
    let style = social_style(&socials, "launch").await.ok().unwrap();
    assert_eq!(style.viewport, design_model::PORTRAIT_VIEWPORT);
    assert_eq!(style.screens.len(), 3);
    assert!(style.screens[0].name.is_empty());
    assert!(style.screens[0].html.contains("One harness"));
    assert!(social_style(&socials, "missing").await.is_err());
}

#[tokio::test]
async fn a_template_saves_from_a_document() {
    let directory = tempfile::tempdir().unwrap();
    let documents = DocumentStore::new(directory.path().join("documents"));
    documents.save("report", &sample_document()).await.unwrap();
    let style = document_style(&documents, "report").await.ok().unwrap();
    assert_eq!(style.viewport, design_model::A4_VIEWPORT);
    assert_eq!(style.screens.len(), 3);
    assert!(style.screens[0].name.is_empty());
    assert!(style.screens[0].html.contains("Swift Design"));
    assert!(document_style(&documents, "missing").await.is_err());
}

#[tokio::test]
async fn a_template_saves_from_a_deck() {
    let directory = tempfile::tempdir().unwrap();
    let decks = DeckStore::new(directory.path().join("decks"));
    decks.save("talk", &sample_deck()).await.unwrap();
    let style = deck_style(&decks, "talk").await.ok().unwrap();
    assert_eq!(style.viewport, DECK_VIEWPORT);
    assert_eq!(style.screens.len(), 3);
    assert!(style.screens[0].name.is_empty());
    assert!(style.screens[0].html.contains("Swift Design"));
    assert!(deck_style(&decks, "missing").await.is_err());
}

#[tokio::test]
async fn a_screenless_template_renders_as_a_swatch_and_takes_the_default_mark() {
    let directory = tempfile::tempdir().unwrap();
    let application = test_application(&directory);
    let store = TemplateStore::new(directory.path().join("templates"));
    let design = sample_design();
    store
        .save(&Template {
            id: "acme-1".to_owned(),
            name: "Acme".to_owned(),
            saved_at: "2024-01-01T00:00:00Z".to_owned(),
            source_design: "https://acme.com".to_owned(),
            theme: design.theme.clone(),
            viewport: design.viewport,
            screens: Vec::new(),
            note: Some("Generous whitespace & 8px corners.".to_owned()),
            is_default: false,
        })
        .await
        .unwrap();
    let (status, body) = send(
        application.clone(),
        "GET",
        "/templates/acme-1/render?screen=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Generous whitespace &amp; 8px corners."));
    assert!(body.contains("Get started"));
    let (status, body) = send(
        application.clone(),
        "PUT",
        "/templates/acme-1/default",
        Some(r#"{"is_default":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"is_default\":true"));
    let (_, listing) = send(application.clone(), "GET", "/templates", None).await;
    assert!(listing.contains("\"is_default\":true"));
    assert!(store.load("acme-1").await.unwrap().unwrap().is_default);
    let (status, _) = send(
        application.clone(),
        "PUT",
        "/templates/missing/default",
        Some(r#"{"is_default":true}"#),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_extraction_needs_a_name_one_source_and_a_model() {
    let directory = tempfile::tempdir().unwrap();
    let application = test_application(&directory);
    let (status, body) = send(
        application.clone(),
        "POST",
        "/templates/extract",
        Some(r#"{"name":"","url":"https://acme.com"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, body) = send(
        application.clone(),
        "POST",
        "/templates/extract",
        Some(r#"{"name":"Acme"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("exactly one source"));
    // No model is chosen in a fresh test application.
    let (status, body) = send(
        application.clone(),
        "POST",
        "/templates/extract",
        Some(r#"{"name":"Acme","url":"https://acme.com"}"#),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("choose a model"));
}

#[test]
fn template_ids_are_slugs_with_the_save_time() {
    let id = template_id("Midnight Finance!");
    assert!(id.starts_with("midnight-finance-"), "{id}");
    assert!(template_id("***").starts_with("template-"));
    assert!(!template_id("A").contains("--"));
}

#[tokio::test]
async fn saved_templates_round_trip_and_list_newest_first() {
    let directory = tempfile::tempdir().unwrap();
    let store = TemplateStore::new(directory.path().to_path_buf());
    assert!(store.list().await.unwrap().is_empty());
    let design = sample_design();
    let older = Template {
        id: "older".to_owned(),
        name: "Older".to_owned(),
        saved_at: "2024-01-01T00:00:00Z".to_owned(),
        source_design: "talk".to_owned(),
        theme: design.theme.clone(),
        viewport: design.viewport,
        note: None,
        is_default: false,
        screens: design.screens.clone(),
    };
    let newer = Template {
        id: "newer".to_owned(),
        name: "Newer".to_owned(),
        saved_at: "2025-01-01T00:00:00Z".to_owned(),
        ..older.clone()
    };
    store.save(&older).await.unwrap();
    store.save(&newer).await.unwrap();
    let listed = store.list().await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["newer", "older"]
    );
    let loaded = store.load("older").await.unwrap().unwrap();
    assert_eq!(loaded.theme, design.theme);
    assert_eq!(loaded.screens, design.screens);
    assert!(store.delete("older").await.unwrap());
    assert!(!store.delete("older").await.unwrap());
    assert!(store.load("older").await.unwrap().is_none());
}
