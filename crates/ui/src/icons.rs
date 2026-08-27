//! Inline SVG glyphs for the chrome: chevrons, check, play, download,
//! pencil, dashed square, layout. Each is a small `stroke`-only path coloured
//! by `currentColor`, so the surrounding text colour drives it.

/// A left-pointing chevron, for back buttons.
pub(crate) const CHEVRON_LEFT: &str = "<svg width=\"12\" height=\"12\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M9 2.5L4.5 7L9 11.5\" stroke=\"currentColor\" \
stroke-width=\"1.4\" stroke-linecap=\"round\" stroke-linejoin=\"round\"></path></svg>";

/// A right-pointing chevron, for rows that open something.
pub(crate) const CHEVRON_RIGHT: &str = "<svg width=\"12\" height=\"12\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M5 2.5L9.5 7L5 11.5\" stroke=\"currentColor\" \
stroke-width=\"1.4\" stroke-linecap=\"round\" stroke-linejoin=\"round\"></path></svg>";

/// A down-pointing chevron, for dropdown triggers.
pub(crate) const CHEVRON_DOWN: &str = "<svg width=\"10\" height=\"10\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M2.5 5L7 9.5L11.5 5\" stroke=\"currentColor\" \
stroke-width=\"1.4\" stroke-linecap=\"round\" stroke-linejoin=\"round\"></path></svg>";

/// A check mark, for the saved state.
pub(crate) const CHECK: &str = "<svg width=\"11\" height=\"11\" viewBox=\"0 0 12 12\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M1.8 6.2l2.6 2.6 5.8-5.8\" stroke=\"currentColor\" \
stroke-width=\"1.3\" stroke-linecap=\"round\" stroke-linejoin=\"round\"></path></svg>";

/// A play triangle, for Present.
pub(crate) const PLAY: &str = "<svg width=\"12\" height=\"12\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M3 2.5l8 4.5-8 4.5V2.5z\" stroke=\"currentColor\" \
stroke-width=\"1.3\" stroke-linejoin=\"round\"></path></svg>";

/// A down arrow onto a tray, for exports.
pub(crate) const DOWNLOAD: &str = "<svg width=\"12\" height=\"12\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M7 2v7M4 6.2L7 9.2l3-3M2.6 11.5h8.8\" \
stroke=\"currentColor\" stroke-width=\"1.3\" stroke-linecap=\"round\" \
stroke-linejoin=\"round\"></path></svg>";

/// A paperclip, for the attach button.
pub(crate) const PAPERCLIP: &str = "<svg width=\"16\" height=\"16\" viewBox=\"0 0 24 24\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 \
0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48\" stroke=\"currentColor\" \
stroke-width=\"1.7\" stroke-linecap=\"round\" stroke-linejoin=\"round\"></path></svg>";

/// A desktop monitor, for the desktop canvas.
pub(crate) const MONITOR: &str = "<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" \
fill=\"none\" aria-hidden=\"true\"><rect x=\"2\" y=\"4\" width=\"20\" height=\"13\" rx=\"1.5\" \
stroke=\"currentColor\" stroke-width=\"1.6\"></rect><path d=\"M9 20h6M12 17v3\" \
stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\"></path></svg>";

/// A phone, for the phone canvas.
pub(crate) const PHONE: &str = "<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" \
fill=\"none\" aria-hidden=\"true\"><rect x=\"7\" y=\"2\" width=\"10\" height=\"20\" rx=\"2\" \
stroke=\"currentColor\" stroke-width=\"1.6\"></rect><path d=\"M10.75 18.5h2.5\" \
stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\"></path></svg>";

/// A tablet, for the tablet canvas.
pub(crate) const TABLET: &str = "<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" \
fill=\"none\" aria-hidden=\"true\"><rect x=\"4\" y=\"2.5\" width=\"16\" height=\"19\" rx=\"2\" \
stroke=\"currentColor\" stroke-width=\"1.6\"></rect><path d=\"M10.5 18.75h3\" \
stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\"></path></svg>";

/// A pencil, for rename.
#[allow(dead_code)]
pub(crate) const PENCIL: &str = "<svg width=\"11\" height=\"11\" viewBox=\"0 0 12 12\" \
fill=\"none\" aria-hidden=\"true\"><path d=\"M8.2 1.9l1.9 1.9-6 6-2.4.5.5-2.4 6-6z\" \
stroke=\"currentColor\" stroke-width=\"1.1\" stroke-linejoin=\"round\"></path></svg>";

/// A dashed square, for the empty node inspector.
pub(crate) const DASHED_SQUARE: &str = "<svg width=\"14\" height=\"14\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><rect x=\"1.5\" y=\"1.5\" width=\"11\" height=\"11\" rx=\"2\" \
stroke=\"currentColor\" stroke-width=\"1.2\" stroke-dasharray=\"2.4 2\"></rect></svg>";

/// A screen layout: a framed panel with a heading band and two panes,
/// for the template picker.
pub(crate) const LAYOUT: &str = "<svg width=\"14\" height=\"14\" viewBox=\"0 0 14 14\" \
fill=\"none\" aria-hidden=\"true\"><rect x=\"1.75\" y=\"2.25\" width=\"10.5\" \
height=\"9.5\" rx=\"1.6\" stroke=\"currentColor\" stroke-width=\"1.2\"></rect>\
<path d=\"M1.75 5.6h10.5M6.6 5.6v6.15\" stroke=\"currentColor\" stroke-width=\"1.2\" \
stroke-linecap=\"round\"></path></svg>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_one_hidden_svg_in_current_color() {
        for glyph in [
            CHEVRON_LEFT,
            CHEVRON_RIGHT,
            CHEVRON_DOWN,
            CHECK,
            PLAY,
            DOWNLOAD,
            PENCIL,
            DASHED_SQUARE,
            LAYOUT,
        ] {
            assert!(glyph.starts_with("<svg "));
            assert!(glyph.ends_with("</svg>"));
            assert!(glyph.contains("aria-hidden=\"true\""));
            assert!(glyph.contains("stroke=\"currentColor\""));
        }
    }
}
