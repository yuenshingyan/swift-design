//! Serves the Swift Design tab icon.
//!
//! Safari reads neither a `data:` URI nor an SVG favicon, so the icon
//! ships as two files under `assets/`: an ICO that every browser
//! reads, and an SVG for the browsers that prefer one. Safari also
//! asks for `/favicon.ico` on its own, with no link element involved.
//!
//! `assets/favicon.svg` is the source. It draws the same path as the
//! brand mark in the `ui` crate, and a test there holds the two
//! together. Rebuild `assets/favicon.ico` from the SVG whenever the
//! mark changes: render it at 64 by 64 pixels on a transparent
//! background, then wrap that PNG in an ICO container.

use axum::Router;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// The tab icon as an ICO holding one 64 by 64 pixel image.
const FAVICON_ICO: &[u8] = include_bytes!("../../../assets/favicon.ico");

/// The tab icon as an SVG, for browsers that read one.
const FAVICON_SVG: &str = include_str!("../../../assets/favicon.svg");

/// How long a browser may hold the icon before it asks again.
const ICON_CACHE_CONTROL: &str = "public, max-age=86400";

/// The icon routes.
pub fn routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/favicon.ico", get(serve_ico))
        .route("/favicon.svg", get(serve_svg))
}

/// Serves the ICO icon.
async fn serve_ico() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/x-icon"),
            (header::CACHE_CONTROL, ICON_CACHE_CONTROL),
        ],
        FAVICON_ICO,
    )
        .into_response()
}

/// Serves the SVG icon.
async fn serve_svg() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, ICON_CACHE_CONTROL),
        ],
        FAVICON_SVG,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ico_holds_one_64_pixel_image() {
        // ICONDIR: two reserved bytes, type 1, then the image count.
        assert_eq!(&FAVICON_ICO[0..6], &[0, 0, 1, 0, 1, 0]);
        // ICONDIRENTRY: width and height, both 64.
        assert_eq!(&FAVICON_ICO[6..8], &[64, 64]);
        // The one image is a PNG.
        assert_eq!(&FAVICON_ICO[22..26], b"\x89PNG");
    }

    #[test]
    fn the_svg_draws_one_black_path() {
        assert!(FAVICON_SVG.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(FAVICON_SVG.contains("fill=\"#15181C\""));
        assert_eq!(FAVICON_SVG.matches("<path").count(), 1);
    }
}
