#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::sample_mailing;

fn theme() -> Theme {
    sample_mailing().theme
}

fn inline(html: &str, css: &str) -> String {
    let variables = theme_variables(&theme());
    let (mut rules, _) = parse_rules(css, &variables);
    let mut all_rules = base_rules(&theme());
    let base_count = all_rules.len();
    for rule in &mut rules {
        rule.order += base_count;
    }
    all_rules.append(&mut rules);
    let tree = parse_fragment(html);
    let mut properties: Vec<Vec<(String, String)>> = (0..tree.len())
        .map(|index| merged_properties(&tree, index, &all_rules, &theme()))
        .collect();
    downgrade_layout(&tree, &mut properties);
    let styles: Vec<String> = properties.into_iter().map(joined_style).collect();
    serialize(&tree, &styles)
}

#[test]
fn simple_selectors_match_by_tag_class_and_id() {
    let output = inline(
        "<div class='a'><p id='x' class='b c'>Hi</p></div>",
        ".a { padding: 4px } p { color: #111111 } #x { font-size: 15px } .b.c { margin-top: 2px }",
    );
    assert!(output.contains("padding:4px"));
    assert!(output.contains("color:#111111"));
    assert!(output.contains("font-size:15px"));
    assert!(output.contains("margin-top:2px"));
}

#[test]
fn descendant_and_child_combinators_walk_the_ancestry() {
    let output = inline(
        "<div class='a'><div class='b'><p>Deep</p></div></div><p>Top</p>",
        ".a p { color: #222222 } .a > p { font-weight: 700 }",
    );
    assert!(output.contains("<p style=\"box-sizing:border-box;margin:0;color:#222222\">Deep"));
    assert!(!output.contains("font-weight:700"));
}

#[test]
fn nth_and_edge_children_match_by_position() {
    let output = inline(
        "<ul><li>1</li><li>2</li><li>3</li></ul>",
        "li:first-child { color: #111111 } li:last-child { color: #333333 } li:nth-child(2) { color: #222222 }",
    );
    assert!(output.contains("color:#111111\">1"));
    assert!(output.contains("color:#222222\">2"));
    assert!(output.contains("color:#333333\">3"));
}

#[test]
fn unsupported_selectors_fall_back_to_the_style_block() {
    let variables = theme_variables(&theme());
    let (rules, fallback) = parse_rules(
        ".cta:hover { opacity: 0.9 } li::marker { color: var(--accent) } @media (min-width: 300px) { .a { color: #111111 } } .kept { margin: 0 }",
        &variables,
    );
    assert_eq!(rules.len(), 1);
    assert!(fallback.contains(".cta:hover"));
    assert!(fallback.contains("li::marker"));
    assert!(fallback.contains("color: #0284c7"));
    assert!(fallback.contains("@media (min-width: 300px)"));
}

#[test]
fn specificity_orders_id_class_and_type_and_later_rules_win() {
    let output = inline(
        "<p id='x' class='a'>Hi</p>",
        "p { color: #111111 } .a { color: #222222 } #x { color: #333333 }",
    );
    assert!(output.contains("color:#333333"));
    let output = inline(
        "<p class='a'>Hi</p>",
        ".a { color: #111111 } .a { color: #222222 }",
    );
    assert!(output.contains("color:#222222"));
    assert!(!output.contains("#111111"));
}

#[test]
fn an_existing_style_attribute_wins_and_important_wins_over_it() {
    let output = inline(
        "<p class='a' style='color: #444444'>Hi</p>",
        ".a { color: #111111 } .a { font-size: 14px !important }",
    );
    assert!(output.contains("color:#444444"));
    assert!(output.contains("font-size:14px"));
}

#[test]
fn theme_variables_resolve_and_unknown_fallbacks_pass_through() {
    let variables = theme_variables(&theme());
    assert_eq!(
        resolve_variables("color: var(--accent)", &variables),
        "color: #0284c7"
    );
    assert_eq!(
        resolve_variables("color: var(--unknown, #123456)", &variables),
        "color: #123456"
    );
    assert_eq!(
        resolve_variables("color: var(--unknown)", &variables),
        "color: var(--unknown)"
    );
    assert!(resolve_variables("font: var(--body-font)", &variables).contains("'Inter'"));
}

#[test]
fn base_styles_materialize_without_the_canvas_machinery() {
    let output = inline(
        "<div class='e'><h2>T</h2><p>B</p><a href='https://x.example'>L</a></div>",
        "",
    );
    assert!(output.contains("letter-spacing:-0.02em"));
    assert!(output.contains("margin:0"));
    assert!(output.contains("color:#0284c7\""));
    assert!(!output.contains("--swift-design"));
    assert!(!output.contains("transform"));
    assert!(!output.contains("var("));
}

#[test]
fn the_root_takes_the_theme_ground_and_loses_its_height() {
    let output = inline(
        "<div class='e1'><p>Hi</p></div>",
        ".e1 { height: 100%; padding: 40px }",
    );
    assert!(output.contains("background:#f8fafc"));
    assert!(output.contains("color:#0f172a"));
    assert!(output.contains("padding:40px"));
    assert!(!output.contains("height:100%"));
}

