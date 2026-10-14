#![allow(clippy::unwrap_used)]

use design_model::{Design, Transition, TransitionAxis, TransitionEffect};
use sha2::Digest;

use crate::export::base64_encode;
use crate::render::{
    AUDIT_SCRIPT, DESIGN_WINDOW_SCRIPT, EDITING_SCRIPT, FIT_SCRIPT, MAX_TRANSITION_MS,
    NAVIGATION_SCRIPT, RenderOptions, css_safe, escape_html, render_design, render_design_with,
};

fn sample_design() -> Design {
    serde_json::from_str(include_str!("../../../../fixtures/sample-design.json")).unwrap()
}

#[test]
fn a_screen_link_opens_the_screen_in_play_mode_and_only_selects_in_edit_mode() {
    assert!(NAVIGATION_SCRIPT.contains("/^#screen-(\\d+)$/"));
    assert!(NAVIGATION_SCRIPT.contains("type: 'swift-design-navigate', target"));
    assert!(EDITING_SCRIPT.contains("closest('a[href]')) { event.preventDefault(); }"));
    // The frame ids are the link targets, so a full page scrolls to
    // them by itself.
    let html = render_design(&sample_design(), false);
    assert!(html.contains("id=\"screen-2\""));
}

#[test]
fn renders_one_section_per_screen_with_the_html_as_written() {
    let design = sample_design();
    let html = render_design(&design, false);
    assert_eq!(
        html.matches("<div class=\"screen-root\" data-swift-design-root>")
            .count(),
        design.screens.len()
    );
    assert!(html.starts_with("<!doctype html>"));
    assert!(html.contains("id=\"screen-1\""));
    assert!(html.contains("<h1>Swift Design</h1>"));
    assert!(html.contains("<ul><li>Agents read the schema and write design JSON.</li>"));
}

#[test]
fn screen_css_is_scoped_to_its_screen() {
    let html = render_design(&sample_design(), false);
    assert!(html.contains("[data-swift-design-screen=\"0\"] .s1-hero{"));
    assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
    assert!(!html.contains("\nh2{"));
}

#[test]
fn theme_variables_fonts_and_scaling_are_present() {
    let html = render_design(&sample_design(), false);
    assert!(html.contains("--background: #101418"));
    assert!(html.contains("--heading-font: 'Inter'"));
    assert!(html.contains("--mono-font: 'JetBrains Mono'"));
    assert!(html.contains("fonts.googleapis.com/css2?family=Inter"));
    assert!(html.contains("--swift-design-scale: calc(tan(atan2(100cqw, 1440px)))"));
    assert!(html.contains("aspect-ratio: 1440 / 900;"));
    assert!(html.contains("data-swift-design-width=\"1440\" data-swift-design-height=\"900\""));
    // The page scale and the content fit multiply into one transform.
    assert!(html.contains(
        "transform: scale(calc(var(--swift-design-scale, 1) * var(--swift-design-fit, 1)))"
    ));
    assert!(html.contains("new ResizeObserver"));
}

#[test]
fn the_csp_hash_matches_the_emitted_script() {
    let html = render_design(&sample_design(), true);
    let start = html.find("<script>").unwrap() + "<script>".len();
    let end = html.find("</script>").unwrap();
    let script = &html[start..end];
    let hash = base64_encode(&sha2::Sha256::digest(script.as_bytes()));
    assert!(html.contains(&format!("script-src 'sha256-{hash}'")));
    assert!(html.contains("connect-src 'none'"));
    assert!(html.contains("img-src 'self' data:;"));
}

#[test]
fn a_single_screen_renders_with_its_real_index() {
    let html = render_design_with(
        &sample_design(),
        RenderOptions {
            is_editable: true,
            only_screen: Some(2),
            ..RenderOptions::default()
        },
    );
    assert_eq!(
        html.matches("<section class=\"screen\" data-swift-design-screen=\"")
            .count(),
        1
    );
    assert!(html.contains("<section class=\"screen\" data-swift-design-screen=\"2\">"));
    assert!(html.contains("id=\"screen-3\""));
    // Only that screen's CSS is emitted.
    assert!(html.contains("[data-swift-design-screen=\"2\"] h2{"));
    assert!(!html.contains("[data-swift-design-screen=\"0\"]"));
}

