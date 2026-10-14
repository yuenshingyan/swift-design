#![allow(clippy::unwrap_used)]

use axum::body::to_bytes;
use axum::http::{StatusCode, header};
use design_model::Design;

use crate::export::{
    ExportScope, FONT_LINK_PREFIX, FontLink, attachment_disposition, base64_encode,
    build_deck_pdf_response, build_deck_zip_response, build_design_zip_response,
    build_document_pdf_response, build_document_zip_response, build_pptx_response,
    build_social_pdf_response, build_social_zip_response, file_download, google_fonts_link,
    inline_font_urls, inline_google_fonts, inline_uploaded_frame_images, inline_uploaded_images,
    inline_uploaded_page_images, inline_uploaded_slide_images, only_cover_index, pack_unit_images,
    upload_references,
};
use crate::pptx::ExportSources;
use crate::render;
use crate::test_support::{
    SAMPLE_DECK, SAMPLE_DESIGN, SAMPLE_DOCUMENT, sample_artwork, sample_deck, sample_document,
    sample_social, send, test_application,
};
use crate::uploads::UploadStore;

#[tokio::test]
async fn social_exports_without_chrome_are_503() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    let scope = ExportScope {
        id: "launch",
        only: None,
    };
    let response = build_social_pdf_response(scope, sample_social(), &store, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let scope = ExportScope {
        id: "launch",
        only: Some(1),
    };
    let response = build_social_pdf_response(scope, sample_social(), &store, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response =
        build_social_zip_response("launch", &sample_social(), "http://127.0.0.1:3000", None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn new_png_zip_exports_without_chrome_are_503() {
    let response =
        build_design_zip_response("overview", &sample_design(), "http://127.0.0.1:3000", None)
            .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("PNG exports need Chrome"));
    assert!(message.contains("SWIFT_DESIGN_CHROME"));

    let response =
        build_deck_zip_response("talk", &sample_deck(), "http://127.0.0.1:3000", None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response =
        build_document_zip_response("report", &sample_document(), "http://127.0.0.1:3000", None)
            .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn unit_images_pack_as_numbered_pngs_in_order() {
    let bytes = pack_unit_images(
        "overview",
        "screen",
        &[b"PNG1".to_vec(), b"PNG2".to_vec(), b"PNG3".to_vec()],
    )
    .unwrap();
    assert_eq!(&bytes[..2], b"PK");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 3);
    assert_eq!(archive.by_index(0).unwrap().name(), "overview-screen-1.png");
    assert_eq!(archive.by_index(1).unwrap().name(), "overview-screen-2.png");
    assert_eq!(archive.by_index(2).unwrap().name(), "overview-screen-3.png");
    let mut content = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_index(2).unwrap(), &mut content).unwrap();
    assert_eq!(content, b"PNG3");
}

#[tokio::test]
async fn zip_download_headers_name_the_attachment() {
    let response = file_download("overview", "zip", "application/zip", b"PK".to_vec());
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/zip"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"overview.zip\""
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.as_ref(), b"PK");
}

#[test]
fn a_scoped_export_stem_names_the_unit_like_a_zip_entry() {
    let scoped = ExportScope {
        id: "launch",
        only: Some(1),
    };
    assert_eq!(scoped.stem("cover"), "launch-cover-2");
    let whole = ExportScope {
        id: "launch",
        only: None,
    };
    assert_eq!(whole.stem("cover"), "launch");
}

#[test]
fn a_unit_number_converts_to_an_index_or_a_404() {
    let artwork = sample_artwork();
    assert_eq!(only_cover_index("launch", &artwork, None).unwrap(), None);
    assert_eq!(
        only_cover_index("launch", &artwork, Some(2)).unwrap(),
        Some(1)
    );
    let response = only_cover_index("launch", &artwork, Some(9)).unwrap_err();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = only_cover_index("launch", &artwork, Some(0)).unwrap_err();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn frame_images_pack_as_numbered_pngs() {
    let bytes = pack_unit_images("launch", "frame", &[b"PNG1".to_vec(), b"PNG2".to_vec()]).unwrap();
    assert_eq!(&bytes[..2], b"PK");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 2);
    assert_eq!(archive.by_index(0).unwrap().name(), "launch-frame-1.png");
    assert_eq!(archive.by_index(1).unwrap().name(), "launch-frame-2.png");
    let mut content = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_index(1).unwrap(), &mut content).unwrap();
    assert_eq!(content, b"PNG2");
}

