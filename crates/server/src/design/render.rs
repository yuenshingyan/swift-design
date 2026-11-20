//! Renders a validated design to one standalone HTML page.
//!
//! Every screen is a px canvas the size of the design's viewport, scaled
//! with CSS to fit its frame, so the same layout appears in thumbnails,
//! the editor, and screenshots. Screen `html` is inserted as written and
//! screen `css` is scoped to the screen. Validation (`design_model::markup`)
//! rejected unsafe markup before this point; a hash-based CSP is the
//! second layer. Callers must run `Design::validate` first.

use design_model::{Design, Screen, Transition, TransitionEffect};

use crate::screen_css::{google_fonts_link, scope_css};

use crate::render::*;

const PROTOTYPE_SCRIPT: &str = include_str!("render/prototype.js");
const PROTOTYPE_STYLE: &str =
    "[data-swift-design-root] [data-demo-panel][hidden] { display: none !important; }\n";

/// How to render a design page.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based screen. Used by thumbnails and the
    /// editor preview, so the page never scrolls.
    pub only_screen: Option<usize>,
    /// Initial zero-based screen in the full interactive demo.
    pub start_screen: Option<usize>,
    /// Adds the layout audit script; the polish pass reads its result
    /// from a DOM dump.
    pub is_auditing: bool,
    /// Extra origin allowed for images, such as the server URL when the
    /// page loads from a file for a screenshot.
    pub asset_origin: Option<String>,
    /// One screen per viewport-sized page, no scripts, no transition.
    /// Used for `--print-to-pdf`.
    pub is_print: bool,
}

/// Renders the whole design as an HTML document.
pub fn render_design(design: &Design, is_editable: bool) -> String {
    render_design_with(
        design,
        RenderOptions {
            is_editable,
            ..RenderOptions::default()
        },
    )
}

/// Renders the design, or one screen of it, per `options`.
pub fn render_design_with(design: &Design, options: RenderOptions) -> String {
    let mut sections = String::new();
    let mut screen_styles = String::new();
    let start_screen = options.start_screen.filter(|&index| {
        options.only_screen.is_none() && !options.is_print && index < design.screens.len()
    });
    for (index, screen) in design.screens.iter().enumerate() {
        if options.only_screen.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_screen(screen, index, start_screen == Some(index)));
        if let Some(css) = &screen.css {
            screen_styles.push_str(&scope_css(
                css,
                &format!("[data-swift-design-screen=\"{index}\"]"),
            ));
            screen_styles.push('\n');
        }
    }
    let script = page_script(&options);
    let (script_source, script_element) = script_markup(&script);
    // A single-screen page has nothing to transition to, so the editor
    // preview, thumbnails, and screenshots keep the plain scroll page.
    // A print has one screen per page, so it never transitions either.
    let transition = (options.only_screen.is_none() && !options.is_print).then(|| {
        design.transition.unwrap_or(Transition {
            effect: TransitionEffect::None,
            ..Transition::default()
        })
    });
    let mut main_attributes = String::new();
    if options.is_auditing {
        main_attributes.push_str(" data-swift-design-prototype-audit");
    }
    if let Some(app_window) = &design.app_window {
        main_attributes.push_str(&format!(
            " data-swift-design-window-left=\"{left}\" data-swift-design-window-top=\"{top}\" \
             data-swift-design-window-width=\"{width}\" data-swift-design-window-height=\"{height}\"",
            left = app_window.left,
            top = app_window.top,
            width = app_window.width,
            height = app_window.height,
        ));
    }
    if let Some(transition) = transition {
        main_attributes.push_str(&design_attributes(transition));
        if transition.effect == TransitionEffect::None {
            // Instant app navigation uses links/keys, not scrolling between screens.
            main_attributes.push_str(" data-swift-design-wheel-navigation=\"false\"");
        }
    }
    let transition_style = transition.map(transition_styles).unwrap_or_default();
    let print_style = if options.is_print {
        print_stylesheet(design.viewport)
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{transition_style}{screen_styles}{PROTOTYPE_STYLE}</style>\n</head>\n<body>\n\
         <main class=\"design\" data-swift-design-width=\"{width}\" data-swift-design-height=\"{height}\"{main_attributes}>\n{sections}</main>\n\
         {script_element}</body>\n</html>\n",
        title = escape_html(&design.title),
        fonts = google_fonts_link(&design.theme).unwrap_or_default(),
        style = stylesheet(&design.theme, design.viewport),
        width = design.viewport.width,
        height = design.viewport.height,
    )
}

/// The page script for `options`: the fit always, layout and navigation
/// on screen, editing and audit on request.
///
/// A print carries the app-window geometry and the fit only: the PDF
/// must hold the same content the studio shows, and the rest of the
/// scripts have no place in a print.
fn page_script(options: &RenderOptions) -> String {
    let mut script = DESIGN_WINDOW_SCRIPT.to_owned();
    script.push_str(FIT_SCRIPT);
    if options.is_print {
        return script;
    }
    script.push_str(LAYOUT_SCRIPT);
    if !options.is_editable {
        script.push_str(PROTOTYPE_SCRIPT);
    }
    script.push_str(NAVIGATION_SCRIPT);
    if options.is_editable {
        script.push_str(EDITING_SCRIPT);
    }
    if options.is_auditing {
        script.push_str(AUDIT_SCRIPT);
    }
    script
}

/// Renders one screen: a full-window frame that centers the viewport
/// box, and the viewport-sized root that holds the screen's HTML as
/// written.
fn render_screen(screen: &Screen, index: usize, is_initial: bool) -> String {
    let state = if is_initial {
        " data-swift-design-state=\"current\""
    } else {
        ""
    };
    format!(
        "<div class=\"screen-frame\" id=\"screen-{number}\" data-swift-design-frame{state}>\n\
         <section class=\"screen\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"screen-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = screen.html,
    )
}

#[cfg(test)]
mod tests;
