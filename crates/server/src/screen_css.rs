//! Scoping of screen CSS and the Google Fonts link.
//!
//! The model writes plain selectors for one screen. `scope_css` prefixes
//! every selector with that screen's scope so rules never leak into other
//! screens or the page. Conditional at-rules recurse; `@keyframes`,
//! `@font-face`, and friends pass through unchanged; `@import` is dropped.

use design_model::Theme;

/// Block at-rules whose body holds style rules that must be scoped.
const CONDITIONAL_AT_RULES: [&str; 7] = [
    "media",
    "supports",
    "container",
    "layer",
    "scope",
    "starting-style",
    "document",
];

/// Statement at-rules that are dropped: they load or rename things.
const DROPPED_AT_RULES: [&str; 3] = ["import", "charset", "namespace"];

/// Font families that need no web font.
const GENERIC_FAMILIES: [&str; 16] = [
    "system-ui",
    "ui-sans-serif",
    "ui-serif",
    "ui-monospace",
    "sans-serif",
    "serif",
    "monospace",
    "arial",
    "helvetica",
    "helvetica neue",
    "georgia",
    "times new roman",
    "courier new",
    "verdana",
    "menlo",
    "monaco",
];

/// Prefixes every selector in `css` with `scope`. Never panics; stops
/// at the end of the input.
pub fn scope_css(css: &str, scope: &str) -> String {
    let mut output = String::with_capacity(css.len() + 64);
    scope_block(&strip_comments(css), scope, &mut output);
    output
}

/// Scopes one block body: a sequence of rules and at-rules.
fn scope_block(css: &str, scope: &str, output: &mut String) {
    let mut rest = css;
    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return;
        }
        rest = trimmed;
        if rest.starts_with('}') {
            // A stray closer: drop it.
            rest = &rest[1..];
            continue;
        }
        let Some(prelude_end) = find_prelude_end(rest) else {
            // No block or terminator: drop the tail.
            return;
        };
        let prelude = rest[..prelude_end].trim();
        if rest[prelude_end..].starts_with(';') {
            if !is_dropped_at_rule(prelude) {
                output.push_str(prelude);
                output.push(';');
            }
            rest = &rest[prelude_end + 1..];
            continue;
        }
        let body_start = prelude_end + 1;
        let Some(body_length) = find_block_end(&rest[body_start..]) else {
            let body = &rest[body_start..];
            emit_rule(prelude, body, scope, output);
            return;
        };
        let body = &rest[body_start..body_start + body_length];
        emit_rule(prelude, body, scope, output);
        rest = &rest[body_start + body_length + 1..];
    }
}

/// Writes one rule with its prelude scoped as needed.
fn emit_rule(prelude: &str, body: &str, scope: &str, output: &mut String) {
    if let Some(name) = prelude.strip_prefix('@') {
        let name = name
            .split(|character: char| character.is_whitespace() || character == '(')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        output.push_str(prelude);
        output.push('{');
        if CONDITIONAL_AT_RULES.contains(&name.as_str()) {
            scope_block(body, scope, output);
        } else {
            output.push_str(body);
        }
        output.push('}');
        return;
    }
    let selectors: Vec<String> = split_top_level(prelude, ',')
        .into_iter()
        .map(|selector| scope_selector(selector.trim(), scope))
        .collect();
    output.push_str(&selectors.join(", "));
    output.push('{');
    output.push_str(body);
    output.push('}');
}

/// Prefixes one selector with the scope, or replaces a root-like first
/// compound with it.
fn scope_selector(selector: &str, scope: &str) -> String {
    if selector.is_empty() {
        return scope.to_owned();
    }
    for root in [":root", "html", "body", ":scope", "&", ".screen"] {
        if let Some(rest) = selector.strip_prefix(root)
            && rest
                .chars()
                .next()
                .is_none_or(|next| next.is_whitespace() || matches!(next, '>' | '+' | '~' | ','))
        {
            return format!("{scope}{rest}");
        }
    }
    if selector.starts_with(['>', '+', '~']) {
        return format!("{scope} {selector}");
    }
    format!("{scope} {selector}")
}

/// True for `@import`, `@charset`, and `@namespace` statements.
fn is_dropped_at_rule(prelude: &str) -> bool {
    let Some(name) = prelude.strip_prefix('@') else {
        return false;
    };
    let name = name
        .split(|character: char| {
            character.is_whitespace() || character == '(' || character == '"' || character == '\''
        })
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    DROPPED_AT_RULES.contains(&name.as_str())
}

/// Index of the first `{` or `;` outside strings and parentheses.
pub(crate) fn find_prelude_end(css: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut parentheses = 0usize;
    for (index, character) in css.char_indices() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => quote = Some(character),
                '(' => parentheses += 1,
                ')' => parentheses = parentheses.saturating_sub(1),
                '{' | ';' if parentheses == 0 => return Some(index),
                _ => {}
            },
        }
    }
    None
}

/// Length of the block body starting right after `{`: the index of the
/// matching `}`.
pub(crate) fn find_block_end(css: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for (index, character) in css.char_indices() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => quote = Some(character),
                '{' => depth += 1,
                '}' => {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth -= 1;
                }
                _ => {}
            },
        }
    }
    None
}

/// Splits on `separator` outside strings, parentheses, and brackets.
pub(crate) fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let mut start = 0;
    for (index, character) in text.char_indices() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => quote = Some(character),
                '(' | '[' => depth += 1,
                ')' | ']' => depth = depth.saturating_sub(1),
                _ if character == separator && depth == 0 => {
                    parts.push(&text[start..index]);
                    start = index + character.len_utf8();
                }
                _ => {}
            },
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Removes `/* ... */` comments.
pub(crate) fn strip_comments(css: &str) -> String {
    let mut result = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        result.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return result,
        }
    }
    result.push_str(rest);
    result
}

