use axum::body::to_bytes;
use axum::http::{StatusCode, header};
use design_model::Design;

use crate::deck::pptx::ExportSources;
use crate::export::{
    ExportScope, FONT_LINK_PREFIX, FontLink, attachment_disposition, base64_encode,
    build_deck_pdf_response, build_deck_zip_response, build_design_zip_response,
    build_document_pdf_response, build_document_zip_response, build_pptx_response,
    build_social_pdf_response, build_social_zip_response, file_download, google_fonts_link,
    inline_font_urls, inline_google_fonts, inline_uploaded_frame_images, inline_uploaded_images,
    inline_uploaded_page_images, inline_uploaded_slide_images, only_cover_index,
    pack_design_images, pack_unit_images, selected_screen_indexes, upload_references,
};
use crate::test_support::{
    SAMPLE_DECK, SAMPLE_DESIGN, SAMPLE_DOCUMENT, sample_artwork, sample_deck, sample_document,
    sample_social, send, test_application,
};
use crate::uploads::UploadStore;

#[tokio::test]
async fn social_exports_without_chrome_are_503() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
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
    let response = build_design_zip_response(
        "overview",
        &sample_design(),
        "http://127.0.0.1:3000",
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("test response body should be readable as bytes");
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).expect("test JSON bytes should deserialize");
    let message = body["error"]["message"]
        .as_str()
        .expect("test JSON value should be a string");
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
fn screen_selections_support_ranges_and_keep_design_order_without_duplicates() {
    assert_eq!(
        selected_screen_indexes(None, 3).expect("test screen selection should be valid"),
        vec![0, 1, 2]
    );
    assert_eq!(
        selected_screen_indexes(Some(" 5, 2-4,3, 1 "), 5)
            .expect("test screen selection should be valid"),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        selected_screen_indexes(Some("23-35"), 35).expect("test screen selection should be valid"),
        (22..35).collect::<Vec<_>>()
    );
    assert_eq!(
        selected_screen_indexes(Some("2"), 3).expect("test screen selection should be valid"),
        vec![1]
    );
    assert_eq!(
        selected_screen_indexes(Some("3,1"), 3).expect("test screen selection should be valid"),
        vec![0, 2]
    );
    for selection in [
        "",
        " ",
        "0",
        "4",
        "3-1",
        "1-4",
        "-1",
        "1-",
        "1,,2",
        "1-2-3",
        "+1",
        "one",
        "999999999999999999999999999999999",
    ] {
        assert!(
            selected_screen_indexes(Some(selection), 3).is_err(),
            "{selection}"
        );
    }
    assert!(selected_screen_indexes(Some(&"1,".repeat(2049)), 3).is_err());
}

#[test]
fn selected_png_zip_preserves_original_screen_numbers_and_exact_content() {
    use std::io::Read;

    let bytes = pack_design_images(
        "demo",
        &[(22, b"SCREEN23".to_vec()), (34, b"SCREEN35".to_vec())],
    )
    .expect("selected test screens should pack into a ZIP");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .expect("test bytes should form a readable ZIP archive");
    assert_eq!(archive.len(), 2);
    assert_eq!(
        archive
            .by_index(0)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "demo-screen-23.png"
    );
    assert_eq!(
        archive
            .by_index(1)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "demo-screen-35.png"
    );
    let mut content = String::new();
    archive
        .by_name("demo-screen-35.png")
        .expect("ZIP should contain the named entry")
        .read_to_string(&mut content)
        .expect("test content should be readable as text");
    assert_eq!(content, "SCREEN35");
    assert!(archive.by_name("demo-screen-1.png").is_err());
}

#[tokio::test]
async fn invalid_screen_selection_is_rejected_before_screenshot_work() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = crate::design::core::DesignStore::new(directory.path().join("designs"));
    store
        .save("demo", &sample_design())
        .await
        .expect("test data should be saved successfully");
    let application = test_application(&directory);
    for selection in ["", "0", "999", "2-1", "1,,2", "bad"] {
        let (status, _) = send(
            application.clone(),
            "GET",
            &format!("/designs/demo/export.zip?screens={selection}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{selection}");
    }
}

