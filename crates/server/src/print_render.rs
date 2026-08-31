//! Renders a validated print to one standalone HTML page.
//!
//! Every sheet is a px canvas the size of the print's size, rotated by
//! its orientation and scaled with CSS to fit its shell, so the same
//! layout appears in thumbnails, the editor, the PNGs, and the PDF.
//! Sheet `html` is inserted as written and sheet `css` is scoped to
//! the sheet. The page uses the same DOM vocabulary as a design page
//! (`main.design`, `data-swift-design-*`), so the layout, navigation,
//! editing, and audit scripts in `render.rs` serve both. A print has
//! no transition and no presenter, so nothing here is print-only
//! beyond the canvas size. Callers must run `Print::validate` first.

use design_model::{Print, Sheet};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, escape_html,
    print_stylesheet, script_markup, stylesheet,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// How to render a print sheet.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based sheet. Used by thumbnails and the
    /// editor preview, so the sheet never scrolls.
    pub only_sheet: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// sheet loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One sheet per PDF page, no scripts. Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole print as an HTML page.
pub fn render_print(print: &Print, is_editable: bool) -> String {
    render_print_with(
        print,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the print, or one sheet of it, per `options`.
pub fn render_print_with(print: &Print, options: RenderOptions) -> String {
    let (sections, sheet_styles) = sheet_markup(print, options.only_sheet);
    let script = sheet_script(&options);
    let (script_source, script_element) = script_markup(&script);
    let viewport = print.viewport();
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{sheet_styles}</style>\n</head>\n<body>\n\
         <main class=\"design print\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\">\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&print.title),
        fonts = google_fonts_link(&print.theme).unwrap_or_default(),
        style = stylesheet(&print.theme, viewport),
        width = viewport.width,
        height = viewport.height,
    )
}

/// The sheet sections and their scoped CSS, for every sheet or only
/// `only_sheet`.
fn sheet_markup(print: &Print, only_sheet: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut sheet_styles = String::new();
    for (index, sheet) in print.sheets.iter().enumerate() {
        if only_sheet.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_sheet(sheet, index));
        if let Some(css) = &sheet.css {
            sheet_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            sheet_styles.push('\n');
        }
    }
    (sections, sheet_styles)
}

/// The sheet script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request. A print export carries the
/// fit alone, so the PDF holds the same content the studio shows.
fn sheet_script(options: &RenderOptions) -> String {
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

/// Renders one sheet: a full-window shell that centers the canvas box,
/// and the canvas-sized root that holds the sheet's HTML as written.
fn render_sheet(sheet: &Sheet, index: usize) -> String {
    format!(
        "<div class=\"sheet-frame\" id=\"sheet-{number}\" data-swift-design-frame>\n\
         <section class=\"sheet\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"sheet-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = sheet.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Orientation, Print, PrintSize};
    use sha2::Digest;

    use crate::export::base64_encode;
    use crate::print_render::{RenderOptions, render_print, render_print_with};

    fn sample_print() -> Print {
        serde_json::from_str(include_str!("../../../fixtures/sample-print.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_sheet_with_the_html_as_written() {
        let print = sample_print();
        let html = render_print(&print, false);
        assert_eq!(
            html.matches("<div class=\"sheet-root\" data-swift-design-root>")
                .count(),
            print.sheets.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"sheet-1\""));
        assert!(html.contains("<h1>One harness. Five kinds.</h1>"));
        assert!(html.contains("<main class=\"design print\" data-swift-design-width=\"794\" data-swift-design-height=\"1123\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 794px)))"));
        assert!(html.contains("aspect-ratio: 794 / 1123;"));
    }

    #[test]
    fn the_landscape_orientation_swaps_the_canvas() {
        let mut print = sample_print();
        print.size = PrintSize::A3;
        print.orientation = Orientation::Landscape;
        let html = render_print(&print, false);
        assert!(
            html.contains("data-swift-design-width=\"1587\" data-swift-design-height=\"1123\"")
        );
        assert!(html.contains("aspect-ratio: 1587 / 1123;"));
    }

    #[test]
    fn sheet_css_is_scoped_to_its_sheet() {
        let html = render_print(&sample_print(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .s1-sheet{"));
        assert!(html.contains("[data-swift-design-screen=\"1\"] h2{"));
        assert!(!html.contains("\nh2{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_print(&sample_print(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_sheet_renders_with_its_real_index() {
        let html = render_print_with(
            &sample_print(),
            RenderOptions {
                only_sheet: Some(1),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"sheet\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"sheet\" data-swift-design-screen=\"1\">"));
        assert!(html.contains("id=\"sheet-2\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_auditing_sheet_carries_the_audit_script_and_a_matching_hash() {
        let html = render_print_with(
            &sample_print(),
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
        assert!(!render_print(&sample_print(), false).contains("swift-design-findings"));
    }

    #[test]
    fn print_mode_emits_page_rules_and_the_fit_script_only() {
        let print = sample_print();
        let html = render_print_with(
            &print,
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
            print.sheets.len()
        );
    }

    #[test]
    fn escapes_the_print_title() {
        let mut print = sample_print();
        print.title = "<script>alert(1)</script>".to_owned();
        let html = render_print(&print, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