#[test]
fn columns_compile_to_mso_ghost_tables() {
    let output = inline(
        "<div class='columns'><div class='column'><p>L</p></div><div class='column'><p>R</p></div></div>",
        ".columns { width: 520px } .column { display: inline-block; vertical-align: top; width: 260px }",
    );
    assert!(output.contains("<!--[if mso]><table role=\"presentation\""));
    assert!(output.contains("<!--[if mso]><td width=\"260\" valign=\"top\"><![endif]-->"));
    assert!(output.contains("<!--[if mso]></td><![endif]-->"));
    assert!(output.contains("<!--[if mso]></tr></table><![endif]-->"));
    assert!(output.contains("display:inline-block"));
}

#[test]
fn keyframes_and_animation_are_dropped() {
    let variables = theme_variables(&theme());
    let (rules, fallback) = parse_rules(
        "@keyframes e1-x { from { opacity: 0 } to { opacity: 1 } } .a { animation: e1-x 1s; transition: opacity 1s; color: #111111 }",
        &variables,
    );
    assert!(fallback.is_empty());
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].declarations, "color: #111111");
}

#[test]
fn subjects_come_from_the_notes() {
    let (subject, preheader) = subject_and_preheader(Some(
        "Subject: Six kinds, one chat. Preheader: Email joins the harness.",
    ));
    assert_eq!(subject.as_deref(), Some("Six kinds, one chat."));
    assert_eq!(preheader.as_deref(), Some("Email joins the harness."));
    assert_eq!(subject_and_preheader(None), (None, None));
    let (subject, preheader) = subject_and_preheader(Some("Just a remark."));
    assert_eq!(subject, None);
    assert_eq!(preheader, None);
}

#[test]
fn the_preheader_hides_at_the_body_start() {
    let mailing = sample_mailing();
    let files = export_mailing_emails(&mailing);
    assert_eq!(files.len(), 2);
    let first = &files[0].html;
    assert!(first.contains(
            "<div style=\"display:none;max-height:0;overflow:hidden;mso-hide:all;\">Email joins the harness.</div>"
        ));
    assert!(first.contains("<title>Six kinds, one chat.</title>"));
}

#[test]
fn the_export_wraps_every_email_in_the_table_shell() {
    let mailing = sample_mailing();
    for file in export_mailing_emails(&mailing) {
        assert!(file.html.starts_with("<!DOCTYPE html>"));
        assert!(
            file.html
                .contains("<!--[if mso]><table role=\"presentation\" width=\"600\"")
        );
        assert!(file.html.contains("width:100%;max-width:600px"));
        assert!(file.html.contains("font-size:16px;line-height:1.3"));
        assert!(file.html.contains("@media (max-width:620px)"));
        assert!(!file.html.contains("var("));
        assert!(!file.html.contains("display:flex"));
        assert!(!file.html.contains("display:grid"));
        assert!(file.subject.is_some());
    }
    // The second fixture email exercises the columns compiler.
    assert!(
        export_mailing_emails(&mailing)[1]
            .html
            .contains("<!--[if mso]><td width=\"260\"")
    );
}

#[test]
fn flex_and_grid_containers_downgrade_to_stacked_blocks() {
    let output = inline(
        "<div class='hero'><p>A</p><p>B</p><p>C</p></div>",
        ".hero { display: flex; flex-direction: row; gap: 24px; align-items: center; justify-content: space-between; }",
    );
    assert!(!output.contains("display:flex"));
    assert!(!output.contains("flex-direction"));
    assert!(!output.contains("justify-content"));
    assert!(!output.contains("gap:"));
    assert_eq!(output.matches("margin-bottom:24px").count(), 2);
    let output = inline(
        "<div class='steps'><p>A</p><p>B</p></div>",
        ".steps { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; } p { flex: 1; align-self: end; }",
    );
    assert!(!output.contains("display:grid"));
    assert!(!output.contains("grid-template-columns"));
    assert!(!output.contains("align-self"));
    assert!(output.contains("margin-bottom:16px"));
}

#[test]
fn a_downgraded_child_keeps_its_own_margin_and_loses_auto_margins() {
    let output = inline(
        "<div class='hero'><p class='a'>A</p><p class='b'>B</p><p>C</p></div>",
        ".hero { display: flex; gap: 24px } .a { margin-bottom: 4px } .b { margin-top: auto }",
    );
    assert!(output.contains("margin-bottom:4px"));
    assert!(!output.contains("margin-bottom:4px;margin-bottom"));
    assert!(!output.contains("margin-top:auto"));
}

#[test]
fn optional_end_tags_nest_like_the_markup_checker() {
    let output = inline("<ul><li>a<li>b</ul><p>x<p>y", "li { color: #111111 }");
    assert_eq!(output.matches("</li>").count(), 2);
    assert_eq!(output.matches("</p>").count(), 2);
    assert!(!output.contains("<li style=\"box-sizing:border-box;color:#111111\">a<li"));
}

#[test]
fn svg_subtrees_pass_through_untouched() {
    let output = inline(
        "<div class='a'><svg viewBox='0 0 10 10'><rect width='10' height='10'/></svg></div>",
        ".a { padding: 8px }",
    );
    assert!(output.contains("<rect width='10' height='10'/>"));
    assert!(output.contains("</svg>"));
}
