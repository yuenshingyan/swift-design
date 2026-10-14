use design_model::markup::decode_entities;

// -- The HTML tree --------------------------------------------------------

/// Elements that never have a closing tag.
pub(super) const VOID_TAGS: [&str; 7] = ["area", "br", "col", "embed", "hr", "img", "input"];

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
pub(super) enum Node {
    Element(usize),
    Text(String),
    Raw(String),
}

/// One element in the arena. Element 0 is a synthetic fragment root
/// with an empty tag; it never matches a selector and never serializes
/// a tag of its own.
pub(super) struct Element {
    pub(super) tag: String,
    /// Attribute names and decoded values, in source order. `style` is
    /// read here and re-emitted from the merged style instead.
    pub(super) attributes: Vec<(String, Option<String>)>,
    pub(super) parent: usize,
    pub(super) children: Vec<Node>,
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
pub(super) fn parse_fragment(html: &str) -> Vec<Element> {
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
pub(super) fn attribute_value(element: &Element, name: &str) -> Option<String> {
    element
        .attributes
        .iter()
        .find(|(attribute, _)| attribute == name)
        .and_then(|(_, value)| value.clone())
}

/// The class names of an element.
pub(super) fn class_list(element: &Element) -> Vec<String> {
    attribute_value(element, "class")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}
