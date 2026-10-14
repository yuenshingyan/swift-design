//! The email-client HTML export: `GET /mailings/{id}/export.email.zip`.
//!
//! Email clients strip stylesheets and ignore modern layout CSS, so
//! the studio render cannot be sent. This module builds one
//! self-contained HTML document per email: the email CSS is inlined
//! into `style` attributes, the theme variables are resolved to
//! literal values, and the email sits in a centered 600 px table shell
//! with Outlook ghost tables around the `columns` pattern. Rules the
//! inliner cannot place on an element (`:hover`, pseudo-elements,
//! `@media`) stay in a `<style>` block that only some clients read.
//! `@keyframes`, `animation`, and `transition` are dropped. Flex and
//! grid containers downgrade to stacked blocks with their `gap` as
//! child margins, so an email from before the email-safe rules stays
//! readable. The export has no fit script: an email sized past its
//! canvas flows taller.
//! Images arrive as `data:` URIs, which Gmail blocks; rehost images at
//! a public URL for Gmail. No CSS or HTML crate: the walk is the same
//! string scan `docx.rs` and `office.rs` use.

use design_model::{Email, Mailing, Theme};

use crate::render::{css_safe, escape_html};
use crate::screen_css::{
    find_block_end, find_prelude_end, google_fonts_link, split_top_level, strip_comments,
};

/// One export file: the document, plus the subject and the preheader
/// read from the email's notes for `{id}-subjects.txt`.
pub struct EmailHtmlFile {
    /// The complete email-client HTML document.
    pub html: String,
    /// The `Subject:` text from the notes, when present.
    pub subject: Option<String>,
    /// The `Preheader:` text from the notes, when present.
    pub preheader: Option<String>,
}

/// Builds one email-client HTML document per email, in send order.
pub fn export_mailing_emails(mailing: &Mailing) -> Vec<EmailHtmlFile> {
    mailing
        .emails
        .iter()
        .map(|email| email_document(email, &mailing.theme, &mailing.title))
        .collect()
}

/// One email as a self-contained email-client document.
fn email_document(email: &Email, theme: &Theme, mailing_title: &str) -> EmailHtmlFile {
    let (subject, preheader) = subject_and_preheader(email.notes.as_deref());
    let variables = theme_variables(theme);
    let (mut rules, fallback) = parse_rules(email.css.as_deref().unwrap_or(""), &variables);
    let mut all_rules = base_rules(theme);
    let base_count = all_rules.len();
    for rule in &mut rules {
        rule.order += base_count;
    }
    all_rules.append(&mut rules);
    let tree = parse_fragment(&email.html);
    let mut properties: Vec<Vec<(String, String)>> = (0..tree.len())
        .map(|index| merged_properties(&tree, index, &all_rules, theme))
        .collect();
    downgrade_layout(&tree, &mut properties);
    let styles: Vec<String> = properties.into_iter().map(joined_style).collect();
    let body = serialize(&tree, &styles);
    let title = subject.clone().unwrap_or_else(|| mailing_title.to_owned());
    let background = css_safe(&theme.colors.background);
    let fonts = google_fonts_link(theme)
        .map(|link| format!("<!--[if !mso]><!-->{link}<!--<![endif]-->\n"))
        .unwrap_or_default();
    let preview = preheader
        .as_deref()
        .map(|text| {
            format!(
                "<div style=\"display:none;max-height:0;overflow:hidden;mso-hide:all;\">{}</div>\n",
                escape_html(text)
            )
        })
        .unwrap_or_default();
    let html = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"X-UA-Compatible\" content=\"IE=edge\">\n\
         <title>{title}</title>\n\
         {fonts}<style>\n{fallback}\
         @media (max-width:620px){{ div.column{{display:block!important;width:100%!important;}} \
         div.columns{{width:100%!important;}} }}\n</style>\n\
         </head>\n<body style=\"margin:0;padding:0;background:{background};\">\n\
         {preview}\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" \
         border=\"0\" bgcolor=\"{background}\"><tr><td align=\"center\">\n\
         <!--[if mso]><table role=\"presentation\" width=\"600\" cellpadding=\"0\" \
         cellspacing=\"0\" border=\"0\"><tr><td><![endif]-->\n\
         <div style=\"width:100%;max-width:600px;margin:0 auto;font-size:16px;line-height:1.3;\">\n\
         {body}\
         </div>\n\
         <!--[if mso]></td></tr></table><![endif]-->\n\
         </td></tr></table>\n</body>\n</html>\n",
        title = escape_html(&title),
    );
    EmailHtmlFile {
        html,
        subject,
        preheader,
    }
}

