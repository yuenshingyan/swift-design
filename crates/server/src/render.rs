//! Shared HTML escaping, styles, scripts, transitions, and CSP helpers for all artifacts.

use design_model::transition::MAX_TRANSITION_MS;
use design_model::{Theme, Transition, TransitionEffect, Viewport};
use sha2::{Digest, Sha256};

use crate::export::base64_encode;

mod scripts;
pub(crate) use scripts::*;

/// The transition duration, capped and zero for a cut.
fn transition_ms(transition: Transition) -> u32 {
    match transition.effect {
        TransitionEffect::None => 0,
        _ => transition.duration_ms.min(MAX_TRANSITION_MS),
    }
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
