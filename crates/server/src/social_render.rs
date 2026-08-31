//! Renders a validated social to one standalone HTML page.
//!
//! Every frame is a px canvas the size of the social's format, scaled
//! with CSS to fit its shell, so the same layout appears in thumbnails,
//! the editor, the PNGs, and the PDF. Frame `html` is inserted as
//! written and frame `css` is scoped to the frame. The page uses the
//! same DOM vocabulary as a design page (`main.design`,
//! `data-swift-design-*`), so the layout, navigation, editing, and
//! audit scripts in `render.rs` serve both. A social has no transition
//! and no presenter, so nothing here is social-only beyond the format
//! size. Callers must run `Social::validate` first.

use design_model::{Frame, Social};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, escape_html,
    print_stylesheet, script_markup, stylesheet,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// How to render a social frame.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based frame. Used by thumbnails and the
    /// editor preview, so the frame never scrolls.
    pub only_frame: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// frame loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One frame per PDF sheet, no scripts. Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole social as an HTML page.
pub fn render_social(social: &Social, is_editable: bool) -> String {
    render_social_with(
        social,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the social, or one frame of it, per `options`.
pub fn render_social_with(social: &Social, options: RenderOptions) -> String {
    let (sections, frame_styles) = frame_markup(social, options.only_frame);
    let script = frame_script(&options);
    let (script_source, script_element) = script_markup(&script);
    let viewport = social.viewport();
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{frame_styles}</style>\n</head>\n<body>\n\
         <main class=\"design social\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\">\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&social.title),
        fonts = google_fonts_link(&social.theme).unwrap_or_default(),
        style = stylesheet(&social.theme, viewport),
        width = viewport.width,
        height = viewport.height,
    )
}

/// The frame sections and their scoped CSS, for every frame or only
/// `only_frame`.
fn frame_markup(social: &Social, only_frame: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut frame_styles = String::new();
    for (index, frame) in social.frames.iter().enumerate() {
        if only_frame.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_frame(frame, index));
        if let Some(css) = &frame.css {
            frame_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            frame_styles.push('\n');
        }
    }
    (sections, frame_styles)
}

/// The frame script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request. A print carries the fit
/// alone, so the PDF holds the same content the studio shows.
fn frame_script(options: &RenderOptions) -> String {
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

/// Renders one frame: a full-window shell that centers the format box,
/// and the format-sized root that holds the frame's HTML as written.
fn render_frame(frame: &Frame, index: usize) -> String {
    format!(
        "<div class=\"frame-shell\" id=\"frame-{number}\" data-swift-design-frame>\n\
         <section class=\"frame\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"frame-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = frame.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Format, Social};
    use sha2::Digest;

    use crate::export::base64_encode;
    use crate::social_render::{RenderOptions, render_social, render_social_with};

    fn sample_social() -> Social {
        serde_json::from_str(include_str!("../../../fixtures/sample-social.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_frame_with_the_html_as_written() {
        let social = sample_social();
        let html = render_social(&social, false);
        assert_eq!(
            html.matches("<div class=\"frame-root\" data-swift-design-root>")
                .count(),
            social.frames.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"frame-1\""));
        assert!(html.contains("<h1>One harness. Four kinds.</h1>"));
        assert!(html.contains("<main class=\"design social\" data-swift-design-width=\"1080\" data-swift-design-height=\"1350\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 1080px)))"));
        assert!(html.contains("aspect-ratio: 1080 / 1350;"));
    }

    #[test]
    fn the_story_format_sets_the_story_canvas() {
        let mut social = sample_social();
        social.format = Format::Story;
        let html = render_social(&social, false);
        assert!(
            html.contains("data-swift-design-width=\"1080\" data-swift-design-height=\"1920\"")
        );
        assert!(html.contains("aspect-ratio: 1080 / 1920;"));
    }

    #[test]
    fn frame_css_is_scoped_to_its_frame() {
        let html = render_social(&sample_social(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .f1-frame{"));
        assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
        assert!(!html.contains("\nh2{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_social(&sample_social(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_frame_renders_with_its_real_index() {
        let html = render_social_with(
            &sample_social(),
            RenderOptions {
                only_frame: Some(2),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"frame\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"frame\" data-swift-design-screen=\"2\">"));
        assert!(html.contains("id=\"frame-3\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_auditing_frame_carries_the_audit_script_and_a_matching_hash() {
        let html = render_social_with(
            &sample_social(),
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
        assert!(!render_social(&sample_social(), false).contains("swift-design-findings"));
    }

    #[test]
    fn print_mode_emits_frame_rules_and_the_fit_script_only() {
        let social = sample_social();
        let html = render_social_with(
            &social,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 1080px 1350px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("swiftDesignFit"));
        assert!(!html.contains("ResizeObserver"));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            social.frames.len()
        );
    }

    #[test]
    fn escapes_the_social_title() {
        let mut social = sample_social();
        social.title = "<script>alert(1)</script>".to_owned();
        let html = render_social(&social, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
