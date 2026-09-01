//! Renders a validated campaign to one standalone HTML page.
//!
//! Every ad is a px canvas of the campaign's `size`, scaled
//! with CSS to fit its shell, so the same layout appears in
//! thumbnails, the editor, the PNGs, and the PDF.
//! Ad `html` is inserted as written and ad `css` is scoped to
//! the ad. The page uses the same DOM vocabulary as a design page
//! (`main.design`, `data-swift-design-*`), so the layout, navigation,
//! editing, and audit scripts in `render.rs` serve both. A campaign has
//! no transition and no presenter, so nothing here is campaign-only
//! beyond the canvas size. Callers must run `Campaign::validate` first.

use design_model::{Ad, Campaign};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, escape_html,
    print_stylesheet, script_markup, stylesheet,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// How to render a campaign ad.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based ad. Used by thumbnails and the
    /// editor preview, so the ad never scrolls.
    pub only_ad: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// ad loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One ad per PDF page, no scripts. Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole campaign as an HTML page.
pub fn render_campaign(campaign: &Campaign, is_editable: bool) -> String {
    render_campaign_with(
        campaign,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the campaign, or one ad of it, per `options`.
pub fn render_campaign_with(campaign: &Campaign, options: RenderOptions) -> String {
    let (sections, ad_styles) = ad_markup(campaign, options.only_ad);
    let script = ad_script(&options);
    let (script_source, script_element) = script_markup(&script);
    let viewport = campaign.viewport();
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{ad_styles}</style>\n</head>\n<body>\n\
         <main class=\"design campaign\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\">\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&campaign.title),
        fonts = google_fonts_link(&campaign.theme).unwrap_or_default(),
        style = stylesheet(&campaign.theme, viewport),
        width = viewport.width,
        height = viewport.height,
    )
}

/// The ad sections and their scoped CSS, for every ad or only
/// `only_ad`.
fn ad_markup(campaign: &Campaign, only_ad: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut ad_styles = String::new();
    for (index, ad) in campaign.ads.iter().enumerate() {
        if only_ad.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_ad(ad, index));
        if let Some(css) = &ad.css {
            ad_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            ad_styles.push('\n');
        }
    }
    (sections, ad_styles)
}

/// The ad script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request. A campaign export carries the
/// fit alone, so the PDF holds the same content the studio shows.
fn ad_script(options: &RenderOptions) -> String {
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

/// Renders one ad: a full-window shell that centers the canvas box,
/// and the canvas-sized root that holds the ad's HTML as written.
fn render_ad(ad: &Ad, index: usize) -> String {
    format!(
        "<div class=\"ad-frame\" id=\"ad-{number}\" data-swift-design-frame>\n\
         <section class=\"ad\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"ad-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = ad.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{AdSize, Campaign};
    use sha2::Digest;

    use crate::campaign_render::{RenderOptions, render_campaign, render_campaign_with};
    use crate::export::base64_encode;

    fn sample_campaign() -> Campaign {
        serde_json::from_str(include_str!("../../../fixtures/sample-campaign.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_ad_with_the_html_as_written() {
        let campaign = sample_campaign();
        let html = render_campaign(&campaign, false);
        assert_eq!(
            html.matches("<div class=\"ad-root\" data-swift-design-root>")
                .count(),
            campaign.ads.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"ad-1\""));
        assert!(html.contains("<h1>Seven kinds. One chat.</h1>"));
        assert!(html.contains("<main class=\"design campaign\" data-swift-design-width=\"300\" data-swift-design-height=\"250\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 300px)))"));
        assert!(html.contains("aspect-ratio: 300 / 250;"));
    }

    #[test]
    fn the_leaderboard_size_widens_the_canvas() {
        let mut campaign = sample_campaign();
        campaign.size = AdSize::Leaderboard;
        let html = render_campaign(&campaign, false);
        assert!(html.contains("data-swift-design-width=\"728\" data-swift-design-height=\"90\""));
        assert!(html.contains("aspect-ratio: 728 / 90;"));
    }

    #[test]
    fn ad_css_is_scoped_to_its_ad() {
        let html = render_campaign(&sample_campaign(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .a1-ad{"));
        assert!(html.contains("[data-swift-design-screen=\"1\"] h1{"));
        assert!(!html.contains("\nh1{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_campaign(&sample_campaign(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_ad_renders_with_its_real_index() {
        let html = render_campaign_with(
            &sample_campaign(),
            RenderOptions {
                only_ad: Some(1),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"ad\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"ad\" data-swift-design-screen=\"1\">"));
        assert!(html.contains("id=\"ad-2\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_auditing_ad_carries_the_audit_script_and_a_matching_hash() {
        let html = render_campaign_with(
            &sample_campaign(),
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
        assert!(!render_campaign(&sample_campaign(), false).contains("swift-design-findings"));
    }

    #[test]
    fn print_mode_emits_page_rules_and_the_fit_script_only() {
        let campaign = sample_campaign();
        let html = render_campaign_with(
            &campaign,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 300px 250px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("swiftDesignFit"));
        assert!(!html.contains("ResizeObserver"));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            campaign.ads.len()
        );
    }

    #[test]
    fn escapes_the_campaign_title() {
        let mut campaign = sample_campaign();
        campaign.title = "<script>alert(1)</script>".to_owned();
        let html = render_campaign(&campaign, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