#[test]
fn undo_and_redo_ship_with_the_editing_script() {
    assert!(EDITING_SCRIPT.contains("UNDO_LIMIT = 50"));
    assert!(EDITING_SCRIPT.contains("recordSnapshot("));
    assert!(EDITING_SCRIPT.contains("recordCoalesced("));
    assert!(EDITING_SCRIPT.contains("stacksOf"));
    assert!(EDITING_SCRIPT.contains("stopImmediatePropagation"));
    assert!(EDITING_SCRIPT.contains("event.shiftKey"));
    assert!(EDITING_SCRIPT.contains("redo()"));
}

#[test]
fn arrow_keys_nudge_the_selected_node() {
    assert!(EDITING_SCRIPT.contains("NUDGE_STEP = 1"));
    assert!(EDITING_SCRIPT.contains("NUDGE_SHIFT_STEP = 10"));
    assert!(EDITING_SCRIPT.contains("NUDGE_SAVE_DELAY_MS"));
    assert!(EDITING_SCRIPT.contains("ArrowUp"));
    assert!(EDITING_SCRIPT.contains("isNudgeActive"));
}

#[test]
fn snap_guides_ship_with_the_editing_script() {
    assert!(EDITING_SCRIPT.contains("swift-design-guide"));
    assert!(EDITING_SCRIPT.contains("SNAP_THRESHOLD = 6"));
    assert!(EDITING_SCRIPT.contains("SNAP_SIBLING_LIMIT = 24"));
    assert!(EDITING_SCRIPT.contains("snapLinesOf"));
    assert!(EDITING_SCRIPT.contains("hideGuides"));
}

#[test]
fn no_selection_toolbar_ships_with_the_editing_script() {
    assert!(!EDITING_SCRIPT.contains("swift-design-toolbar"));
    assert!(!EDITING_SCRIPT.contains("toolbarButton"));
    assert!(!EDITING_SCRIPT.contains("updateToolbar"));
}

#[test]
fn the_rotate_handle_ships_with_the_editing_script() {
    assert!(EDITING_SCRIPT.contains("swift-design-rotate"));
    assert!(EDITING_SCRIPT.contains("case 'rotate'"));
    assert!(EDITING_SCRIPT.contains("case 'reset-rotation'"));
    assert!(EDITING_SCRIPT.contains("Reset rotation"));
    assert!(EDITING_SCRIPT.contains("ROTATE_SNAP_DEGREES = 15"));
    assert!(EDITING_SCRIPT.contains("style.rotate"));
}

#[test]
fn resize_handles_ship_only_with_the_editing_script() {
    assert!(EDITING_SCRIPT.contains("swift-design-handles"));
    assert!(EDITING_SCRIPT.contains("data-swift-design-handle"));
    assert!(EDITING_SCRIPT.contains("case 'width'"));
    assert!(EDITING_SCRIPT.contains("case 'height'"));
    assert!(EDITING_SCRIPT.contains("case 'reset-size'"));
    assert!(EDITING_SCRIPT.contains("RESIZE_MINIMUM = 16"));
    assert!(EDITING_SCRIPT.contains("Reset size"));
    // The overlay lives on the body, outside the serialized root.
    assert!(EDITING_SCRIPT.contains("document.body.appendChild(handleBox)"));
    // The gesture survives the pointer leaving the preview frame,
    // and the edge opposite the handle stays anchored whatever the
    // layout does with the new size.
    assert!(EDITING_SCRIPT.contains("setPointerCapture"));
    assert!(EDITING_SCRIPT.contains("function correctResizeAnchor"));
}

