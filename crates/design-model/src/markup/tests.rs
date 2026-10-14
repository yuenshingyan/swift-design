use super::*;

fn first(problems: Vec<String>) -> String {
    problems.into_iter().next().unwrap_or_default()
}

#[test]
fn layout_declarations_come_from_real_attributes_not_text() {
    let declarations = layout_declarations(
        "<div DATA-LAYOUT-ALIGN='left' data-layout-gap='&#120;'><p>data-layout-align='right'</p></div>",
    );
    assert_eq!(
        declarations,
        [
            ("data-layout-align".to_owned(), "left".to_owned()),
            ("data-layout-gap".to_owned(), "x".to_owned())
        ]
    );
}

#[test]
fn screen_link_remapping_changes_only_real_anchor_destinations() {
    let html = "<p>你好 #screen-2</p><a HREF = '#screen-2' data-label='#screen-1'>Go</a><a href=\"#screen-1\">Home</a><a href=#screen-3>Last</a><a href='https://example.com/#screen-2'>External</a>";
    let expected = "<p>你好 #screen-2</p><a HREF = '#screen-1' data-label='#screen-1'>Go</a><a href=\"#screen-3\">Home</a><a href=#screen-2>Last</a><a href='https://example.com/#screen-2'>External</a>";
    assert_eq!(remap_screen_links(html, &[2, 0, 1]), expected);
}

#[test]
fn screen_link_remapping_handles_entities_and_leaves_invalid_targets_unchanged() {
    let html = "<a href='&#35;screen-&#50;'>Go</a><a href='#screen-0'>Zero</a><a href='#screen-7'>Missing</a><a href='#screen-999999999999999999999999999999999'>Huge</a>";
    let result = remap_screen_links(html, &[2, 0, 1]);
    assert!(result.contains("href='#screen-1'>Go"));
    assert!(result.contains("href='#screen-0'"));
    assert!(result.contains("href='#screen-7'"));
    assert!(result.contains("href='#screen-999999999999999999999999999999999'"));
    assert_eq!(
        remap_screen_links("<a href='#screen-1'>Go</a>", &[usize::MAX]),
        "<a href='#screen-1'>Go</a>"
    );
    assert_eq!(screen_link_target("#screen-2"), Some(2));
    assert_eq!(screen_link_target("#screen-2-extra"), None);
    assert_eq!(screen_link_target(" #screen-2"), None);
}

#[test]
fn malformed_html_and_comments_are_not_rewritten() {
    for html in [
        "<a href='#screen-1>Go</a>",
        "<!-- href='#screen-1' -->",
        "<p>href='#screen-1'</p>",
    ] {
        assert_eq!(remap_screen_links(html, &[1, 0]), html);
    }
}

#[test]
fn a_realistic_fragment_passes() {
    let html = "<div class='hero'><h1>Swift <em>Design</em></h1>\
            <ul><li>One<li>Two</ul><p>a < b and &amp; c</p>\
            <img src='/uploads/editor.png' alt='Editor'>\
            <svg viewBox='0 0 24 24'><defs><linearGradient id='g'/></defs>\
            <path d='M0 0h24v24' fill='url(#g)'/><rect width='4' height='4'/></svg>\
            <a href='https://example.com'>link</a><br><hr>\
            <table><tr><td>a<td>b</tr><tr><td>c<td>d</table></div>";
    assert_eq!(html_problems(html), Vec::<String>::new());
}

#[test]
fn forbidden_tags_are_reported() {
    for tag in [
        "script", "style", "iframe", "object", "embed", "link", "meta", "form", "video",
    ] {
        let problems = html_problems(&format!("<div><{tag}></{tag}></div>"));
        assert!(first(problems).contains(&format!("<{tag}>")), "{tag}");
    }
    assert!(first(html_problems("<html><body>x</body></html>")).contains("fragment"));
    assert!(first(html_problems("<!-- x --><p>y</p>")).contains("comment"));
}

#[test]
fn an_input_is_allowed_as_a_checkbox_or_a_radio_only() {
    let toggle =
        "<input type='checkbox' id='s1-menu' class='s1-toggle'><label for='s1-menu'>Menu</label>";
    assert_eq!(html_problems(toggle), Vec::<String>::new());
    assert_eq!(
        html_problems("<input type='RADIO' name='s1-tab' checked>"),
        Vec::<String>::new()
    );
    assert!(first(html_problems("<input type='text'>")).contains("type `text`"));
    assert!(first(html_problems("<input placeholder='Search'>")).contains("no type"));
    assert!(first(html_problems("<input type='checkbox' onchange='x()'>")).contains("onchange"));
}

#[test]
fn a_details_block_and_a_screen_link_are_allowed() {
    let menu =
        "<details><summary>File</summary><ul><li><a href='#screen-2'>Open</a></li></ul></details>";
    assert_eq!(html_problems(menu), Vec::<String>::new());
}

#[test]
fn a_title_is_allowed_inside_svg_only() {
    let chart = "<svg viewBox='0 0 10 10' role='img'><title>Sales</title><rect width='4' height='4'/></svg>";
    assert_eq!(html_problems(chart), Vec::<String>::new());
    assert!(first(html_problems("<div><title>x</title></div>")).contains("<title>"));
    assert!(first(html_problems("<svg></svg><title>x</title>")).contains("<title>"));
}

