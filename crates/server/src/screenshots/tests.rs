use super::routes::image_number;
use super::*;

#[test]
fn image_numbers_come_from_png_names() {
    assert_eq!(image_number("1.png"), Some(1));
    assert_eq!(image_number("12.png"), Some(12));
    assert_eq!(image_number("0.png"), None);
    assert_eq!(image_number("1.jpg"), None);
    assert_eq!(image_number("cover.png"), None);
}

#[test]
fn window_sizes_follow_the_viewport() {
    assert_eq!(window_size(Viewport::default()), "1440,900");
    assert_eq!(
        window_size(Viewport {
            width: 390,
            height: 844
        }),
        "390,844"
    );
}

#[test]
fn vision_support_follows_the_model_name() {
    assert!(supports_vision("gpt-5-mini"));
    assert!(supports_vision("claude-sonnet-5"));
    assert!(supports_vision("google/gemini-2.5-flash"));
    assert!(!supports_vision("llama-3.3-70b-versatile"));
    assert!(!supports_vision("deepseek/deepseek-chat"));
}

#[test]
fn base_href_lands_inside_head() {
    let html = "<!doctype html>\n<html><head>\n<title>x</title></head><body></body></html>";
    let result = with_base_href(html, "http://127.0.0.1:3000/");
    assert!(result.contains("<head>\n<base href=\"http://127.0.0.1:3000/\">"));
    assert!(with_base_href("<p>x</p>", "http://h").starts_with("<base href=\"http://h/\">"));
}

#[test]
fn a_fixed_canvas_pins_the_ad_to_the_top_left_at_its_px_size() {
    let html = "<!doctype html>\n<html><head>\n<style>a</style></head><body></body></html>";
    let result = with_fixed_canvas(html, design_model::LEADERBOARD_AD_VIEWPORT);
    assert!(result.contains("justify-content: flex-start"));
    assert!(result.contains("[data-swift-design-screen] { width: 728px; }</style>\n</head>"));
    // The override lands after the shell styles, so it wins.
    let shell = result.find("<style>a</style>").unwrap_or(usize::MAX);
    let fixed = result.find("flex-start").unwrap_or(0);
    assert!(shell < fixed);
}

#[tokio::test]
async fn scratch_paths_share_a_stem_and_carry_the_extension() {
    let (html_path, pdf_path) = scratch_paths("-print", "pdf")
        .await
        .unwrap_or_else(|error| {
            panic!("scratch paths: {error}");
        });
    assert!(html_path.to_string_lossy().ends_with("-print.html"));
    assert!(pdf_path.to_string_lossy().ends_with(".pdf"));
    assert_eq!(html_path.parent(), pdf_path.parent());
    let html_stem = html_path.to_string_lossy().replace("-print.html", "");
    let pdf_stem = pdf_path.to_string_lossy().replace(".pdf", "");
    assert_eq!(html_stem, pdf_stem);
}

#[tokio::test]
async fn chrome_missing_response_uses_the_shared_shape() {
    let response = chrome_missing_response("PDF exports");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|error| panic!("body: {error}"));
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("json: {error}"));
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("PDF exports need Chrome"));
    assert!(message.contains(CHROME_ENVIRONMENT_VARIABLE));
}