#[test]
fn scripts_follow_the_options() {
    let design = sample_design();
    let plain = render_design(&design, false);
    assert!(!plain.contains("swift-design-html"));
    assert!(!plain.contains("data-swift-design-findings"));
    assert!(!plain.contains("data-swift-design-dragging"));
    let editable = render_design(&design, true);
    assert!(editable.contains("swift-design-html"));
    assert!(editable.contains("swift-design-apply"));
    assert!(editable.contains("contextmenu"));
    assert!(editable.contains("contentEditable"));
    // Dragging moves a node; a double click is what edits its text.
    assert!(editable.contains("data-swift-design-dragging"));
    assert!(editable.contains("style.translate"));
    assert!(editable.contains("'dblclick'"));
    // The HTML posted back carries none of the scripts' marks.
    assert!(editable.contains("node.removeAttribute('data-swift-design-inner-root')"));
    assert!(editable.contains("node.removeAttribute('data-swift-design-window-applied')"));
    assert!(editable.contains("node.style.setProperty(property, value, priority)"));
    // A command-click adds a node to the selection.
    assert!(editable.contains("event.metaKey || event.ctrlKey"));
    assert!(editable.contains("selection: selection.map(brief)"));
    // Pinning to the chat is a context-menu choice, not a side
    // effect of a click.
    assert!(editable.contains("Pin to chat"));
    assert!(editable.contains("action: 'pin'"));
    // Pin to chat covers the node case; only the screen-level Ask
    // AI item remains.
    assert!(editable.contains("Ask AI about this screen"));
    assert!(!editable.contains("'Ask AI about this'"));
    // The destructive items are marked, and the mark styles them.
    assert!(editable.contains("isDanger: true"));
    assert!(editable.contains("data-swift-design-danger"));
    assert!(editable.contains("reset-position"));
    assert!(editable.contains("reset-size"));
    // The plain render carries none of the editing overlays.
    assert!(!plain.contains("swift-design-handles"));
    assert!(!plain.contains("swift-design-guide"));
    assert!(!plain.contains("swift-design-rotate"));
    assert!(!plain.contains("UNDO_LIMIT"));
    assert!(editable.contains("swift-design-handles"));
    let auditing = render_design_with(
        &design,
        RenderOptions {
            is_auditing: true,
            asset_origin: Some("http://127.0.0.1:3000".to_owned()),
            ..RenderOptions::default()
        },
    );
    assert!(auditing.contains("data-swift-design-findings"));
    assert!(auditing.contains("img-src 'self' data: http://127.0.0.1:3000;"));
}