#[tokio::test]
async fn selected_png_export_captures_only_the_requested_original_screen() {
    let chrome = crate::screenshots::find_chrome();
    if chrome.is_none() {
        return;
    }
    let design = sample_design();
    let selected = design.screens.len() - 1;
    let response = build_design_zip_response(
        "demo",
        &design,
        "http://127.0.0.1:3000",
        chrome,
        Some(&[selected]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zip");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("test response body should be readable as bytes");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .expect("test bytes should form a readable ZIP archive");
    assert_eq!(archive.len(), 1);
    let mut entry = archive
        .by_index(0)
        .expect("ZIP should contain the entry at the expected index");
    assert_eq!(entry.name(), format!("demo-screen-{}.png", selected + 1));
    use std::io::Read;
    let mut signature = [0; 8];
    entry
        .read_exact(&mut signature)
        .expect("test content should contain all requested bytes");
    assert_eq!(&signature, b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn parallel_selected_export_keeps_sparse_screens_in_original_order() {
    let chrome = crate::screenshots::find_chrome();
    if chrome.is_none() {
        return;
    }
    let mut design = sample_design();
    design.screens = vec![design.screens[0].clone(); 4];
    let response = build_design_zip_response(
        "demo",
        &design,
        "http://127.0.0.1:3000",
        chrome,
        Some(&[3, 0, 2]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("test response body should be readable as bytes");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .expect("test bytes should form a readable ZIP archive");
    assert_eq!(archive.len(), 3);
    for (entry, screen) in [1, 3, 4].into_iter().enumerate() {
        assert_eq!(
            archive
                .by_index(entry)
                .expect("ZIP should contain the entry at the expected index")
                .name(),
            format!("demo-screen-{screen}.png")
        );
    }
}

#[test]
fn unit_images_pack_as_numbered_pngs_in_order() {
    let bytes = pack_unit_images(
        "overview",
        "screen",
        &[b"PNG1".to_vec(), b"PNG2".to_vec(), b"PNG3".to_vec()],
    )
    .expect("test unit images should pack into a ZIP");
    assert_eq!(&bytes[..2], b"PK");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .expect("test bytes should form a readable ZIP archive");
    assert_eq!(archive.len(), 3);
    assert_eq!(
        archive
            .by_index(0)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "overview-screen-1.png"
    );
    assert_eq!(
        archive
            .by_index(1)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "overview-screen-2.png"
    );
    assert_eq!(
        archive
            .by_index(2)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "overview-screen-3.png"
    );
    let mut content = Vec::new();
    std::io::Read::read_to_end(
        &mut archive
            .by_index(2)
            .expect("ZIP should contain the entry at the expected index"),
        &mut content,
    )
    .expect("test content should be readable in full");
    assert_eq!(content, b"PNG3");
}

#[tokio::test]
async fn zip_download_headers_name_the_attachment() {
    let response = file_download("overview", "zip", "application/zip", b"PK".to_vec());
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("test collection should contain the requested entry"),
        "application/zip"
    );
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .expect("test collection should contain the requested entry"),
        "attachment; filename=\"overview.zip\""
    );
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("test response body should be readable as bytes");
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
    assert_eq!(
        only_cover_index("launch", &artwork, None)
            .expect("test cover selection should be in range"),
        None
    );
    assert_eq!(
        only_cover_index("launch", &artwork, Some(2))
            .expect("test cover selection should be in range"),
        Some(1)
    );
    let response = only_cover_index("launch", &artwork, Some(9))
        .expect_err("invalid unit number should return not found");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = only_cover_index("launch", &artwork, Some(0))
        .expect_err("invalid unit number should return not found");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn frame_images_pack_as_numbered_pngs() {
    let bytes = pack_unit_images("launch", "frame", &[b"PNG1".to_vec(), b"PNG2".to_vec()])
        .expect("test unit images should pack into a ZIP");
    assert_eq!(&bytes[..2], b"PK");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .expect("test bytes should form a readable ZIP archive");
    assert_eq!(archive.len(), 2);
    assert_eq!(
        archive
            .by_index(0)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "launch-frame-1.png"
    );
    assert_eq!(
        archive
            .by_index(1)
            .expect("ZIP should contain the entry at the expected index")
            .name(),
        "launch-frame-2.png"
    );
    let mut content = Vec::new();
    std::io::Read::read_to_end(
        &mut archive
            .by_index(1)
            .expect("ZIP should contain the entry at the expected index"),
        &mut content,
    )
    .expect("test content should be readable in full");
    assert_eq!(content, b"PNG2");
}

#[tokio::test]
async fn inlines_uploaded_images_in_frames() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    store
        .save("launch", "logo.png", b"PNGDATA")
        .await
        .expect("test data should be saved successfully");
    let mut social = sample_social();
    social.frames[0].html = "<img src='/uploads/logo.png'>".to_owned();
    social.frames[0].css = Some(".a { background: url('/uploads/logo.png'); }".to_owned());
    inline_uploaded_frame_images(&mut social, &store)
        .await
        .expect("test uploaded frame images should be embedded");
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert!(social.frames[0].html.contains(&expected));
    assert!(
        social.frames[0]
            .css
            .as_deref()
            .expect("optional test content should be present")
            .contains(&expected)
    );
    assert!(!social.frames[0].html.contains("/uploads/"));
}

#[tokio::test]
async fn document_pdf_export_without_chrome_is_503() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    let response = build_document_pdf_response("report", sample_document(), &store, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn inlines_uploaded_images_in_pages() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    store
        .save("report", "chart.png", b"PNGDATA")
        .await
        .expect("test data should be saved successfully");
    let mut document = sample_document();
    document.pages[0].html = "<img src='/uploads/chart.png'>".to_owned();
    document.pages[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
    inline_uploaded_page_images(&mut document, &store)
        .await
        .expect("test uploaded page images should be embedded");
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert!(document.pages[0].html.contains(&expected));
    assert!(
        document.pages[0]
            .css
            .as_deref()
            .expect("optional test content should be present")
            .contains(&expected)
    );
    assert!(!document.pages[0].html.contains("/uploads/"));
}

const STYLESHEET_URL: &str =
    "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap";
const FONT_CSS: &str = "@font-face { font-family: 'Inter'; font-style: normal; font-weight: 400; src: url(https://fonts.gstatic.com/s/inter/a.woff2) format('woff2'); }\n@font-face { font-family: 'Inter'; font-weight: 700; src: url(https://fonts.gstatic.com/s/inter/b.woff2) format('woff2'); }";

fn sample_design() -> Design {
    serde_json::from_str(include_str!("../../../../fixtures/sample-design.json"))
        .expect("test JSON should deserialize")
}

async fn write_invalid_artifact(directory: &tempfile::TempDir, kind: &str, id: &str) {
    let path = directory.path().join(kind).join(format!("{id}.json"));
    tokio::fs::create_dir_all(
        path.parent()
            .expect("test file path should have a parent directory"),
    )
    .await
    .expect("test directory tree should be created");
    let mut value: serde_json::Value = match kind {
        "designs" => serde_json::from_str(SAMPLE_DESIGN).expect("test JSON should deserialize"),
        "decks" => serde_json::from_str(SAMPLE_DECK).expect("test JSON should deserialize"),
        "documents" => serde_json::from_str(SAMPLE_DOCUMENT).expect("test JSON should deserialize"),
        _ => unreachable!(),
    };
    match kind {
        "designs" => value["screens"] = serde_json::json!([]),
        "decks" => value["slides"] = serde_json::json!([]),
        "documents" => value["pages"] = serde_json::json!([]),
        _ => unreachable!(),
    }
    tokio::fs::write(
        path,
        serde_json::to_vec(&value).expect("test value should serialize to JSON bytes"),
    )
    .await
    .expect("test fixture should be written successfully");
}

#[tokio::test]
async fn png_zip_routes_reject_invalid_ids() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
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
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
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
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
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
    let link =
        google_fonts_link(&html).expect("test HTML should contain a Google Fonts stylesheet link");
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
    let rewritten = inline_font_urls(css, &fake_fetch)
        .await
        .expect("test font URLs should be inlined");
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
    let rewritten = inline_font_urls(FONT_CSS, &fake_fetch)
        .await
        .expect("test font URLs should be inlined");
    assert!(rewritten.contains(&data_uri("a.woff2")));
    assert!(rewritten.contains(&data_uri("b.woff2")));
    assert!(!rewritten.contains("fonts.gstatic.com"));
    assert_eq!(rewritten.matches("data:font/woff2;base64,").count(), 2);
}

#[tokio::test]
async fn rewrite_keeps_the_rest_of_the_css_unchanged() {
    let rewritten = inline_font_urls(FONT_CSS, &fake_fetch)
        .await
        .expect("test font URLs should be inlined");
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
            .expect("test font URLs should be inlined"),
        "p { color: red }"
    );
}

#[tokio::test]
async fn a_fetch_error_keeps_the_original_html_and_link() {
    let html = crate::design::render::render_design(&sample_design(), false);
    assert!(html.contains(FONT_LINK_PREFIX));
    let result = inline_google_fonts(html.clone(), failing_fetch).await;
    assert_eq!(result, html);
}

#[tokio::test]
async fn exported_html_holds_no_google_fonts_host_after_the_rewrite() {
    let html = crate::design::render::render_design(&sample_design(), false);
    let result = inline_google_fonts(html, fake_fetch).await;
    assert!(!result.contains("fonts.googleapis.com"));
    assert!(result.contains("<style>\n@font-face"));
    assert!(!result.contains("rel=\"preconnect\""));
    assert!(result.contains("style-src 'unsafe-inline';"));
}

#[tokio::test]
async fn exported_html_holds_no_gstatic_host_after_the_rewrite() {
    let html = crate::design::render::render_design(&sample_design(), false);
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
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    let response = build_deck_pdf_response("overview", sample_deck(), &store, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn pptx_export_without_chrome_is_503() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    let sources = ExportSources {
        uploads: &store,
        base_url: "http://127.0.0.1:3000".to_owned(),
    };
    let response = build_pptx_response("overview", &sample_deck(), &sources, None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("test response body should be readable as bytes");
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).expect("test JSON bytes should deserialize");
    let message = body["error"]["message"]
        .as_str()
        .expect("test JSON value should be a string");
    assert!(message.starts_with("PPTX exports need Chrome"));
    assert!(message.contains("SWIFT_DESIGN_CHROME"));
}

#[tokio::test]
async fn inlines_uploaded_images_in_slides() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    store
        .save("talk", "chart.png", b"PNGDATA")
        .await
        .expect("test data should be saved successfully");
    let mut deck = sample_deck();
    deck.slides[0].html = "<img src='/uploads/chart.png'>".to_owned();
    deck.slides[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
    inline_uploaded_slide_images(&mut deck, &store)
        .await
        .expect("test uploaded slide images should be embedded");
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert!(deck.slides[0].html.contains(&expected));
    assert!(
        deck.slides[0]
            .css
            .as_deref()
            .expect("optional test content should be present")
            .contains(&expected)
    );
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
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    store
        .save("talk", "chart.png", b"PNGDATA")
        .await
        .expect("test data should be saved successfully");

    let mut design = sample_design();
    design.screens[0].html =
        "<img src='/uploads/chart.png'><div style=\"background:url(/uploads/chart.png)\"></div>"
            .to_owned();
    design.screens[0].css = Some(".a { background: url('/uploads/chart.png'); }".to_owned());
    inline_uploaded_images(&mut design, &store)
        .await
        .expect("test uploaded images should be embedded");
    let expected = format!("data:image/png;base64,{}", base64_encode(b"PNGDATA"));
    assert_eq!(design.screens[0].html.matches(&expected).count(), 2);
    assert!(
        design.screens[0]
            .css
            .as_deref()
            .expect("optional test content should be present")
            .contains(&expected)
    );
    assert!(!design.screens[0].html.contains("/uploads/"));
}

#[tokio::test]
async fn keeps_missing_and_external_images_untouched() {
    let directory = tempfile::tempdir().expect("temporary test directory should be created");
    let store = UploadStore::new(directory.path().to_path_buf());
    let mut design = sample_design();
    design.screens[0].html =
        "<img src='/uploads/absent.png'><a href='https://example.com/a.png'>x</a>".to_owned();
    inline_uploaded_images(&mut design, &store)
        .await
        .expect("test uploaded images should be embedded");
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
