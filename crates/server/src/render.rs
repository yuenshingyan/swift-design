//! Renders a validated design to one standalone HTML page.
//!
//! Every screen is a px canvas the size of the design's viewport, scaled
//! with CSS to fit its frame, so the same layout appears in thumbnails,
//! the editor, and screenshots. Screen `html` is inserted as written and
//! screen `css` is scoped to the screen. Validation (`design_model::markup`)
//! rejected unsafe markup before this point; a hash-based CSP is the
//! second layer. Callers must run `Design::validate` first.

use design_model::transition::MAX_TRANSITION_MS;
use design_model::{Design, Screen, Theme, Transition, TransitionEffect, Viewport};
use sha2::{Digest, Sha256};

use crate::export::base64_encode;
use crate::screen_css::{google_fonts_link, scope_css};

mod scripts;

pub(crate) use scripts::*;

/// How to render a design page.
#[derive(Clone, Debug, Default)]
pub struct RenderOptions {
    /// Nodes are selectable and text is editable in place; every change
    /// is posted to the parent window for the editor.
    pub is_editable: bool,
    /// Render only this zero-based screen. Used by thumbnails and the
    /// editor preview, so the page never scrolls.
    pub only_screen: Option<usize>,
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

/// Print rules appended after the base stylesheet, so they override it.
/// Every frame is one page the size of the canvas, so the root needs no
/// scaling at all.
pub(crate) fn print_stylesheet(viewport: Viewport) -> String {
    let width = viewport.width;
    let height = viewport.height;
    format!(
        "@page {{ size: {width}px {height}px; margin: 0; }}\n\
         html, body {{ height: auto; background: transparent; }}\n\
         * {{ -webkit-print-color-adjust: exact; print-color-adjust: exact; }}\n\
         main.design {{ height: auto; overflow: visible; scroll-snap-type: none; }}\n\
         main.design > [data-swift-design-frame] {{ width: {width}px; height: {height}px; display: block; overflow: hidden; }}\n\
         main.design > [data-swift-design-frame]:not(:last-child) {{ break-after: page; page-break-after: always; }}\n\
         [data-swift-design-screen] {{ width: {width}px; height: {height}px; container-type: normal; --swift-design-scale: 1; }}\n\
         [data-swift-design-root] {{ transform-origin: 0 0;\n\
           transform: scale(var(--swift-design-fit, 1)); }}\n"
    )
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
    for (index, screen) in design.screens.iter().enumerate() {
        if options.only_screen.is_some_and(|only| only != index) {
            continue;
        }
        sections.push_str(&render_screen(screen, index));
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
    let transition = (options.only_screen.is_none() && !options.is_print)
        .then_some(design.transition)
        .flatten();
    let mut main_attributes = String::new();
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
         <title>{title}</title>\n{fonts}<style>\n{style}{print_style}{transition_style}{screen_styles}</style>\n</head>\n<body>\n\
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
    script.push_str(NAVIGATION_SCRIPT);
    if options.is_editable {
        script.push_str(EDITING_SCRIPT);
    }
    if options.is_auditing {
        script.push_str(AUDIT_SCRIPT);
    }
    script
}

/// The `script-src` value and the `<script>` element for `script`. An
/// empty script allows no script at all and emits no element.
pub(crate) fn script_markup(script: &str) -> (String, String) {
    if script.is_empty() {
        return ("'none'".to_owned(), String::new());
    }
    (
        format!("'sha256-{}'", script_hash(script)),
        format!("<script>{script}</script>\n"),
    )
}

/// The `main.design` attributes that name the design's transition. The
/// navigation script reads the duration; the CSS reads the effect and
/// the axis.
pub(crate) fn design_attributes(transition: Transition) -> String {
    format!(
        " data-swift-design-effect=\"{effect}\" data-swift-design-axis=\"{axis}\" \
         data-swift-design-duration=\"{duration}\"",
        effect = transition.effect.as_str(),
        axis = transition.axis.as_str(),
        duration = transition_ms(transition),
    )
}

/// The transition duration the page uses: the design's value, capped, and
/// zero for `none`, which always cuts.
fn transition_ms(transition: Transition) -> u32 {
    match transition.effect {
        TransitionEffect::None => 0,
        _ => transition.duration_ms.min(MAX_TRANSITION_MS),
    }
}

/// The stacked-layout CSS for a transition.
///
/// Every frame sits on top of the others and stays hidden until it
/// carries a `data-swift-design-state`. The script moves a frame through
/// `entering` then `current`, which animates it from the effect's start
/// state to none, and marks the old frame `leaving`.
pub(crate) fn transition_styles(transition: Transition) -> String {
    let frame = "main.design[data-swift-design-effect] > [data-swift-design-frame]";
    let motion = "transition: opacity var(--swift-design-duration) ease, \
                  transform var(--swift-design-duration) ease;";
    let (entering, leaving) = match transition.effect {
        TransitionEffect::None => ("opacity: 0;", "opacity: 0;"),
        TransitionEffect::Fade => ("opacity: 0;", "opacity: 0;"),
        TransitionEffect::Push => (
            "opacity: 1; transform: var(--swift-design-in);",
            "opacity: 1; transform: var(--swift-design-out);",
        ),
        TransitionEffect::Cover => (
            "opacity: 1; transform: var(--swift-design-in);",
            "opacity: 1; transform: none;",
        ),
        TransitionEffect::Zoom => (
            "opacity: 0; transform: scale(0.88);",
            "opacity: 0; transform: scale(1.12);",
        ),
    };
    format!(
        "main.design[data-swift-design-effect] {{ position: relative; height: 100vh; overflow: hidden;\n\
           scroll-snap-type: none; --swift-design-sign: 1; --swift-design-duration: {duration}ms; }}\n\
         main.design[data-swift-design-axis=\"vertical\"] {{\n\
           --swift-design-in: translate3d(0, calc(var(--swift-design-sign) * 100%), 0);\n\
           --swift-design-out: translate3d(0, calc(var(--swift-design-sign) * -100%), 0); }}\n\
         main.design[data-swift-design-axis=\"horizontal\"] {{\n\
           --swift-design-in: translate3d(calc(var(--swift-design-sign) * 100%), 0, 0);\n\
           --swift-design-out: translate3d(calc(var(--swift-design-sign) * -100%), 0, 0); }}\n\
         {frame} {{ position: absolute; inset: 0; height: 100%; opacity: 0; visibility: hidden;\n\
           z-index: 0; will-change: opacity, transform; }}\n\
         {frame}[data-swift-design-state] {{ visibility: visible; }}\n\
         {frame}[data-swift-design-state=\"entering\"] {{ z-index: 2; {entering} }}\n\
         {frame}[data-swift-design-state=\"leaving\"] {{ z-index: 1; {leaving} {motion} }}\n\
         {frame}[data-swift-design-state=\"current\"] {{ z-index: 2; opacity: 1; transform: none; {motion} }}\n\
         @media (prefers-reduced-motion: reduce) {{\n\
           main.design[data-swift-design-effect] {{ --swift-design-duration: 0ms; }} }}\n",
        duration = transition_ms(transition),
    )
}

/// The base64 SHA-256 of the page script, for the CSP.
pub(crate) fn script_hash(script: &str) -> String {
    base64_encode(&Sha256::digest(script.as_bytes()))
}

/// Renders one screen: a full-window frame that centers the viewport
/// box, and the viewport-sized root that holds the screen's HTML as
/// written.
fn render_screen(screen: &Screen, index: usize) -> String {
    format!(
        "<div class=\"screen-frame\" id=\"screen-{number}\" data-swift-design-frame>\n\
         <section class=\"screen\" data-swift-design-screen=\"{index}\">\n\
         <div class=\"screen-root\" data-swift-design-root>{html}</div>\n\
         </section>\n</div>\n",
        number = index + 1,
        html = screen.html,
    )
}

/// Builds the base stylesheet: page chrome, the viewport-shaped frame,
/// the scaled viewport-sized root, theme variables, and small defaults
/// that screen CSS always overrides.
pub(crate) fn stylesheet(theme: &Theme, viewport: Viewport) -> String {
    let background = css_safe(&theme.colors.background);
    let text = css_safe(&theme.colors.text);
    let accent = css_safe(&theme.colors.accent);
    let muted = css_safe(&theme.colors.muted);
    let heading_font = css_safe(&theme.fonts.heading);
    let body_font = css_safe(&theme.fonts.body);
    let mono_font = css_safe(&theme.fonts.mono);
    let width = viewport.width;
    let height = viewport.height;
    format!(
        "html, body {{ margin: 0; height: 100%; background: #000; }}\n\
         main.design {{ height: 100vh; overflow-y: auto; scroll-snap-type: y mandatory; }}\n\
         main.design > [data-swift-design-frame] {{ height: 100vh; display: flex; align-items: center;\n\
           justify-content: center; scroll-snap-align: start; }}\n\
         [data-swift-design-screen] {{ position: relative; width: min(100vw, calc(100vh * {width} / {height}));\n\
           aspect-ratio: {width} / {height};\n\
           container-type: size; contain: layout paint; overflow: hidden; box-sizing: border-box;\n\
           background: {background};\n\
           --swift-design-scale: calc(tan(atan2(100cqw, {width}px)));\n\
           --background: {background}; --text: {text}; --accent: {accent}; --muted: {muted};\n\
           --heading-font: '{heading_font}', Inter, system-ui, sans-serif;\n\
           --body-font: '{body_font}', Inter, system-ui, sans-serif;\n\
           --mono-font: '{mono_font}', ui-monospace, monospace; }}\n\
         [data-swift-design-root] {{ position: relative; width: {width}px; height: {height}px; overflow: hidden;\n\
           box-sizing: border-box; transform-origin: 0 0;\n\
           transform: scale(calc(var(--swift-design-scale, 1) * var(--swift-design-fit, 1)));\n\
           background: var(--background); color: var(--text);\n\
           font: 32px/1.3 var(--body-font); }}\n\
         [data-swift-design-root] * {{ box-sizing: border-box; }}\n\
         [data-swift-design-inner-root] {{ width: 100% !important; height: 100% !important;\n\
           min-height: 0 !important; max-height: none !important; max-width: none !important; }}\n\
         [data-swift-design-root] :is(h1, h2, h3, h4, h5, h6) {{ font-family: var(--heading-font);\n\
           line-height: 1.1; margin: 0; letter-spacing: -0.02em; }}\n\
         [data-swift-design-root] :is(p, ul, ol, figure, blockquote) {{ margin: 0; }}\n\
         [data-swift-design-root] :is(ul, ol) {{ padding-left: 1.2em; }}\n\
         [data-swift-design-root] img {{ display: block; max-width: 100%; }}\n\
         [data-swift-design-root] :is(pre, code) {{ font-family: var(--mono-font); }}\n\
         [data-swift-design-root] a {{ color: var(--accent); }}\n\
         [data-swift-design-root] table {{ border-collapse: collapse; }}\n"
    )
}

/// Escapes text for HTML element and attribute positions.
pub(crate) fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Keeps only characters that cannot break out of a CSS declaration or
/// a CSP directive. Theme values come from agent-written JSON, so
/// quotes, braces, semicolons, and `</style>` sequences must never reach
/// the stylesheet.
pub(crate) fn css_safe(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, ' ' | '-' | '_' | '#' | ',' | ':' | '/' | '.')
        })
        .collect()
}

#[cfg(test)]
mod tests;