#[test]
fn app_window_script_stays_design_owned_and_runs_before_fit() {
    assert!(DESIGN_WINDOW_SCRIPT.contains("data-swift-design-window"));
    assert!(!FIT_SCRIPT.contains("data-swift-design-window"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("style.setProperty(property, value, 'important')"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("position', 'absolute'"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("transform', 'none'"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("translate', 'none'"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("rotate', 'none'"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("scale', 'none'"));
    assert!(DESIGN_WINDOW_SCRIPT.contains("window.swiftDesignApplyWindow = apply"));
    assert!(FIT_SCRIPT.contains("window.swiftDesignApplyWindow()"));
    assert!(
        DESIGN_WINDOW_SCRIPT
            .contains("if (!windowElement.hasAttribute('data-swift-design-window-applied'))")
    );
    let html = render_design(&sample_design(), false);
    let script_start = html.find(DESIGN_WINDOW_SCRIPT).unwrap();
    let fit_start = html.find(FIT_SCRIPT).unwrap();
    assert!(script_start < fit_start);
}

#[test]
fn legacy_designs_and_scoped_exports_have_no_window_metadata() {
    let design = sample_design();
    let full = render_design(&design, false);
    let scoped = render_design_with(
        &design,
        RenderOptions {
            only_screen: Some(2),
            is_print: true,
            ..RenderOptions::default()
        },
    );
    assert!(!full.contains("data-swift-design-window-left="));
    assert!(!scoped.contains("data-swift-design-window-left="));
    assert!(scoped.contains(DESIGN_WINDOW_SCRIPT));
    assert!(scoped.contains(FIT_SCRIPT));
    assert!(!scoped.contains("ResizeObserver"));
    assert!(!scoped.contains("data-swift-design-effect="));
}

#[test]
fn audit_checks_app_window_geometry_when_metadata_exists() {
    assert!(AUDIT_SCRIPT.contains("swiftDesignWindowLeft"));
    assert!(AUDIT_SCRIPT.contains("window_geometry"));
    assert!(AUDIT_SCRIPT.contains("rootRect.width / canvasWidth"));
    assert!(AUDIT_SCRIPT.contains("fitFactor < 0.999"));
    assert!(AUDIT_SCRIPT.contains("Array.from(root.children).find"));
}

#[test]
fn audit_has_opt_in_alignment_checks() {
    assert!(AUDIT_SCRIPT.contains("data-layout-align"));
    assert!(AUDIT_SCRIPT.contains("data-layout-gap"));
    assert!(AUDIT_SCRIPT.contains("kind: 'alignment'"));
    assert!(AUDIT_SCRIPT.contains("alignmentModes.has(align)"));
    assert!(AUDIT_SCRIPT.contains("gapModes.has(gap)"));
    assert!(AUDIT_SCRIPT.contains("needs at least 3 visible direct children"));
    assert!(AUDIT_SCRIPT.contains("differ by more than ' + tolerancePx + ' canvas px"));
}

#[test]
fn audit_respects_hidden_toggles_and_scrollports() {
    assert!(AUDIT_SCRIPT.contains("details:not([open])"));
    assert!(AUDIT_SCRIPT.contains("node.hasAttribute('hidden')"));
    assert!(AUDIT_SCRIPT.contains("style.visibility === 'collapse'"));
    assert!(AUDIT_SCRIPT.contains("isIntentionalScrollport"));
    assert!(AUDIT_SCRIPT.contains("overflowX"));
    assert!(AUDIT_SCRIPT.contains("overflowY"));
    assert!(AUDIT_SCRIPT.contains("!isInsideScrollport(element, root)"));
}

#[test]
fn audit_reports_measured_screen_coverage() {
    assert!(AUDIT_SCRIPT.contains("auditedScreens.push(screen)"));
    assert!(AUDIT_SCRIPT.contains("data-swift-design-audited-screens"));
    assert!(AUDIT_SCRIPT.contains("JSON.stringify(auditedScreens)"));
    assert!(AUDIT_SCRIPT.contains("frame.style.setProperty('visibility', 'visible', 'important')"));
    assert!(AUDIT_SCRIPT.contains("if (window.swiftDesignFit) { window.swiftDesignFit(); }"));
}

#[test]
fn audit_uses_demo_specific_fit_threshold() {
    assert!(AUDIT_SCRIPT.contains("const isDemo"));
    assert!(AUDIT_SCRIPT.contains("const fitThreshold = isDemo ? 0.999 : 0.97"));
    assert!(AUDIT_SCRIPT.contains("fitFactor < fitThreshold"));
}

#[test]
fn every_page_carries_the_fit_script() {
    // A screen that holds more than the canvas would be cut off, so
    // the fit runs on the studio page, the audit page, and the print
    // page alike.
    let design = sample_design();
    for options in [
        RenderOptions::default(),
        RenderOptions {
            is_editable: true,
            ..RenderOptions::default()
        },
        RenderOptions {
            is_auditing: true,
            ..RenderOptions::default()
        },
        RenderOptions {
            is_print: true,
            ..RenderOptions::default()
        },
    ] {
        let html = render_design_with(&design, options);
        assert!(html.contains("--swift-design-fit"), "{html}");
    }
}

#[test]
fn the_fit_measures_through_an_agents_own_root() {
    // An agent's own canvas-sized, clipped box hides its overflow
    // from the root: the script marks it, the stylesheet sizes it
    // with the root, and the text inside is measured directly.
    assert!(FIT_SCRIPT.contains("swiftDesignInnerRoot"));
    assert!(FIT_SCRIPT.contains("contentOverflows(root)"));
    let html = render_design(&sample_design(), false);
    assert!(html.contains(
        "[data-swift-design-inner-root] { width: 100% !important; height: 100% !important;"
    ));
}

#[test]
fn print_mode_emits_page_rules_and_the_fit_script_only() {
    let mut design = sample_design();
    design.transition = Some(Transition::default());
    let html = render_design_with(
        &design,
        RenderOptions {
            is_print: true,
            ..RenderOptions::default()
        },
    );
    assert!(html.contains("@page { size: 1440px 900px; margin: 0; }"));
    assert!(html.contains("break-after: page"));
    assert!(html.contains("transform: scale(var(--swift-design-fit, 1)); }"));
    // A print carries geometry and fit only, so the PDF holds the
    // same content the studio shows.
    assert!(html.contains("swiftDesignFit"));
    assert!(!html.contains("ResizeObserver"));
    assert!(!html.contains("addEventListener('keydown'"));
    assert!(!html.contains("ResizeObserver"));
    assert!(!html.contains("ArrowRight"));
    assert!(!html.contains("data-swift-design-effect="));
    assert!(html.contains("fonts.googleapis.com"));
    assert_eq!(
        html.matches("data-swift-design-root>").count(),
        design.screens.len()
    );
}

#[test]
fn a_design_without_a_transition_renders_the_plain_scroll_page() {
    let html = render_design(&sample_design(), false);
    assert!(!html.contains("data-swift-design-effect="));
    assert!(!html.contains("--swift-design-in"));
    assert!(html.contains(
        "<main class=\"design\" data-swift-design-width=\"1440\" data-swift-design-height=\"900\">"
    ));
    assert!(html.contains("scroll-snap-type: y mandatory"));
}

#[test]
fn a_transition_adds_the_design_attributes_and_the_stacked_rules() {
    let mut design = sample_design();
    design.transition = Some(Transition {
        effect: TransitionEffect::Cover,
        axis: TransitionAxis::Horizontal,
        duration_ms: 620,
    });
    let html = render_design(&design, false);
    assert!(html.contains("data-swift-design-effect=\"cover\""));
    assert!(html.contains("data-swift-design-axis=\"horizontal\""));
    assert!(html.contains("data-swift-design-duration=\"620\""));
    assert!(html.contains("--swift-design-duration: 620ms"));
    // Cover moves the new screen in and leaves the old one still.
    assert!(html.contains("[data-swift-design-state=\"entering\"] { z-index: 2; opacity: 1; transform: var(--swift-design-in); }"));
    assert!(html.contains(
        "[data-swift-design-state=\"leaving\"] { z-index: 1; opacity: 1; transform: none;"
    ));
    assert!(html.contains("prefers-reduced-motion"));
}

#[test]
fn the_none_effect_cuts_whatever_duration_the_design_asks_for() {
    let mut design = sample_design();
    design.transition = Some(Transition {
        effect: TransitionEffect::None,
        duration_ms: 900,
        ..Transition::default()
    });
    let html = render_design(&design, false);
    assert!(html.contains("data-swift-design-duration=\"0\""));
    assert!(html.contains("--swift-design-duration: 0ms"));
}

#[test]
fn a_duration_over_the_limit_is_capped_in_the_page() {
    let mut design = sample_design();
    design.transition = Some(Transition {
        duration_ms: 99_000,
        ..Transition::default()
    });
    let html = render_design(&design, false);
    assert!(html.contains(&format!(
        "data-swift-design-duration=\"{MAX_TRANSITION_MS}\""
    )));
}

#[test]
fn a_single_screen_page_keeps_the_scroll_layout_despite_a_transition() {
    let mut design = sample_design();
    design.transition = Some(Transition::default());
    let html = render_design_with(
        &design,
        RenderOptions {
            only_screen: Some(1),
            ..RenderOptions::default()
        },
    );
    assert!(!html.contains("data-swift-design-effect="));
    assert!(!html.contains("--swift-design-in"));
}

#[test]
fn escapes_the_design_title() {
    let mut design = sample_design();
    design.title = "<script>alert(1)</script>".to_owned();
    let html = render_design(&design, false);
    assert!(html.contains("<title>&lt;script&gt;alert(1)&lt;/script&gt;</title>"));
}

#[test]
fn css_safe_strips_breakout_characters() {
    assert_eq!(css_safe("Inter, sans-serif"), "Inter, sans-serif");
    assert_eq!(css_safe("x'; } </style>"), "x  /style");
    assert_eq!(css_safe("http://127.0.0.1:3000"), "http://127.0.0.1:3000");
}

#[test]
fn escape_html_covers_every_special_character() {
    assert_eq!(
        escape_html(r#"<a href="x">&'"#),
        "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
    );
}