/// The `Subject:` and `Preheader:` texts in the notes. Each runs to
/// the other marker or to the end of its line.
fn subject_and_preheader(notes: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(notes) = notes else {
        return (None, None);
    };
    (
        marked_text(notes, "Subject:", &["Preheader:"]),
        marked_text(notes, "Preheader:", &["Subject:"]),
    )
}

/// The text after `marker`, cut at a line break or any of `stops`.
fn marked_text(notes: &str, marker: &str, stops: &[&str]) -> Option<String> {
    let start = notes.find(marker)? + marker.len();
    let rest = &notes[start..];
    let mut end = rest.find('\n').unwrap_or(rest.len());
    for stop in stops {
        if let Some(position) = rest.find(stop) {
            end = end.min(position);
        }
    }
    let text = rest[..end].trim();
    (!text.is_empty()).then(|| text.to_owned())
}

mod tree;

use tree::*;

// -- The CSS rules --------------------------------------------------------

/// One `:nth-child()` argument the matcher supports.
enum Nth {
    Index(usize),
    Even,
    Odd,
}

/// One structural pseudo-class the matcher supports.
enum Pseudo {
    First,
    Last,
    Nth(Nth),
}

/// One compound: `tag.class#id:pseudo` with every part optional.
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    pseudo: Option<Pseudo>,
}

/// How one compound relates to the one on its right.
enum Combinator {
    Descendant,
    Child,
}

/// One parsed selector: compounds left to right, with a combinator
/// between each pair.
struct Selector {
    compounds: Vec<Compound>,
    combinators: Vec<Combinator>,
}

/// One inlinable rule: a selector, its declarations, and its cascade
/// position.
struct StyleRule {
    selector: Selector,
    declarations: String,
    specificity: (u32, u32, u32),
    order: usize,
}

/// Splits `css` into inlinable rules and a fallback stylesheet, with
/// the theme variables resolved in both. `@keyframes` is dropped;
/// `animation` and `transition` declarations are stripped; conditional
/// at-rules and unsupported selectors go to the fallback block.
fn parse_rules(css: &str, variables: &[(String, String)]) -> (Vec<StyleRule>, String) {
    let css = strip_comments(css);
    let mut rules = Vec::new();
    let mut fallback = String::new();
    let mut rest = css.as_str();
    let mut order = 1usize;
    while !rest.trim().is_empty() {
        let Some(prelude_end) = find_prelude_end(rest) else {
            break;
        };
        let prelude = rest[..prelude_end].trim().to_owned();
        if rest[prelude_end..].starts_with(';') {
            rest = &rest[prelude_end + 1..];
            continue;
        }
        let body_start = prelude_end + 1;
        let Some(body_length) = find_block_end(&rest[body_start..]) else {
            break;
        };
        let body = rest[body_start..body_start + body_length].to_owned();
        rest = &rest[body_start + body_length + 1..];
        if let Some(name) = prelude.strip_prefix('@') {
            let name = name.split_whitespace().next().unwrap_or_default();
            if name.eq_ignore_ascii_case("keyframes") {
                continue;
            }
            fallback.push_str(&resolve_variables(&prelude, variables));
            fallback.push('{');
            fallback.push_str(&resolve_variables(&body, variables));
            fallback.push_str("}\n");
            continue;
        }
        // A body with a nested rule cannot be inlined.
        if body.contains('{') {
            fallback.push_str(&resolve_variables(&prelude, variables));
            fallback.push('{');
            fallback.push_str(&resolve_variables(&body, variables));
            fallback.push_str("}\n");
            continue;
        }
        let declarations = clean_declarations(&resolve_variables(&body, variables));
        if declarations.is_empty() {
            continue;
        }
        let mut fallback_selectors = Vec::new();
        for selector_text in split_top_level(&prelude, ',') {
            match parse_selector(selector_text) {
                Some(selector) => {
                    let specificity = specificity_of(&selector);
                    rules.push(StyleRule {
                        selector,
                        declarations: declarations.clone(),
                        specificity,
                        order,
                    });
                }
                None => fallback_selectors.push(selector_text.trim().to_owned()),
            }
        }
        if !fallback_selectors.is_empty() {
            fallback.push_str(&fallback_selectors.join(", "));
            fallback.push('{');
            fallback.push_str(&declarations);
            fallback.push_str("}\n");
        }
        order += 1;
    }
    (rules, fallback)
}

