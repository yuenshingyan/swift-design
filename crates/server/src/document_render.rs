//! Renders a validated document to one standalone HTML page.
//!
//! Every page is a px canvas the size of the document's paper, scaled
//! with CSS to fit its frame, so the same layout appears in thumbnails,
//! the editor, and the PDF. Page `html` is inserted as written and page
//! `css` is scoped to the page. The page uses the same DOM vocabulary as
//! a design page (`main.design`, `data-swift-design-*`), so the layout,
//! navigation, editing, and audit scripts in `render.rs` serve both. A
//! document has no transition and no presenter, so nothing here is
//! document-only beyond the paper size. Callers must run
//! `Document::validate` first.

use design_model::{Document, Page};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, escape_html,
    print_stylesheet, script_markup, stylesheet,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// How to render a document page.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based page. Used by thumbnails and the
    /// editor preview, so the page never scrolls.
    pub only_page: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// page loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One page per paper sheet, no scripts. Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole document as an HTML page.
pub fn render_document(document: &Document, is_editable: bool) -> String {
    render_document_with(
        document,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the document, or one page of it, per `options`.
pub fn render_document_with(document: &Document, options: RenderOptions) -> String {
    let (sections, page_styles) = page_markup(document, options.only_page);
    let script = page_script(&options);
    let (script_source, script_element) = script_markup(&script);
    let viewport = document.viewport();
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{page_styles}</style>\n</head>\n<body>\n\
         <main class=\"design document\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\">\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&document.title),
        fonts = google_fonts_link(&document.theme).unwrap_or_default(),
        style = stylesheet(&document.theme, viewport),
        width = viewport.width,
        height = viewport.height,
    )
}

/// The page sections and their scoped CSS, for every page or only
/// `only_page`.
fn page_markup(document: &Document, only_page: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut page_styles = String::new();
    for (index, page) in document.pages.iter().enumerate() {
        if only_page.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_page(page, index));
        if let Some(css) = &page.css {
            page_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            page_styles.push('\n');
        }
    }
    (sections, page_styles)
}

/// The page script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request. A print carries the fit
/// alone, so the PDF holds the same content the studio shows.
fn page_script(options: &RenderOptions) -> String {
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

/// Renders one page: a full-window frame that centers the paper box, and
/// the paper-sized root that holds the page's HTML as written.
fn render_page(page: &Page, index: usize) -> String {
    format!(
        "<div class=\"page-frame\" id=\"page-{number}\" data-swift-design-frame>\n\
         <section class=\"page\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"page-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = page.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Document, Paper};
    use sha2::Digest;

    use crate::document_render::{RenderOptions, render_document, render_document_with};
    use crate::export::base64_encode;

    fn sample_document() -> Document {
        serde_json::from_str(include_str!("../../../fixtures/sample-document.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_page_with_the_html_as_written() {
        let document = sample_document();
        let html = render_document(&document, false);
        assert_eq!(
            html.matches("<div class=\"page-root\" data-swift-design-root>")
                .count(),
            document.pages.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"page-1\""));
        assert!(html.contains("<h1>Swift Design</h1>"));
        assert!(html.contains("<main class=\"design document\" data-swift-design-width=\"794\" data-swift-design-height=\"1123\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 794px)))"));
        assert!(html.contains("aspect-ratio: 794 / 1123;"));
    }

    #[test]
    fn letter_paper_sets_the_letter_canvas() {
        let mut document = sample_document();
        document.paper = Paper::Letter;
        let html = render_document(&document, false);
        assert!(html.contains("data-swift-design-width=\"816\" data-swift-design-height=\"1056\""));
        assert!(html.contains("aspect-ratio: 816 / 1056;"));
    }

    #[test]
    fn page_css_is_scoped_to_its_page() {
        let html = render_document(&sample_document(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .p1-cover{"));
        assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
        assert!(!html.contains("\nh2{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_document(&sample_document(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_page_renders_with_its_real_index() {
        let html = render_document_with(
            &sample_document(),
            RenderOptions {
                only_page: Some(2),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"page\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"page\" data-swift-design-screen=\"2\">"));
        assert!(html.contains("id=\"page-3\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_auditing_page_carries_the_audit_script_and_a_matching_hash() {
        let html = render_document_with(
            &sample_document(),
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
        assert!(!render_document(&sample_document(), false).contains("swift-design-findings"));
    }

    #[test]
    fn print_mode_emits_page_rules_and_the_fit_script_only() {
        let document = sample_document();
        let html = render_document_with(
            &document,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 794px 1123px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("swiftDesignFit"));
        assert!(!html.contains("ResizeObserver"));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            document.pages.len()
        );
    }

    #[test]
    fn escapes_the_document_title() {
        let mut document = sample_document();
        document.title = "<script>alert(1)</script>".to_owned();
        let html = render_document(&document, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
