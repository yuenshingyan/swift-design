use super::*;

#[tokio::test]
async fn exporting_a_mailing_returns_an_email_html_zip() {
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
    let request = Request::builder()
        .method("GET")
        .uri("/mailings/launch/export.email.zip")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"launch.email.zip\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "launch-email-1.html",
            "launch-email-2.html",
            "launch-subjects.txt"
        ]
    );
    let mut first = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("launch-email-1.html").unwrap(),
        &mut first,
    )
    .unwrap();
    assert!(first.contains("<!--[if mso]>"));
    assert!(first.contains("<h1 style="));
    assert!(!first.contains("var("));
}

#[tokio::test]
async fn exporting_a_mailing_returns_an_html_download() {
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
    let request = Request::builder()
        .method("GET")
        .uri("/mailings/launch/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"launch.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>Six kinds. One chat.</h1>")
    );
}

#[tokio::test]
async fn exporting_an_artwork_returns_an_html_download() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/artworks/launch",
        Some(SAMPLE_ARTWORK),
    )
    .await;
    let request = Request::builder()
        .method("GET")
        .uri("/artworks/launch/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"launch.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>Eight kinds. One chat.</h1>")
    );
}

/// GETs `uri` and asserts a scoped export: 200, the download named
/// `filename`, a body that holds `present` and not `absent`.
async fn assert_scoped_export(
    application: axum::Router,
    uri: &str,
    filename: &str,
    markers: (&str, &str),
) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        format!("attachment; filename=\"{filename}\"")
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    let (present, absent) = markers;
    assert!(html.contains(present), "missing {present} in {uri}");
    assert!(!html.contains(absent), "unexpected {absent} in {uri}");
}

#[tokio::test]
async fn exporting_one_frame_returns_that_frame_alone() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/socials/launch",
        Some(SAMPLE_SOCIAL),
    )
    .await;
    assert_scoped_export(
        application.clone(),
        "/socials/launch/export?frame=2",
        "launch-frame-2.html",
        ("id=\"frame-2\"", "id=\"frame-1\""),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/socials/launch/export?frame=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("use 1 to 3"));
}

#[tokio::test]
async fn exporting_one_sheet_returns_that_sheet_alone() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/prints/launch",
        Some(SAMPLE_PRINT),
    )
    .await;
    assert_scoped_export(
        application.clone(),
        "/prints/launch/export?sheet=2",
        "launch-sheet-2.html",
        ("id=\"sheet-2\"", "id=\"sheet-1\""),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/prints/launch/export?sheet=0",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("use 1 to 2"));
}

#[tokio::test]
async fn exporting_one_email_returns_that_email_alone() {
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
    assert_scoped_export(
        application.clone(),
        "/mailings/launch/export?email=2",
        "launch-email-2.html",
        ("id=\"email-2\"", "id=\"email-1\""),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/mailings/launch/export?email=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("use 1 to 2"));
}

#[tokio::test]
async fn exporting_one_ad_returns_that_ad_alone() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/campaigns/launch",
        Some(SAMPLE_CAMPAIGN),
    )
    .await;
    assert_scoped_export(
        application.clone(),
        "/campaigns/launch/export?ad=2",
        "launch-ad-2.html",
        ("id=\"ad-2\"", "id=\"ad-1\""),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/campaigns/launch/export?ad=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("use 1 to 2"));
}

#[tokio::test]
async fn exporting_one_cover_returns_that_cover_alone() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/artworks/launch",
        Some(SAMPLE_ARTWORK),
    )
    .await;
    assert_scoped_export(
        application.clone(),
        "/artworks/launch/export?cover=2",
        "launch-cover-2.html",
        ("id=\"cover-2\"", "id=\"cover-1\""),
    )
    .await;
    let (status, body) = send(
        application.clone(),
        "GET",
        "/artworks/launch/export?cover=9",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("use 1 to 2"));
}

#[tokio::test]
async fn exporting_a_campaign_returns_an_html_download() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/campaigns/launch",
        Some(SAMPLE_CAMPAIGN),
    )
    .await;
    let request = Request::builder()
        .method("GET")
        .uri("/campaigns/launch/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"launch.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>Seven kinds. One chat.</h1>")
    );
}

#[tokio::test]
async fn exporting_a_print_returns_an_html_download() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "poster").await;
    send(
        application.clone(),
        "PUT",
        "/prints/poster",
        Some(SAMPLE_PRINT),
    )
    .await;
    let request = Request::builder()
        .method("GET")
        .uri("/prints/poster/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"poster.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>One harness. Five kinds.</h1>")
    );
}

#[tokio::test]
async fn exporting_a_social_returns_an_html_download() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "launch").await;
    send(
        application.clone(),
        "PUT",
        "/socials/launch",
        Some(SAMPLE_SOCIAL),
    )
    .await;
    let request = Request::builder()
        .method("GET")
        .uri("/socials/launch/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"launch.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>One harness. Four kinds.</h1>")
    );
    let (status, _) = send(
        application.clone(),
        "GET",
        "/socials/launch/frames/cover.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        application.clone(),
        "GET",
        "/socials/launch/frames/9.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(application, "GET", "/socials/missing/export.zip", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn exporting_a_document_returns_an_html_download() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "report").await;
    send(
        application.clone(),
        "PUT",
        "/documents/report",
        Some(SAMPLE_DOCUMENT),
    )
    .await;
    let request = Request::builder()
        .method("GET")
        .uri("/documents/report/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"report.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>Swift Design</h1>")
    );
    let (status, _) = send(
        application.clone(),
        "GET",
        "/documents/report/pages/cover.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        application.clone(),
        "GET",
        "/documents/report/pages/9.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let request = Request::builder()
        .method("GET")
        .uri("/documents/report/export.docx")
        .body(Body::empty())
        .unwrap();
    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"report.docx\""
    );
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .contains("wordprocessingml")
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    // A ZIP starts with `PK`.
    assert_eq!(&bytes[..2], b"PK");
}

#[tokio::test]
async fn exporting_a_deck_returns_an_html_download() {
    let directory = TempDir::new().unwrap();
    let application = test_application(&directory);
    open_generating_session(&application, "talk").await;
    send(application.clone(), "PUT", "/decks/talk", Some(SAMPLE_DECK)).await;
    let request = Request::builder()
        .method("GET")
        .uri("/decks/talk/export")
        .body(Body::empty())
        .unwrap();
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"talk.html\""
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .contains("<h1>Swift Design</h1>")
    );
    let (status, _) = send(
        application.clone(),
        "GET",
        "/decks/talk/slides/cover.png",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(application, "GET", "/decks/talk/slides/9.png", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn exporting_a_design_returns_an_html_download() {
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
    let response = application
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/designs/overview/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .starts_with("<!doctype html>")
    );
}