/// The declarations without `animation` and `transition` properties,
/// re-joined with single semicolons.
fn clean_declarations(body: &str) -> String {
    let kept: Vec<String> = split_top_level(body, ';')
        .into_iter()
        .filter_map(|declaration| {
            let declaration = declaration.trim();
            if declaration.is_empty() {
                return None;
            }
            let property = declaration
                .split(':')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if property.starts_with("animation") || property.starts_with("transition") {
                return None;
            }
            Some(declaration.to_owned())
        })
        .collect();
    kept.join("; ")
}

/// Parses one selector, or `None` when it uses a feature the inliner
/// does not support.
fn parse_selector(text: &str) -> Option<Selector> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut compounds = Vec::new();
    let mut combinators = Vec::new();
    // Normalize `a>b` to `a > b`, then walk tokens.
    let spaced = text.replace('>', " > ");
    for token in spaced.split_whitespace() {
        if token == ">" {
            let last = combinators.pop()?;
            if !matches!(last, Combinator::Descendant) {
                return None;
            }
            combinators.push(Combinator::Child);
            continue;
        }
        if !compounds.is_empty() && combinators.len() < compounds.len() {
            combinators.push(Combinator::Descendant);
        }
        compounds.push(parse_compound(token)?);
    }
    if compounds.is_empty() || combinators.len() != compounds.len() - 1 {
        return None;
    }
    Some(Selector {
        compounds,
        combinators,
    })
}

/// Parses one compound token, or `None` when a part is unsupported.
fn parse_compound(token: &str) -> Option<Compound> {
    let mut compound = Compound {
        tag: None,
        id: None,
        classes: Vec::new(),
        pseudo: None,
    };
    if token == "*" {
        return Some(compound);
    }
    let mut rest = token;
    if rest.chars().next()?.is_ascii_alphabetic() {
        let end = rest.find(['.', '#', ':']).unwrap_or(rest.len());
        compound.tag = Some(rest[..end].to_ascii_lowercase());
        rest = &rest[end..];
    }
    while !rest.is_empty() {
        let marker = rest.chars().next()?;
        rest = &rest[1..];
        match marker {
            '.' | '#' => {
                let end = rest.find(['.', '#', ':']).unwrap_or(rest.len());
                let part = &rest[..end];
                if part.is_empty() {
                    return None;
                }
                if marker == '.' {
                    compound.classes.push(part.to_owned());
                } else {
                    if compound.id.is_some() {
                        return None;
                    }
                    compound.id = Some(part.to_owned());
                }
                rest = &rest[end..];
            }
            ':' => {
                // A second colon is a pseudo-element: not inlinable.
                if compound.pseudo.is_some() || rest.starts_with(':') {
                    return None;
                }
                let end = match rest.find('(') {
                    Some(open) if rest.find(['.', '#', ':']).is_none_or(|other| open < other) => {
                        open + rest[open..].find(')')? + 1
                    }
                    _ => rest.find(['.', '#', ':']).unwrap_or(rest.len()),
                };
                compound.pseudo = Some(parse_pseudo(&rest[..end])?);
                rest = &rest[end..];
            }
            _ => return None,
        }
    }
    Some(compound)
}

/// Parses one supported pseudo-class, or `None`.
fn parse_pseudo(part: &str) -> Option<Pseudo> {
    let lower = part.to_ascii_lowercase();
    if lower == "first-child" {
        return Some(Pseudo::First);
    }
    if lower == "last-child" {
        return Some(Pseudo::Last);
    }
    let argument = lower
        .strip_prefix("nth-child(")?
        .strip_suffix(')')?
        .trim()
        .to_owned();
    match argument.as_str() {
        "even" => Some(Pseudo::Nth(Nth::Even)),
        "odd" => Some(Pseudo::Nth(Nth::Odd)),
        _ => argument
            .parse::<usize>()
            .ok()
            .map(|index| Pseudo::Nth(Nth::Index(index))),
    }
}

