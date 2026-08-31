//! Renders a validated mailing to one standalone HTML page.
//!
//! Every email is a px canvas the size of the mailing's format, scaled
//! with CSS to fit its shell, so the same layout appears in
//! thumbnails, the editor, the PNGs, and the PDF.
//! Email `html` is inserted as written and email `css` is scoped to
//! the email. The page uses the same DOM vocabulary as a design page
//! (`main.design`, `data-swift-design-*`), so the layout, navigation,
//! editing, and audit scripts in `render.rs` serve both. A mailing has
//! no transition and no presenter, so nothing here is mailing-only
//! beyond the canvas size. Callers must run `Mailing::validate` first.

use design_model::{Email, Mailing};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, escape_html,
    print_stylesheet, script_markup, stylesheet,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// How to render a mailing email.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based email. Used by thumbnails and the
    /// editor preview, so the email never scrolls.
    pub only_email: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// email loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One email per PDF page, no scripts. Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole mailing as an HTML page.
pub fn render_mailing(mailing: &Mailing, is_editable: bool) -> String {
    render_mailing_with(
        mailing,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the mailing, or one email of it, per `options`.
pub fn render_mailing_with(mailing: &Mailing, options: RenderOptions) -> String {
    let (sections, email_styles) = email_markup(mailing, options.only_email);
    let script = email_script(&options);
    let (script_source, script_element) = script_markup(&script);
    let viewport = mailing.viewport();
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{email_styles}</style>\n</head>\n<body>\n\
         <main class=\"design mailing\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\">\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&mailing.title),
        fonts = google_fonts_link(&mailing.theme).unwrap_or_default(),
        style = stylesheet(&mailing.theme, viewport),
        width = viewport.width,
        height = viewport.height,
    )
}

/// The email sections and their scoped CSS, for every email or only
/// `only_email`.
fn email_markup(mailing: &Mailing, only_email: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut email_styles = String::new();
    for (index, email) in mailing.emails.iter().enumerate() {
        if only_email.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_email(email, index));
        if let Some(css) = &email.css {
            email_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            email_styles.push('\n');
        }
    }
    (sections, email_styles)
}

/// The email script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request. A mailing export carries the
/// fit alone, so the PDF holds the same content the studio shows.
fn email_script(options: &RenderOptions) -> String {
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

/// Renders one email: a full-window shell that centers the canvas box,
/// and the canvas-sized root that holds the email's HTML as written.
fn render_email(email: &Email, index: usize) -> String {
    format!(
        "<div class=\"email-frame\" id=\"email-{number}\" data-swift-design-frame>\n\
         <section class=\"email\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"email-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = email.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{EmailFormat, Mailing};
    use sha2::Digest;

    use crate::export::base64_encode;
    use crate::mailing_render::{RenderOptions, render_mailing, render_mailing_with};

    fn sample_mailing() -> Mailing {
        serde_json::from_str(include_str!("../../../fixtures/sample-mailing.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_email_with_the_html_as_written() {
        let mailing = sample_mailing();
        let html = render_mailing(&mailing, false);
        assert_eq!(
            html.matches("<div class=\"email-root\" data-swift-design-root>")
                .count(),
            mailing.emails.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"email-1\""));
        assert!(html.contains("<h1>Six kinds. One chat.</h1>"));
        assert!(html.contains("<main class=\"design mailing\" data-swift-design-width=\"600\" data-swift-design-height=\"1200\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 600px)))"));
        assert!(html.contains("aspect-ratio: 600 / 1200;"));
    }

    #[test]
    fn the_long_format_grows_the_canvas() {
        let mut mailing = sample_mailing();
        mailing.format = EmailFormat::Long;
        let html = render_mailing(&mailing, false);
        assert!(html.contains("data-swift-design-width=\"600\" data-swift-design-height=\"1800\""));
        assert!(html.contains("aspect-ratio: 600 / 1800;"));
    }

    #[test]
    fn email_css_is_scoped_to_its_email() {
        let html = render_mailing(&sample_mailing(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .e1-email{"));
        assert!(html.contains("[data-swift-design-screen=\"1\"] h2{"));
        assert!(!html.contains("\nh2{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_mailing(&sample_mailing(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_email_renders_with_its_real_index() {
        let html = render_mailing_with(
            &sample_mailing(),
            RenderOptions {
                only_email: Some(1),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"email\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"email\" data-swift-design-screen=\"1\">"));
        assert!(html.contains("id=\"email-2\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_auditing_email_carries_the_audit_script_and_a_matching_hash() {
        let html = render_mailing_with(
            &sample_mailing(),
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
        assert!(!render_mailing(&sample_mailing(), false).contains("swift-design-findings"));
    }

    #[test]
    fn print_mode_emits_page_rules_and_the_fit_script_only() {
        let mailing = sample_mailing();
        let html = render_mailing_with(
            &mailing,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 600px 1200px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("swiftDesignFit"));
        assert!(!html.contains("ResizeObserver"));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            mailing.emails.len()
        );
    }

    #[test]
    fn escapes_the_mailing_title() {
        let mut mailing = sample_mailing();
        mailing.title = "<script>alert(1)</script>".to_owned();
        let html = render_mailing(&mailing, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
