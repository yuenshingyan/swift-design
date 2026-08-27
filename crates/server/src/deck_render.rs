//! Renders a validated deck to one standalone HTML page.
//!
//! Every slide is a 1920 by 1080 px canvas, scaled with CSS to fit its
//! 16:9 frame, so the same layout appears in thumbnails, the editor, the
//! presentation, and screenshots. Slide `html` is inserted as written and
//! slide `css` is scoped to the slide. The page uses the same DOM
//! vocabulary as a design page (`main.design`, `data-swift-design-*`),
//! so the layout, navigation, editing, and audit scripts in `render.rs`
//! serve both. What is deck-only lives here: the audience script that
//! follows a presenter window and the PPTX measurement hook. Callers
//! must run `Deck::validate` first.

use design_model::{DECK_VIEWPORT, Deck, Slide};

use crate::render::{
    AUDIT_SCRIPT, EDITING_SCRIPT, LAYOUT_SCRIPT, NAVIGATION_SCRIPT, css_safe, design_attributes,
    escape_html, print_stylesheet, script_markup, stylesheet, transition_styles,
};
use crate::screen_css::{google_fonts_link, scope_css};

/// Follows a presenter window. Loaded only with `audience=true`. The
/// presenter publishes `{type: 'swift-design-presenter', slide, sent_at}`
/// on a BroadcastChannel and in localStorage under the channel name.
/// This page reads the stored snapshot on load (a late joiner lands on
/// the current slide), listens to both, and says hello on the channel so
/// the presenter republishes. Neither API is governed by the CSP.
const AUDIENCE_SCRIPT: &str = r##"(() => {
  const channelName = design && design.dataset.swiftDesignChannel;
  if (!channelName || !frames.length) { return; }
  const apply = (message, isInstant) => {
    if (!message || message.type !== 'swift-design-presenter' || !Number.isInteger(message.slide)) { return; }
    const index = Math.min(Math.max(message.slide, 0), frames.length - 1);
    const current = effect ? shown : scrollIndex();
    show(index, index >= current ? 1 : -1, isInstant || Math.abs(index - current) > 1);
  };
  const parse = (text) => { try { return JSON.parse(text); } catch (error) { return null; } };
  try { apply(parse(localStorage.getItem(channelName)), true); } catch (error) {}
  window.addEventListener('storage', (event) => {
    if (event.key === channelName) { apply(parse(event.newValue)); }
  });
  if (window.BroadcastChannel) {
    const channel = new BroadcastChannel(channelName);
    channel.addEventListener('message', (event) => apply(event.data));
    channel.postMessage({ type: 'swift-design-audience-hello' });
  }
})();
"##;

/// How to render a deck page.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based slide. Used by thumbnails and the
    /// editor preview, so the page never scrolls.
    pub only_slide: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Adds the PPTX measurement script; the PPTX export reads its
    /// result from a DOM dump.
    pub is_measuring: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// page loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One slide per 1920 by 1080 page, no scripts, no transition. Used
    /// for `--print-to-pdf`.
    pub is_print: bool,
    /// Follow a presenter on this BroadcastChannel and localStorage key.
    /// The page ignores its own keys and wheel.
    pub audience_channel: Option<String>,
}