/// The (ids, classes and pseudo-classes, types) counts of a selector.
fn specificity_of(selector: &Selector) -> (u32, u32, u32) {
    let mut counts = (0, 0, 0);
    for compound in &selector.compounds {
        counts.0 += u32::from(compound.id.is_some());
        counts.1 += compound.classes.len() as u32 + u32::from(compound.pseudo.is_some());
        counts.2 += u32::from(compound.tag.is_some());
    }
    counts
}

/// True when `element` matches `selector`, walked right to left with
/// backtracking on descendant combinators.
fn selector_matches(tree: &[Element], element: usize, selector: &Selector) -> bool {
    matches_from(tree, element, selector, selector.compounds.len() - 1)
}

/// True when `element` matches compound `position` and its ancestry
/// matches the compounds to the left.
fn matches_from(tree: &[Element], element: usize, selector: &Selector, position: usize) -> bool {
    if !compound_matches(tree, element, &selector.compounds[position]) {
        return false;
    }
    if position == 0 {
        return true;
    }
    let combinator = &selector.combinators[position - 1];
    let mut ancestor = tree[element].parent;
    loop {
        if ancestor == 0 && tree[ancestor].tag.is_empty() {
            // Only the synthetic root remains; a Child combinator has
            // nothing left to match and a Descendant search is over.
            return false;
        }
        if matches_from(tree, ancestor, selector, position - 1) {
            return true;
        }
        if matches!(combinator, Combinator::Child) {
            return false;
        }
        if ancestor == 0 {
            return false;
        }
        ancestor = tree[ancestor].parent;
    }
}

/// True when `element` matches one compound.
fn compound_matches(tree: &[Element], element: usize, compound: &Compound) -> bool {
    let node = &tree[element];
    if node.tag.is_empty() {
        return false;
    }
    if let Some(tag) = &compound.tag
        && *tag != node.tag
    {
        return false;
    }
    if let Some(id) = &compound.id
        && attribute_value(node, "id").as_deref() != Some(id)
    {
        return false;
    }
    let classes = class_list(node);
    if !compound
        .classes
        .iter()
        .all(|class| classes.iter().any(|owned| owned == class))
    {
        return false;
    }
    match &compound.pseudo {
        None => true,
        Some(pseudo) => {
            let siblings: Vec<usize> = tree[node.parent]
                .children
                .iter()
                .filter_map(|child| match child {
                    Node::Element(index) => Some(*index),
                    _ => None,
                })
                .collect();
            let Some(position) = siblings.iter().position(|index| *index == element) else {
                return false;
            };
            match pseudo {
                Pseudo::First => position == 0,
                Pseudo::Last => position + 1 == siblings.len(),
                Pseudo::Nth(Nth::Index(wanted)) => position + 1 == *wanted,
                Pseudo::Nth(Nth::Even) => (position + 1).is_multiple_of(2),
                Pseudo::Nth(Nth::Odd) => !(position + 1).is_multiple_of(2),
            }
        }
    }
}

/// Replaces `var(--name)` and `var(--name, fallback)` with the theme
/// values. An unknown variable keeps its fallback, or stays as written
/// without one. Fallbacks that use variables resolve over three
/// passes.
fn resolve_variables(value: &str, variables: &[(String, String)]) -> String {
    let mut resolved = value.to_owned();
    for _ in 0..3 {
        let Some(replaced) = resolve_variables_once(&resolved, variables) else {
            break;
        };
        resolved = replaced;
    }
    resolved
}