/// The `<link>` tags that load the theme's fonts from Google Fonts, or
/// `None` when every family is generic or system.
pub fn google_fonts_link(theme: &Theme) -> Option<String> {
    let mut families: Vec<String> = Vec::new();
    for family in [&theme.fonts.heading, &theme.fonts.body, &theme.fonts.mono] {
        let cleaned: String = family
            .trim()
            .trim_matches(['"', '\''])
            .chars()
            .filter(|character| {
                character.is_ascii_alphanumeric() || *character == ' ' || *character == '-'
            })
            .collect();
        let cleaned = cleaned.trim().to_owned();
        if cleaned.is_empty() || GENERIC_FAMILIES.contains(&cleaned.to_ascii_lowercase().as_str()) {
            continue;
        }
        if !families.contains(&cleaned) {
            families.push(cleaned);
        }
    }
    if families.is_empty() {
        return None;
    }
    let query: Vec<String> = families
        .iter()
        .map(|family| format!("family={}:wght@400;500;600;700", family.replace(' ', "+")))
        .collect();
    Some(format!(
        "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n\
         <link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\n\
         <link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?{}&display=swap\">\n",
        query.join("&")
    ))
}

#[cfg(test)]
mod tests {
    use design_model::{FontSet, Palette, Theme};

    use super::*;

    const SCOPE: &str = "[data-swift-design-screen=\"2\"]";

    fn compact(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn selectors_are_prefixed_and_roots_are_replaced() {
        let scoped = scope_css(
            "h1, .a > p { color: red } :root { --x: 1 } body .b { top: 0 } > p { x: 1 }",
            SCOPE,
        );
        assert_eq!(
            compact(&scoped),
            compact(
                "[data-swift-design-screen=\"2\"] h1, [data-swift-design-screen=\"2\"] .a > p{ color: red }\
                 [data-swift-design-screen=\"2\"]{ --x: 1 }[data-swift-design-screen=\"2\"] .b{ top: 0 }\
                 [data-swift-design-screen=\"2\"] > p{ x: 1 }"
            )
        );
    }

    #[test]
    fn conditional_at_rules_recurse_and_others_pass_through() {
        let scoped = scope_css(
            "@media (min-width: 10px) { .a { top: 1px } } @keyframes s2-fade { from { opacity: 0 } to { opacity: 1 } } @font-face { font-family: X; src: url(https://fonts.gstatic.com/x.woff2) }",
            SCOPE,
        );
        assert!(
            scoped.contains(
                "@media (min-width: 10px){[data-swift-design-screen=\"2\"] .a{ top: 1px }}"
            )
        );
        assert!(scoped.contains("@keyframes s2-fade{ from { opacity: 0 } to { opacity: 1 } }"));
        assert!(
            scoped.contains(
                "@font-face{ font-family: X; src: url(https://fonts.gstatic.com/x.woff2) }"
            )
        );
    }

    #[test]
    fn imports_are_dropped_and_strings_are_respected() {
        let scoped = scope_css(
            "@import url(x.css); @layer base, extra; .a::before { content: '{,}' } .b { x: 1 }",
            SCOPE,
        );
        assert!(!scoped.contains("@import"));
        assert!(scoped.contains("@layer base, extra;"));
        assert!(scoped.contains("[data-swift-design-screen=\"2\"] .a::before{ content: '{,}' }"));
        assert!(scoped.contains("[data-swift-design-screen=\"2\"] .b{ x: 1 }"));
    }

    #[test]
    fn stray_and_missing_braces_do_not_panic() {
        assert!(scope_css("} .a { x: 1 }", SCOPE).contains(".a{ x: 1 }"));
        assert!(scope_css(".a { x: 1 ", SCOPE).contains(".a{ x: 1 }"));
        assert_eq!(scope_css("", SCOPE), "");
        assert!(scope_css("/* only a comment */", SCOPE).is_empty());
    }

    #[test]
    fn nested_rules_keep_their_body() {
        let scoped = scope_css(".card { color: red; &:hover { color: blue } }", SCOPE);
        assert!(scoped.contains(
            "[data-swift-design-screen=\"2\"] .card{ color: red; &:hover { color: blue } }"
        ));
    }

    #[test]
    fn font_links_cover_non_generic_families_once() {
        let theme = Theme {
            name: "n".to_owned(),
            colors: Palette {
                background: "#000000".to_owned(),
                text: "#ffffff".to_owned(),
                accent: "#ff0000".to_owned(),
                muted: "#888888".to_owned(),
            },
            fonts: FontSet {
                heading: "Playfair Display".to_owned(),
                body: "Inter".to_owned(),
                mono: "JetBrains Mono".to_owned(),
            },
        };
        let link = google_fonts_link(&theme).unwrap_or_default();
        assert!(link.contains("family=Playfair+Display:wght@400;500;600;700"));
        assert!(link.contains("family=Inter:wght@400;500;600;700"));
        assert!(link.contains("family=JetBrains+Mono:wght@400;500;600;700"));
        assert!(link.contains("display=swap"));
        let mut system = theme.clone();
        system.fonts = FontSet {
            heading: "system-ui".to_owned(),
            body: "Arial".to_owned(),
            mono: "ui-monospace".to_owned(),
        };
        assert!(google_fonts_link(&system).is_none());
    }
}