/// Renders the whole deck as an HTML document.
pub fn render_deck(deck: &Deck, is_editable: bool) -> String {
    render_deck_with(
        deck,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the deck, or one slide of it, per `options`.
pub fn render_deck_with(deck: &Deck, options: RenderOptions) -> String {
    let (sections, slide_styles) = slide_markup(deck, options.only_slide);
    let script = page_script(&options);
    let (script_source, script_element) = script_markup(&script);
    // A single-slide page has nothing to transition to, so the editor
    // preview, thumbnails, and screenshots keep the plain scroll page.
    // A print has one slide per page, so it never transitions either.
    let transition = (options.only_slide.is_none() && !options.is_print)
        .then_some(deck.transition)
        .flatten();
    let mut main_attributes = transition.map(design_attributes).unwrap_or_default();
    if let Some(channel) = &options.audience_channel {
        main_attributes.push_str(&channel_attribute(channel));
    }
    let transition_style = transition.map(transition_styles).unwrap_or_default();
    let print_style = if options.is_print {
        print_stylesheet(DECK_VIEWPORT)
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{transition_style}{slide_styles}</style>\n</head>\n<body>\n\
         <main class=\"design deck\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\"{main_attributes}>\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&deck.title),
        fonts = google_fonts_link(&deck.theme).unwrap_or_default(),
        style = stylesheet(&deck.theme, DECK_VIEWPORT),
        width = DECK_VIEWPORT.width,
        height = DECK_VIEWPORT.height,
    )
}

/// The slide sections and their scoped CSS, for every slide or only
/// `only_slide`.
fn slide_markup(deck: &Deck, only_slide: Option<usize>) -> (String, String) {
    let mut sections = String::new();
    let mut slide_styles = String::new();
    for (index, slide) in deck.slides.iter().enumerate() {
        if only_slide.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_slide(slide, index));
        if let Some(css) = &slide.css {
            slide_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            slide_styles.push('\n');
        }
    }
    (sections, slide_styles)
}

/// The page script for `options`: layout and navigation always, editing,
/// audit, measurement, and audience follow on request, and nothing at
/// all for a print.
fn page_script(options: &RenderOptions) -> String {
    if options.is_print {
        return String::new();
    }
    let mut script = LAYOUT_SCRIPT.to_owned();
    script.push_str(NAVIGATION_SCRIPT);
    if options.is_editable {
        script.push_str(EDITING_SCRIPT);
    }
    if options.is_auditing {
        script.push_str(AUDIT_SCRIPT);
    }
    if options.is_measuring {
        script.push_str(crate::pptx::MEASURE_SCRIPT);
    }
    if options.audience_channel.is_some() {
        script.push_str(AUDIENCE_SCRIPT);
    }
    script
}

/// The `main.design` attribute that names the presenter channel the page
/// follows.
fn channel_attribute(channel: &str) -> String {
    format!(" data-swift-design-channel=\"{}\"", escape_html(channel))
}

/// Renders one slide: a full-window frame that centers the 16:9 box, and
/// the 1920 by 1080 root that holds the slide's HTML as written.
fn render_slide(slide: &Slide, index: usize) -> String {
    format!(
        "<div class=\"slide-frame\" id=\"slide-{number}\" data-swift-design-frame>\n\
         <section class=\"slide\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"slide-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = slide.html,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Deck, Transition, TransitionAxis, TransitionEffect};
    use sha2::Digest;

    use crate::deck_render::{RenderOptions, render_deck, render_deck_with};
    use crate::export::base64_encode;

    fn sample_deck() -> Deck {
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json")).unwrap()
    }

    fn script_hash_of(html: &str) -> String {
        let start = html.find("<script>").unwrap() + "<script>".len();
        let end = html.find("</script>").unwrap();
        base64_encode(&sha2::Sha256::digest(&html.as_bytes()[start..end]))
    }

    #[test]
    fn renders_one_section_per_slide_with_the_html_as_written() {
        let deck = sample_deck();
        let html = render_deck(&deck, false);
        assert_eq!(
            html.matches("<div class=\"slide-root\" data-swift-design-root>")
                .count(),
            deck.slides.len()
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("id=\"slide-1\""));
        assert!(html.contains("<h1>Swift Design</h1>"));
        assert!(html.contains("<main class=\"design deck\" data-swift-design-width=\"1920\" data-swift-design-height=\"1080\">"));
        assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 1920px)))"));
        assert!(html.contains("aspect-ratio: 1920 / 1080;"));
    }

    #[test]
    fn slide_css_is_scoped_to_its_slide() {
        let html = render_deck(&sample_deck(), false);
        assert!(html.contains("[data-swift-design-screen=\"0\"] .s1-hero{"));
        assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
        assert!(!html.contains("\nh2{"));
    }

    #[test]
    fn the_csp_hash_matches_the_emitted_script() {
        let html = render_deck(&sample_deck(), true);
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(html.contains("swift-design-html"));
        assert!(html.contains("connect-src 'none'"));
    }

    #[test]
    fn a_single_slide_renders_with_its_real_index() {
        let html = render_deck_with(
            &sample_deck(),
            RenderOptions {
                only_slide: Some(2),
                ..RenderOptions::default()
            },
        );
        assert_eq!(
            html.matches("<section class=\"slide\" data-swift-design-screen=\"")
                .count(),
            1
        );
        assert!(html.contains("<section class=\"slide\" data-swift-design-screen=\"2\">"));
        assert!(html.contains("id=\"slide-3\""));
        assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
    }

    #[test]
    fn an_audience_page_carries_the_channel_and_the_follow_script() {
        let deck = sample_deck();
        let plain = render_deck(&deck, false);
        assert!(!plain.contains("data-swift-design-channel"));
        assert!(!plain.contains("BroadcastChannel"));
        // The keyboard guard is always in the script; the attribute
        // turns it on.
        assert!(plain.contains("isFollowing"));
        let audience = render_deck_with(
            &deck,
            RenderOptions {
                audience_channel: Some("swift-design-presenter:talk".to_owned()),
                ..RenderOptions::default()
            },
        );
        assert!(audience.contains(
            "<main class=\"design deck\" data-swift-design-width=\"1920\" data-swift-design-height=\"1080\" data-swift-design-channel=\"swift-design-presenter:talk\">"
        ));
        assert!(audience.contains("new BroadcastChannel(channelName)"));
        assert!(audience.contains("'storage'"));
        assert!(audience.contains("swift-design-audience-hello"));
        let hash = script_hash_of(&audience);
        assert!(audience.contains(&format!("script-src 'sha256-{hash}'")));
    }

    #[test]
    fn the_audience_channel_is_escaped_in_the_attribute() {
        let html = render_deck_with(
            &sample_deck(),
            RenderOptions {
                audience_channel: Some("x\"><script>".to_owned()),
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("data-swift-design-channel=\"x&quot;&gt;&lt;script&gt;\""));
        assert!(!html.contains("channel=\"x\"><script>"));
    }

    #[test]
    fn a_measuring_page_carries_the_measure_script_and_a_matching_hash() {
        let html = render_deck_with(
            &sample_deck(),
            RenderOptions {
                is_measuring: true,
                asset_origin: Some("http://127.0.0.1:3000".to_owned()),
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("swift-design-measure"));
        assert!(html.contains("img-src 'self' data: http://127.0.0.1:3000;"));
        let hash = script_hash_of(&html);
        assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
        assert!(!render_deck(&sample_deck(), false).contains("swift-design-measure"));
    }

    #[test]
    fn print_mode_emits_page_rules_and_no_scripts() {
        let mut deck = sample_deck();
        deck.transition = Some(Transition::default());
        let html = render_deck_with(
            &deck,
            RenderOptions {
                is_print: true,
                ..RenderOptions::default()
            },
        );
        assert!(html.contains("@page { size: 1920px 1080px; margin: 0; }"));
        assert!(html.contains("break-after: page"));
        assert!(html.contains("script-src 'none'"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("data-swift-design-effect="));
        assert_eq!(
            html.matches("data-swift-design-root>").count(),
            deck.slides.len()
        );
    }

    #[test]
    fn a_transition_adds_the_attributes_and_the_stacked_rules() {
        let mut deck = sample_deck();
        deck.transition = Some(Transition {
            effect: TransitionEffect::Cover,
            axis: TransitionAxis::Horizontal,
            duration_ms: 620,
        });
        let html = render_deck(&deck, false);
        assert!(html.contains("data-swift-design-effect=\"cover\""));
        assert!(html.contains("data-swift-design-axis=\"horizontal\""));
        assert!(html.contains("--swift-design-duration: 620ms"));
        let single = render_deck_with(
            &deck,
            RenderOptions {
                only_slide: Some(1),
                ..RenderOptions::default()
            },
        );
        assert!(!single.contains("data-swift-design-effect="));
    }

    #[test]
    fn escapes_the_deck_title() {
        let mut deck = sample_deck();
        deck.title = "<script>alert(1)</script>".to_owned();
        let html = render_deck(&deck, false);
        assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
    }
}
