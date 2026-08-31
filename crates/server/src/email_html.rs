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
//! `@keyframes`, `animation`, and `transition` are dropped. The export
//! has no fit script: an email sized past its canvas flows taller.
//! Images arrive as `data:` URIs, which Gmail blocks; rehost images at
//! a public URL for Gmail. No CSS or HTML crate: the walk is the same
//! string scan `docx.rs` and `office.rs` use.

use design_model::markup::decode_entities;
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
    let styles: Vec<String> = (0..tree.len())
        .map(|index| merged_style(&tree, index, &all_rules, theme))
        .collect();
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
         <div style=\"width:600px;max-width:100%;margin:0 auto;font-size:16px;line-height:1.3;\">\n\
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

// -- The HTML tree --------------------------------------------------------

/// Elements that never have a closing tag.
const VOID_TAGS: [&str; 7] = ["area", "br", "col", "embed", "hr", "img", "input"];

/// Block elements that implicitly close an open `<p>`.
const BLOCK_TAGS: [&str; 21] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "div",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "ul",
];

/// One node of the parsed fragment: a child element by arena index,
/// text kept verbatim, or a raw subtree copied through unchanged.
enum Node {
    Element(usize),
    Text(String),
    Raw(String),
}

/// One element in the arena. Element 0 is a synthetic fragment root
/// with an empty tag; it never matches a selector and never serializes
/// a tag of its own.
struct Element {
    tag: String,
    /// Attribute names and decoded values, in source order. `style` is
    /// read here and re-emitted from the merged style instead.
    attributes: Vec<(String, Option<String>)>,
    parent: usize,
    children: Vec<Node>,
}

/// True when the open element `top` closes implicitly before `next`.
/// The same four cases as the markup checker.
fn implicitly_closes(top: &str, next: &str) -> bool {
    let closes_same =
        matches!(next, "p" | "li" | "tr" | "thead" | "tbody" | "tfoot") && top == next;
    let closes_cell = matches!(next, "td" | "th") && matches!(top, "td" | "th");
    let closes_term = matches!(next, "dt" | "dd") && matches!(top, "dt" | "dd");
    let closes_paragraph = top == "p" && BLOCK_TAGS.contains(&next);
    closes_same || closes_cell || closes_term || closes_paragraph
}

