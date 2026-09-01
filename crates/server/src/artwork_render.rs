//! Renders a validated artwork to one standalone HTML page.
//!
//! Every cover is a px canvas of the artwork's `size`, scaled
//! with CSS to fit its shell, so the same layout appears in
//! thumbnails, the editor, the PNGs, and the PDF.
//! Cover `html` is inserted as written and cover `css` is scoped to
//! the cover. The page uses the same DOM vocabulary as a design page
//! (`main.design`, `data-swift-design-*`), so the layout, navigation,
//! editing, and audit scripts in `render.rs` serve both. An artwork has
//! no transition and no presenter, so nothing here is artwork-only
//! beyond the canvas size. Callers must run `Artwork::validate` first.

use design_model::{Artwork, Cover};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, escape_html,
    print_stylesheet, script_markup, stylesheet,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// How to render an artwork cover.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based cover. Used by thumbnails and the
    /// editor preview, so the cover never scrolls.
    pub only_cover: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// cover loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One cover per PDF page, no scripts. Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole artwork as an HTML page.
pub fn render_artwork(artwork: &Artwork, is_editable: bool) -> String {
    render_artwork_with(
        artwork,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the artwork, or one cover of it, per `options`.
pub fn render_artwork_with(artwork: &Artwork, options: RenderOptions) -> String {
    let (sections, cover_styles) = cover_markup(artwork, options.only_cover);
    let script = cover_script(&options);
    let (script_source, script_element) = script_markup(&script);
    let viewport = artwork.viewport();
    let print_style = if options.is_print {
        print_stylesheet(viewport)
    } else {
        String::new()
    };
    let image_sources = match &options.asset_origin {
        Some(origin) => format!("'self' data: {}", css_safe(origin).replace(' ', "")),
        None => "'self' data:".to_owned(),
    };
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; \
         script-src {script_source}; style-src 'unsafe-inline' https://fonts.googleapis.com; \
         font-src 'self' https://fonts.gstatic.com; img-src {image_sources}; connect-src 'none'; \
         object-src 'none'; frame-src 'none'; form-action 'none'\">\n\
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{cover_styles}</style>\n</head>\n<body>\n\
         <main class=\"design artwork\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\">\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&artwork.title),
        fonts = google_fonts_link(&artwork.theme).unwrap_or_default(),
        style = stylesheet(&artwork.theme, viewport),
        width = viewport.width,
        height = viewport.height,
    )
}

/// The cover sections and their scoped CSS, for every cover or only
/// `only_cover`.
fn cover_markup(artwork: &Artwork, only_cover: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut cover_styles = String::new();
    for (index, cover) in artwork.covers.iter().enumerate() {
        if only_cover.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_cover(cover, index));
        if let Some(css) = &cover.css {
            cover_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            cover_styles.push('\n');
        }
    }
    (sections, cover_styles)
}

/// The cover script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request. An artwork export carries the
/// fit alone, so the PDF holds the same content the studio shows.
fn cover_script(options: &RenderOptions) -> String {
    if options.is_print {
        return crate::render::FIT_SCRIPT.to_owned();
    }
    let mut script = crate::render::FIT_SCRIPT.to_owned();
    script.push_str(LAYOUT_SCRIPT);
    script.push_str(NAVIGATION_SCRIPT);
    if options.is_editable {
        script.push_str(EDITING_SCRIPT);
    }
    if options.is_auditing {
        script.push_str(AUDIT_SCRIPT);
    }
    script
}

/// Renders one cover: a full-window shell that centers the canvas box,
/// and the canvas-sized root that holds the cover's HTML as written.
fn render_cover(cover: &Cover, index: usize) -> String {
    format!(
        "<div class=\"cover-frame\" id=\"cover-{number}\" data-swift-design-frame>\n\
         <section class=\"cover\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"cover-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = cover.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Artwork, CoverSize};
    use sha2::Digest;

    use crate::artwork_render::{RenderOptions, render_artwork, render_artwork_with};
    use crate::export::base64_encode;

    fn sample_artwork() -> Artwork {
        serde_json::from_str(include_str!("../../../fixtures/sample-artwork.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_cover_with_the_html_as_written() {
        let artwork = sample_artwork();
        let html = render_artwork(&artwork, false);
        assert_eq!(
            html.matches("<div class=\"cover-root\" data-swift-design-root>")
                .count(),
            artwork.covers.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"cover-1\""));
        assert!(html.contains("<h1>Seven kinds. One chat.</h1>"));
        assert!(html.contains("<main class=\"design artwork\" data-swift-design-width=\"1280\" data-swift-design-height=\"720\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 1280px)))"));
        assert!(html.contains("aspect-ratio: 1280 / 720;"));
    }

    #[test]
    fn the_banner_size_widens_the_canvas() {
        let mut artwork = sample_artwork();
        artwork.size = CoverSize::Banner;
        let html = render_artwork(&artwork, false);
        assert!(
            html.contains("data-swift-design-width=\"2560\" data-swift-design-height=\"1440\"")
        );
        assert!(html.contains("aspect-ratio: 2560 / 1440;"));
    }

    #[test]
    fn cover_css_is_scoped_to_its_cover() {
        let html = render_artwork(&sample_artwork(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .a1-cover{"));
        assert!(html.contains("[data-swift-design-screen=\"1\"] h1{"));
        assert!(!html.contains("\nh1{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_artwork(&sample_artwork(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_cover_renders_with_its_real_index() {
        let html = render_artwork_with(
            &sample_artwork(),
            RenderOptions {
                only_cover: Some(1),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"cover\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"cover\" data-swift-design-screen=\"1\">"));
        assert!(html.contains("id=\"cover-2\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_auditing_cover_carries_the_audit_script_and_a_matching_hash() {
        let html = render_artwork_with(
            &sample_artwork(),
            RenderOptions {
                is_auditing: true,
                asset_origin: Some("http://127.0.0.1:3000".to_owned()),
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("swift-design-findings"));
        assert!(html.contains("img-src 'self' data: http://127.0.0.1:3000;"));
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(!render_artwork(&sample_artwork(), false).contains("swift-design-findings"));
    }

    #[test]
    fn print_mode_emits_page_rules_and_the_fit_script_only() {
        let artwork = sample_artwork();
        let html = render_artwork_with(
            &artwork,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 1280px 720px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("swiftDesignFit"));
        assert!(!html.contains("ResizeObserver"));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            artwork.covers.len()
        );
    }

    #[test]
    fn escapes_the_artwork_title() {
        let mut artwork = sample_artwork();
        artwork.title = "<script>alert(1)</script>".to_owned();
        let html = render_artwork(&artwork, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