/// One substitution pass, or `None` when nothing changed.
fn resolve_variables_once(value: &str, variables: &[(String, String)]) -> Option<String> {
    let start = value.find("var(")?;
    let inner_start = start + "var(".len();
    let mut depth = 1usize;
    let mut inner_end = None;
    for (offset, character) in value[inner_start..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    inner_end = Some(inner_start + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let inner_end = inner_end?;
    let inner = &value[inner_start..inner_end];
    let mut parts = split_top_level(inner, ',');
    let name = parts.remove(0).trim().to_owned();
    let fallback = (!parts.is_empty()).then(|| parts.join(",").trim().to_owned());
    let replacement = variables
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, value)| value.clone())
        .or(fallback);
    let Some(replacement) = replacement else {
        // Unknown with no fallback: leave the var() as written and stop
        // so the scan does not loop on it.
        return None;
    };
    let mut replaced = String::with_capacity(value.len());
    replaced.push_str(&value[..start]);
    replaced.push_str(&replacement);
    replaced.push_str(&value[inner_end + 1..]);
    Some(replaced)
}

/// The seven theme variables as (name, literal value), with the same
/// font stacks the studio render builds.
fn theme_variables(theme: &Theme) -> Vec<(String, String)> {
    vec![
        (
            "--background".to_owned(),
            css_safe(&theme.colors.background),
        ),
        ("--text".to_owned(), css_safe(&theme.colors.text)),
        ("--accent".to_owned(), css_safe(&theme.colors.accent)),
        ("--muted".to_owned(), css_safe(&theme.colors.muted)),
        (
            "--heading-font".to_owned(),
            format!(
                "'{}', Inter, system-ui, sans-serif",
                css_safe(&theme.fonts.heading)
            ),
        ),
        (
            "--body-font".to_owned(),
            format!(
                "'{}', Inter, system-ui, sans-serif",
                css_safe(&theme.fonts.body)
            ),
        ),
        (
            "--mono-font".to_owned(),
            format!("'{}', ui-monospace, monospace", css_safe(&theme.fonts.mono)),
        ),
    ]
}