#[test]
fn event_handlers_and_reserved_attributes_are_reported() {
    assert!(first(html_problems("<p onclick='x()'>hi</p>")).contains("onclick"));
    assert!(
        first(html_problems("<a href='#x'onmouseover=alert(1)>hi</a>")).contains("onmouseover")
    );
    assert!(first(html_problems("<a/onclick=1>hi</a>")).contains("onclick"));
    assert!(first(html_problems("<img src=/uploads/a.png onerror=alert(1)>")).contains("onerror"));
    assert!(first(html_problems("<p data-swift-design-root>hi</p>")).contains("reserved"));
    assert!(first(html_problems("<img srcset='a 1x' src='/uploads/a.png'>")).contains("srcset"));
}

#[test]
fn urls_are_decoded_and_restricted() {
    assert!(first(html_problems("<a href='javascript:alert(1)'>x</a>")).contains("points at"));
    assert!(first(html_problems("<a href='&#106;avascript:alert(1)'>x</a>")).contains("points at"));
    assert!(first(html_problems("<a href='java&#09;script:alert(1)'>x</a>")).contains("points at"));
    assert!(first(html_problems("<img src='data:image/png;base64,AAAA'>")).contains("points at"));
    assert!(first(html_problems("<img src='https://example.com/a.png'>")).contains("points at"));
    assert!(first(html_problems("<img src='//evil/a.png'>")).contains("points at"));
    assert!(first(html_problems("<img src=''>")).contains("empty"));
    assert!(html_problems("<a href='mailto:x@y.z'>x</a>").is_empty());
    assert!(html_problems("<img src='uploads/a-1.png'>").is_empty());
    assert!(first(html_problems("<img src='/uploads/../x'>")).contains("points at"));
    assert_eq!(upload_name("/uploads/a.png"), Some("a.png"));
    assert_eq!(upload_name("/uploads/.hidden"), None);
    assert_eq!(upload_name("/uploads/UPPER.png"), None);
}

#[test]
fn inline_styles_are_checked_like_css() {
    assert!(
        first(html_problems(
            "<p style='background:url(https://x/a.png)'>x</p>"
        ))
        .contains("url(")
    );
    assert!(html_problems("<p style='background:url(/uploads/a.png)'>x</p>").is_empty());
    assert!(first(html_problems("<p style='width:50vw'>x</p>")).contains("vw"));
    // A resize, a move, or a rotation in the editor writes these inline.
    assert!(
        html_problems("<p style='translate: 10px 20px; width: 240px; height: 96px'>x</p>")
            .is_empty()
    );
    assert!(html_problems("<p style='rotate: 15deg'>x</p>").is_empty());
}

#[test]
fn balance_is_checked_with_html_rules() {
    assert!(first(html_problems("<div><span>x</div>")).contains("closes over unclosed <span>"));
    assert!(first(html_problems("<p>x</p></div>")).contains("stray </div>"));
    assert!(first(html_problems("<div>x")).contains("unclosed tags: <div>"));
    assert!(html_problems("<p>a<p>b").is_empty());
    assert!(html_problems("<p>a<div>b</div>").is_empty());
    assert!(html_problems("<ul><li>a<li>b</ul>").is_empty());
    assert!(html_problems("<dl><dt>a<dd>b</dl>").is_empty());
    assert!(html_problems("<img src='/uploads/a.png'/><br/>").is_empty());
    assert!(first(html_problems("<div x='y")).contains("unterminated value"));
    assert!(first(html_problems("<div")).contains("unterminated <div>"));
}

#[test]
fn css_rules_are_enforced() {
    assert!(
        css_problems(".a { color: red; } @media (min-width: 10px) { .b { top: 1px; } }").is_empty()
    );
    assert!(
        css_problems("@keyframes s1-fade { from { opacity: 0 } to { opacity: 1 } }").is_empty()
    );
    assert!(
        css_problems(
            "@font-face { font-family: X; src: url(https://fonts.gstatic.com/s/x.woff2); }"
        )
        .is_empty()
    );
    assert!(
        css_problems(".a { width: calc(100% - 48px); background: url('/uploads/a.png'); }")
            .is_empty()
    );
    assert!(first(css_problems("@import url(x.css);")).contains("@import"));
    assert!(first(css_problems(".a { color: red; } </style><script>")).contains("`<`"));
    assert!(first(css_problems(".a { } } body { }")).contains("no matching"));
    assert!(first(css_problems(".a { color: red;")).contains("unclosed `{`"));
    assert!(first(css_problems(".a { content: 'x }")).contains("unterminated string"));
    assert!(first(css_problems(".a { width: 50vw; }")).contains("vw"));
    assert!(first(css_problems(".a { width: 10cqw; }")).contains("cqw"));
    assert!(first(css_problems(".a { width: expression(1); }")).contains("expression"));
    assert!(first(css_problems(".a { background: url(https://evil/x.png); }")).contains("url("));
    assert!(
        first(css_problems(
            ".a { background: \\75 rl(https://evil/x.png); }"
        ))
        .contains("url(")
    );
    assert!(
        first(css_problems(
            ".a { background: url(data:image/png;base64,AA); }"
        ))
        .contains("data:")
    );
    assert!(css_problems("/* a { */ .b { color: red; }").is_empty());
    assert!(css_problems(".a { width: 100px; height: 1080px; }").is_empty());
}

#[test]
fn entity_decoding_covers_named_and_numeric_forms() {
    assert_eq!(decode_entities("a&amp;b&#65;&#x42;&lt;"), "a&bAB<");
    assert_eq!(decode_entities("&unknown;"), "&unknown;");
    assert_eq!(decode_entities("a & b"), "a & b");
}