#[tokio::test]
async fn inlines_uploaded_images_in_frames() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    store.save("launch", "logo.png", b"PNGDATA").await.unwrap();
    let mut social = sample_social();
    social.frames[0].html = "<img src='/uploads/logo.png'>".to_owned();
    social.frames[0].css = Some(".a { background: url('/uploads/logo.png'); }".to_owned());
    inline_uploaded_frame_images(&mut social, &store)
        .await
        .unwrap();
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert!(social.frames[0].html.contains(&expected));
    assert!(social.frames[0].css.as_deref().unwrap().contains(&expected));
    assert!(!social.frames[0].html.contains("/uploads/"));
}

#[tokio::test]
async fn document_pdf_export_without_chrome_is_503() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    let response = build_document_pdf_response("report", sample_document(), &store, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn inlines_uploaded_images_in_pages() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    store.save("report", "chart.png", b"PNGDATA").await.unwrap();
    let mut document = sample_document();
    document.pages[0].html = "<img src='/uploads/chart.png'>".to_owned();
    document.pages[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
    inline_uploaded_page_images(&mut document, &store)
        .await
        .unwrap();
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert!(document.pages[0].html.contains(&expected));
    assert!(
        document.pages[0]
            .css
            .as_deref()
            .unwrap()
            .contains(&expected)
    );
    assert!(!document.pages[0].html.contains("/uploads/"));
}

const STYLESHEET_URL: &str =
    "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap";
const FONT_CSS: &str = "@font-face { font-family: 'Inter'; font-style: normal; font-weight: 400; src: url(https://fonts.gstatic.com/s/inter/a.woff2) format('woff2'); }\n@font-face { font-family: 'Inter'; font-weight: 700; src: url(https://fonts.gstatic.com/s/inter/b.woff2) format('woff2'); }";

fn sample_design() -> Design {
    serde_json::from_str(include_str!("../../../../fixtures/sample-design.json")).unwrap()
}

async fn write_invalid_artifact(directory: &tempfile::TempDir, kind: &str, id: &str) {
    let path = directory.path().join(kind).join(format!("{id}.json"));
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    let mut value: serde_json::Value = match kind {
        "designs" => serde_json::from_str(SAMPLE_DESIGN).unwrap(),
        "decks" => serde_json::from_str(SAMPLE_DECK).unwrap(),
        "documents" => serde_json::from_str(SAMPLE_DOCUMENT).unwrap(),
        _ => unreachable!(),
    };
    match kind {
        "designs" => value["screens"] = serde_json::json!([]),
        "decks" => value["slides"] = serde_json::json!([]),
        "documents" => value["pages"] = serde_json::json!([]),
        _ => unreachable!(),
    }
    tokio::fs::write(path, serde_json::to_vec(&value).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn png_zip_routes_reject_invalid_ids() {
    let directory = tempfile::tempdir().unwrap();
    let application = test_application(&directory);
    let (status, _) = send(
        application.clone(),
        "GET",
        "/designs/Bad_Id/export.zip",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(application.clone(), "GET", "/decks/Bad_Id/export.zip", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(application, "GET", "/documents/Bad_Id/export.zip", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn png_zip_routes_404_for_missing_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    let application = test_application(&directory);
    let (status, _) = send(
        application.clone(),
        "GET",
        "/designs/missing/export.zip",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        application.clone(),
        "GET",
        "/decks/missing/export.zip",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(application, "GET", "/documents/missing/export.zip", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn png_zip_routes_reject_invalid_artifacts_before_chrome() {
    let directory = tempfile::tempdir().unwrap();
    write_invalid_artifact(&directory, "designs", "broken").await;
    write_invalid_artifact(&directory, "decks", "broken").await;
    write_invalid_artifact(&directory, "documents", "broken").await;
    let application = test_application(&directory);
    let (status, _) = send(
        application.clone(),
        "GET",
        "/designs/broken/export.zip",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(application.clone(), "GET", "/decks/broken/export.zip", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(application, "GET", "/documents/broken/export.zip", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// A fetcher with no network: the stylesheet URL returns `FONT_CSS`
/// and every font URL returns its own file name as bytes.
async fn fake_fetch(url: String) -> anyhow::Result<Vec<u8>> {
    if url.starts_with("https://fonts.googleapis.com/") {
        return Ok(FONT_CSS.as_bytes().to_vec());
    }
    let name = url.rsplit('/').next().unwrap_or_default();
    Ok(format!("FONT:{name}").into_bytes())
}

async fn failing_fetch(url: String) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("offline: {url}")
}

fn data_uri(name: &str) -> String {
    format!(
        "data:font/woff2;base64,{}",
        base64_encode(format!("FONT:{name}").as_bytes())
    )
}

#[test]
fn google_fonts_link_is_found_with_its_bounds() {
    let html = format!(
        "<head>\n<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link rel=\"stylesheet\" href=\"{STYLESHEET_URL}\">\n<style>"
    );
    let link = google_fonts_link(&html).unwrap();
    assert_eq!(
        &html[link.start..link.end],
        format!("<link rel=\"stylesheet\" href=\"{STYLESHEET_URL}\">")
    );
    assert_eq!(
        link,
        FontLink {
            start: link.start,
            end: link.end,
            url: STYLESHEET_URL.to_owned()
        }
    );
    assert!(google_fonts_link("<head><style></style></head>").is_none());
}

#[tokio::test]
async fn rewrite_replaces_one_font_url_with_a_data_uri() {
    let css = "@font-face { src: url(https://fonts.gstatic.com/s/inter/a.woff2) format('woff2'); }";
    let rewritten = inline_font_urls(css, &fake_fetch).await.unwrap();
    assert_eq!(
        rewritten,
        format!(
            "@font-face {{ src: url({}) format('woff2'); }}",
            data_uri("a.woff2")
        )
    );
}

#[tokio::test]
async fn rewrite_replaces_every_font_url() {
    let rewritten = inline_font_urls(FONT_CSS, &fake_fetch).await.unwrap();
    assert!(rewritten.contains(&data_uri("a.woff2")));
    assert!(rewritten.contains(&data_uri("b.woff2")));
    assert!(!rewritten.contains("fonts.gstatic.com"));
    assert_eq!(rewritten.matches("data:font/woff2;base64,").count(), 2);
}

#[tokio::test]
async fn rewrite_keeps_the_rest_of_the_css_unchanged() {
    let rewritten = inline_font_urls(FONT_CSS, &fake_fetch).await.unwrap();
    let restored = rewritten
        .replace(
            &data_uri("a.woff2"),
            "https://fonts.gstatic.com/s/inter/a.woff2",
        )
        .replace(
            &data_uri("b.woff2"),
            "https://fonts.gstatic.com/s/inter/b.woff2",
        );
    assert_eq!(restored, FONT_CSS);
    assert_eq!(
        inline_font_urls("p { color: red }", &fake_fetch)
            .await
            .unwrap(),
        "p { color: red }"
    );
}

#[tokio::test]
async fn a_fetch_error_keeps_the_original_html_and_link() {
    let html = render::render_design(&sample_design(), false);
    assert!(html.contains(FONT_LINK_PREFIX));
    let result = inline_google_fonts(html.clone(), failing_fetch).await;
    assert_eq!(result, html);
}

#[tokio::test]
async fn exported_html_holds_no_google_fonts_host_after_the_rewrite() {
    let html = render::render_design(&sample_design(), false);
    let result = inline_google_fonts(html, fake_fetch).await;
    assert!(!result.contains("fonts.googleapis.com"));
    assert!(result.contains("<style>\n@font-face"));
    assert!(!result.contains("rel=\"preconnect\""));
    assert!(result.contains("style-src 'unsafe-inline';"));
}

#[tokio::test]
async fn exported_html_holds_no_gstatic_host_after_the_rewrite() {
    let html = render::render_design(&sample_design(), false);
    let result = inline_google_fonts(html, fake_fetch).await;
    assert!(!result.contains("fonts.gstatic.com"));
    assert!(result.contains(&data_uri("a.woff2")));
    assert!(result.contains("font-src 'self' data:;"));
}

#[test]
fn attachment_disposition_names_the_design_file() {
    assert_eq!(
        attachment_disposition("overview", "pdf"),
        "attachment; filename=\"overview.pdf\""
    );
    assert_eq!(
        attachment_disposition("overview", "html"),
        "attachment; filename=\"overview.html\""
    );
    assert_eq!(
        attachment_disposition("overview", "pptx"),
        "attachment; filename=\"overview.pptx\""
    );
}

#[tokio::test]
async fn deck_pdf_export_without_chrome_is_503() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    let response = build_deck_pdf_response("overview", sample_deck(), &store, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn pptx_export_without_chrome_is_503() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    let sources = ExportSources {
        uploads: &store,
        base_url: "http://127.0.0.1:3000".to_owned(),
    };
    let response = build_pptx_response("overview", &sample_deck(), &sources, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("PPTX exports need Chrome"));
    assert!(message.contains("SWIFT_DESIGN_CHROME"));
}

#[tokio::test]
async fn inlines_uploaded_images_in_slides() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    store.save("talk", "chart.png", b"PNGDATA").await.unwrap();
    let mut deck = sample_deck();
    deck.slides[0].html = "<img src='/uploads/chart.png'>".to_owned();
    deck.slides[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
    inline_uploaded_slide_images(&mut deck, &store)
        .await
        .unwrap();
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert!(deck.slides[0].html.contains(&expected));
    assert!(deck.slides[0].css.as_deref().unwrap().contains(&expected));
    assert!(!deck.slides[0].html.contains("/uploads/"));
}

#[test]
fn base64_matches_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

#[tokio::test]
async fn inlines_uploaded_images_in_html_and_css() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    store.save("talk", "chart.png", b"PNGDATA").await.unwrap();

    let mut design = sample_design();
    design.screens[0].html =
        "<img src='/uploads/chart.png'><div style=\"background:url(/uploads/chart.png)\"></div>"
            .to_owned();
    design.screens[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
    inline_uploaded_images(&mut design, &store).await.unwrap();
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert_eq!(design.screens[0].html.matches(&expected).count(), 2);
    assert!(
        design.screens[0]
            .css
            .as_deref()
            .unwrap()
            .contains(&expected)
    );
    assert!(!design.screens[0].html.contains("/uploads/"));
}

#[tokio::test]
async fn keeps_missing_and_external_images_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    let mut design = sample_design();
    design.screens[0].html =
        "<img src='/uploads/absent.png'><a href='https://example.com/a.png'>x</a>".to_owned();
    inline_uploaded_images(&mut design, &store).await.unwrap();
    assert!(design.screens[0].html.contains("/uploads/absent.png"));
    assert!(design.screens[0].html.contains("https://example.com/a.png"));
}

#[test]
fn upload_references_are_found_with_their_bounds() {
    let references = upload_references("a /uploads/x.png b uploads/y-1.jpg c /uploads/");
    assert_eq!(references.len(), 2);
    assert_eq!(references[0].1, "x.png");
    assert_eq!(
        &"a /uploads/x.png"[references[0].0..references[0].2],
        "/uploads/x.png"
    );
    assert_eq!(references[1].1, "y-1.jpg");
}