/// The studio base styles as order-0 rules, without the canvas
/// machinery: no transform, no scale, no container queries, no fit.
fn base_rules(theme: &Theme) -> Vec<StyleRule> {
    let variables = theme_variables(theme);
    let value = |name: &str| {
        variables
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let pairs: [(&str, String); 7] = [
        (
            "h1, h2, h3, h4, h5, h6",
            format!(
                "font-family: {}; line-height: 1.1; margin: 0; letter-spacing: -0.02em",
                value("--heading-font")
            ),
        ),
        ("p, ul, ol, figure, blockquote", "margin: 0".to_owned()),
        ("ul, ol", "padding-left: 1.2em".to_owned()),
        ("img", "display: block; max-width: 100%".to_owned()),
        (
            "pre, code",
            format!("font-family: {}", value("--mono-font")),
        ),
        ("a", format!("color: {}", value("--accent"))),
        ("table", "border-collapse: collapse".to_owned()),
    ];
    let mut rules = Vec::new();
    for (order, (selectors, declarations)) in pairs.into_iter().enumerate() {
        for selector_text in split_top_level(selectors, ',') {
            if let Some(selector) = parse_selector(selector_text) {
                // Base rules sit below every author rule: specificity
                // zero, order by position.
                rules.push(StyleRule {
                    selector,
                    declarations: declarations.clone(),
                    specificity: (0, 0, 0),
                    order,
                });
            }
        }
    }
    rules
}

/// The merged properties of one element: box-sizing, the root extras,
/// the base and author rules by specificity then order, then the
/// element's own style attribute. Later declarations win per property;
/// `!important` declarations win last.
fn merged_properties(
    tree: &[Element],
    element: usize,
    rules: &[StyleRule],
    theme: &Theme,
) -> Vec<(String, String)> {
    let node = &tree[element];
    if node.tag.is_empty() {
        return Vec::new();
    }
    let mut matched: Vec<&StyleRule> = rules
        .iter()
        .filter(|rule| selector_matches(tree, element, &rule.selector))
        .collect();
    matched.sort_by_key(|rule| (rule.specificity, rule.order));
    let mut properties: Vec<(String, String, bool)> = Vec::new();
    let mut apply = |declarations: &str| {
        for declaration in split_top_level(declarations, ';') {
            let declaration = declaration.trim();
            let Some(colon) = declaration.find(':') else {
                continue;
            };
            let property = declaration[..colon].trim().to_ascii_lowercase();
            let mut value = declaration[colon + 1..].trim().to_owned();
            if property.is_empty() || value.is_empty() {
                continue;
            }
            let is_important = value.to_ascii_lowercase().ends_with("!important");
            if is_important {
                value = value[..value.len() - "!important".len()].trim().to_owned();
            }
            match properties
                .iter_mut()
                .find(|(known, _, _)| *known == property)
            {
                Some(slot) => {
                    if is_important || !slot.2 {
                        *slot = (property, value, is_important || slot.2);
                    }
                }
                None => properties.push((property, value, is_important)),
            }
        }
    };
    apply("box-sizing: border-box");
    // The first real element is the email root: it carries the theme
    // ground the studio puts on [data-swift-design-root].
    if is_root_element(tree, element) {
        let variables = theme_variables(theme);
        let value = |name: &str| {
            variables
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        apply(&format!(
            "background: {}; color: {}; font-family: {}",
            value("--background"),
            value("--text"),
            value("--body-font")
        ));
    }
    for rule in matched {
        apply(&rule.declarations);
    }
    if let Some(own) = attribute_value(node, "style") {
        apply(&own);
    }
    // The export flows: a px height pinned to the canvas would clip in
    // a client, so the root's height is dropped.
    if is_root_element(tree, element) {
        properties.retain(|(property, _, _)| property != "height");
    }
    properties
        .into_iter()
        .map(|(property, value, _)| (property, value))
        .collect()
}

/// One style attribute value from merged properties.
fn joined_style(properties: Vec<(String, String)>) -> String {
    properties
        .into_iter()
        .map(|(property, value)| format!("{property}:{value}"))
        .collect::<Vec<String>>()
        .join(";")
}

/// The container properties a flex or grid downgrade removes.
const CONTAINER_LAYOUT_PROPERTIES: [&str; 15] = [
    "flex-direction",
    "flex-wrap",
    "flex-flow",
    "align-items",
    "align-content",
    "justify-content",
    "justify-items",
    "place-items",
    "place-content",
    "grid-template-columns",
    "grid-template-rows",
    "grid-auto-flow",
    "grid-auto-rows",
    "grid-auto-columns",
    "grid-template-areas",
];

/// The child properties a flex or grid downgrade removes.
const CHILD_LAYOUT_PROPERTIES: [&str; 7] = [
    "flex",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "align-self",
    "justify-self",
    "order",
];

/// Downgrades flex and grid containers to stacked blocks, the way the
/// mobile media query stacks columns. Emails written under the current
/// rules carry no flex or grid; this keeps emails from before the
/// email-safe rules readable: the layout properties go, and the
/// container's `gap` becomes a bottom margin on every child but the
/// last.
fn downgrade_layout(tree: &[Element], properties: &mut [Vec<(String, String)>]) {
    for element in 0..tree.len() {
        let display = properties[element]
            .iter()
            .find(|(property, _)| property == "display")
            .map(|(_, value)| value.to_ascii_lowercase());
        let is_layout_container = matches!(
            display.as_deref(),
            Some("flex" | "inline-flex" | "grid" | "inline-grid")
        );
        if !is_layout_container {
            continue;
        }
        let gap = gap_px(&properties[element]);
        properties[element].retain(|(property, _)| {
            property != "display"
                && property != "gap"
                && property != "row-gap"
                && property != "column-gap"
                && !CONTAINER_LAYOUT_PROPERTIES.contains(&property.as_str())
        });
        let children: Vec<usize> = tree[element]
            .children
            .iter()
            .filter_map(|child| match child {
                Node::Element(index) => Some(*index),
                _ => None,
            })
            .collect();
        for (position, child) in children.iter().enumerate() {
            properties[*child]
                .retain(|(property, _)| !CHILD_LAYOUT_PROPERTIES.contains(&property.as_str()));
            // An `auto` margin only pushes inside flex; in a block flow
            // it reads as 0 and is noise.
            properties[*child].retain(|(property, value)| {
                !(property.starts_with("margin") && value.eq_ignore_ascii_case("auto"))
            });
            // Only an explicit bottom margin blocks the gap: appended
            // after a `margin` shorthand, `margin-bottom` wins the
            // bottom side, which is how the gap spaced the children.
            let has_bottom_margin = properties[*child]
                .iter()
                .any(|(property, _)| property == "margin-bottom");
            if position + 1 < children.len()
                && !has_bottom_margin
                && let Some(gap) = gap
            {
                properties[*child].push(("margin-bottom".to_owned(), format!("{gap}px")));
            }
        }
    }
}

/// The row gap of a container in whole px, from `gap` or `row-gap`.
fn gap_px(properties: &[(String, String)]) -> Option<u32> {
    let value = properties
        .iter()
        .find(|(property, _)| property == "row-gap")
        .or_else(|| properties.iter().find(|(property, _)| property == "gap"))
        .map(|(_, value)| value.clone())?;
    let first = value.split_whitespace().next()?.to_owned();
    first
        .strip_suffix("px")?
        .trim()
        .parse::<f32>()
        .ok()
        .map(|gap| gap as u32)
}

/// True for a top-level element of the fragment.
fn is_root_element(tree: &[Element], element: usize) -> bool {
    tree[element].parent == 0 && !tree[element].tag.is_empty()
}

/// Serializes the tree with merged styles, quoted attributes, explicit
/// end tags, and Outlook ghost tables around the `columns` pattern.
fn serialize(tree: &[Element], styles: &[String]) -> String {
    let mut output = String::new();
    serialize_children(tree, 0, styles, &mut output);
    output
}

/// Serializes the children of `element` in order.
fn serialize_children(tree: &[Element], element: usize, styles: &[String], output: &mut String) {
    for child in &tree[element].children {
        match child {
            Node::Text(text) => output.push_str(text),
            Node::Raw(raw) => output.push_str(raw),
            Node::Element(index) => serialize_element(tree, *index, styles, output),
        }
    }
}

/// Serializes one element, with the ghost table cells for a `columns`
/// section.
fn serialize_element(tree: &[Element], element: usize, styles: &[String], output: &mut String) {
    let node = &tree[element];
    let classes = class_list(node);
    let is_columns = classes.iter().any(|class| class == "columns");
    let is_column = classes.iter().any(|class| class == "column");
    if is_column {
        let width = width_px(&styles[element])
            .map(|width| width.to_string())
            .unwrap_or_else(|| "300".to_owned());
        output.push_str(&format!(
            "<!--[if mso]><td width=\"{width}\" valign=\"top\"><![endif]-->"
        ));
    }
    output.push('<');
    output.push_str(&node.tag);
    for (name, value) in &node.attributes {
        if name == "style" {
            continue;
        }
        match value {
            Some(value) => {
                output.push_str(&format!(" {name}=\"{}\"", escape_html(value)));
            }
            None => output.push_str(&format!(" {name}")),
        }
    }
    if node.tag == "svg" {
        // The svg open tag keeps its attributes as written; inline
        // styles rarely help svg in email clients but do no harm.
        if !styles[element].is_empty() {
            output.push_str(&format!(" style=\"{}\"", escape_html(&styles[element])));
        }
        output.push('>');
        serialize_children(tree, element, styles, output);
        output.push_str("</svg>");
        return;
    }
    if !styles[element].is_empty() {
        output.push_str(&format!(" style=\"{}\"", escape_html(&styles[element])));
    }
    output.push('>');
    if VOID_TAGS.contains(&node.tag.as_str()) {
        return;
    }
    if is_columns {
        output.push_str(
            "<!--[if mso]><table role=\"presentation\" width=\"100%\" cellpadding=\"0\" \
             cellspacing=\"0\" border=\"0\"><tr><![endif]-->",
        );
    }
    serialize_children(tree, element, styles, output);
    if is_columns {
        output.push_str("<!--[if mso]></tr></table><![endif]-->");
    }
    output.push_str(&format!("</{}>", node.tag));
    if is_column {
        output.push_str("<!--[if mso]></td><![endif]-->");
    }
}

/// The `width` in a merged style, in whole px, when it is a px length.
fn width_px(style: &str) -> Option<u32> {
    for declaration in split_top_level(style, ';') {
        let declaration = declaration.trim();
        let Some(value) = declaration
            .strip_prefix("width:")
            .or_else(|| declaration.strip_prefix("width :"))
        else {
            continue;
        };
        let value = value.trim();
        if let Some(number) = value.strip_suffix("px") {
            return number.trim().parse::<f32>().ok().map(|width| width as u32);
        }
    }
    None
}

#[cfg(test)]
mod tests;