/// Parses a validated fragment into an arena. The input passed the
/// markup checker: no comments, no doctype, optional end tags follow
/// the HTML rules. Malformed input never panics; a stray close tag is
/// dropped.
fn parse_fragment(html: &str) -> Vec<Element> {
    let mut arena = vec![Element {
        tag: String::new(),
        attributes: Vec::new(),
        parent: 0,
        children: Vec::new(),
    }];
    let mut open = vec![0usize];
    let mut rest = html;
    loop {
        let Some(start) = rest.find('<') else {
            push_text(&mut arena, &open, rest);
            break;
        };
        push_text(&mut arena, &open, &rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let tag = &rest[start + 1..start + end];
        rest = &rest[start + end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            close_element(&arena, &mut open, &name.trim().to_ascii_lowercase());
            continue;
        }
        let (name, attributes) = parse_tag(tag);
        if name.is_empty() {
            continue;
        }
        while open.len() > 1 {
            let top = arena[*open.last().unwrap_or(&0)].tag.clone();
            if implicitly_closes(&top, &name) {
                open.pop();
            } else {
                break;
            }
        }
        let parent = *open.last().unwrap_or(&0);
        if name == "svg" {
            let (inner, after) = raw_svg_content(rest);
            let index = arena.len();
            arena.push(Element {
                tag: name,
                attributes,
                parent,
                children: vec![Node::Raw(inner.to_owned())],
            });
            arena[parent].children.push(Node::Element(index));
            rest = after;
            continue;
        }
        let is_void = VOID_TAGS.contains(&name.as_str()) || tag.trim_end().ends_with('/');
        let index = arena.len();
        arena.push(Element {
            tag: name,
            attributes,
            parent,
            children: Vec::new(),
        });
        arena[parent].children.push(Node::Element(index));
        if !is_void {
            open.push(index);
        }
    }
    arena
}

/// Appends non-empty text to the innermost open element.
fn push_text(arena: &mut [Element], open: &[usize], text: &str) {
    if text.is_empty() {
        return;
    }
    let parent = *open.last().unwrap_or(&0);
    arena[parent].children.push(Node::Text(text.to_owned()));
}

/// Pops the stack down through the closest open element named `name`,
/// closing the optional-end-tag elements above it on the way. A close
/// tag with no open element is dropped.
fn close_element(arena: &[Element], open: &mut Vec<usize>, name: &str) {
    if name.is_empty() || VOID_TAGS.contains(&name) {
        return;
    }
    let Some(position) = open.iter().rposition(|index| arena[*index].tag == name) else {
        return;
    };
    if position == 0 {
        return;
    }
    open.truncate(position);
}

/// The lowercased tag name and its decoded attributes.
fn parse_tag(tag: &str) -> (String, Vec<(String, Option<String>)>) {
    let tag = tag.trim().trim_end_matches('/');
    let name_end = tag
        .find(|character: char| character.is_whitespace())
        .unwrap_or(tag.len());
    let name = tag[..name_end].to_ascii_lowercase();
    let mut attributes = Vec::new();
    let mut rest = tag[name_end..].trim_start();
    while !rest.is_empty() {
        let name_end = rest
            .find(|character: char| character.is_whitespace() || character == '=')
            .unwrap_or(rest.len());
        let attribute = rest[..name_end].to_ascii_lowercase();
        if attribute.is_empty() {
            break;
        }
        rest = rest[name_end..].trim_start();
        let value = if let Some(after) = rest.strip_prefix('=') {
            let after = after.trim_start();
            let (raw, next) = match after.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &after[1..];
                    match inner.find(quote) {
                        Some(end) => (&inner[..end], &inner[end + 1..]),
                        None => (inner, ""),
                    }
                }
                _ => {
                    let end = after
                        .find(|character: char| character.is_whitespace())
                        .unwrap_or(after.len());
                    (&after[..end], &after[end..])
                }
            };
            rest = next.trim_start();
            Some(decode_entities(raw))
        } else {
            None
        };
        attributes.push((attribute, value));
    }
    (name, attributes)
}

/// The raw inner content of an `<svg>` subtree and the text after its
/// matching close tag. Nested `<svg>` is counted.
fn raw_svg_content(html: &str) -> (&str, &str) {
    let mut depth = 0usize;
    let mut rest = html;
    let mut consumed = 0usize;
    while let Some(start) = rest.find('<') {
        let after = &rest[start..];
        let lower = after[1..].to_ascii_lowercase();
        if lower.starts_with("/svg") {
            if depth == 0 {
                let close = after.find('>').map(|end| end + 1).unwrap_or(after.len());
                return (&html[..consumed + start], &rest[start + close..]);
            }
            depth -= 1;
        } else if lower.starts_with("svg")
            && lower
                .get(3..4)
                .and_then(|next| next.chars().next())
                .is_none_or(|next| !next.is_ascii_alphanumeric())
        {
            depth += 1;
        }
        consumed += start + 1;
        rest = &rest[start + 1..];
    }
    (html, "")
}

/// The decoded value of one attribute, when present.
fn attribute_value(element: &Element, name: &str) -> Option<String> {
    element
        .attributes
        .iter()
        .find(|(attribute, _)| attribute == name)
        .and_then(|(_, value)| value.clone())
}

/// The class names of an element.
fn class_list(element: &Element) -> Vec<String> {
    attribute_value(element, "class")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

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

/// The merged inline style of one element: box-sizing, the root
/// extras, the base and author rules by specificity then order, then
/// the element's own style attribute. Later declarations win per
/// property; `!important` declarations win last.
fn merged_style(tree: &[Element], element: usize, rules: &[StyleRule], theme: &Theme) -> String {
    let node = &tree[element];
    if node.tag.is_empty() {
        return String::new();
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
        .map(|(property, value, _)| format!("{property}:{value}"))
        .collect::<Vec<String>>()
        .join(";")
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
#[allow(clippy::unwrap_used)]
mod tests {
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
        let styles: Vec<String> = (0..tree.len())
            .map(|index| merged_style(&tree, index, &all_rules, &theme()))
            .collect();
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
            assert!(file.html.contains("width:600px;max-width:100%"));
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
}
