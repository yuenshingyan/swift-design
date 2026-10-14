//! Checks for screen HTML and CSS written by a model.
//!
//! The server inserts screen HTML into the page as written and scopes
//! the CSS, so both must be safe and well formed before they reach a
//! browser. The checks here are strict: anything the tokenizer cannot
//! read with confidence is a problem, and every problem names the
//! construct so an agent can fix it.

/// Most characters one screen's `html` may have.
pub const SCREEN_HTML_LIMIT: usize = 100_000;
/// Most characters one screen's `css` may have.
pub const SCREEN_CSS_LIMIT: usize = 50_000;

mod css;

#[cfg(test)]
mod tests;

pub use css::css_problems;

/// Tags a screen may not contain. `<input>` is allowed with the
/// `checkbox` and `radio` types only; `open_tag` checks the type.
const FORBIDDEN_TAGS: [&str; 32] = [
    "script",
    "style",
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "link",
    "meta",
    "base",
    "noscript",
    "template",
    "slot",
    "form",
    "button",
    "textarea",
    "select",
    "option",
    "math",
    "video",
    "audio",
    "source",
    "track",
    "portal",
    "plaintext",
    "xmp",
    "listing",
    "title",
    "set",
    "animate",
    "animatemotion",
];

/// Attribute names a screen may not use.
const FORBIDDEN_ATTRIBUTES: [&str; 6] = [
    "srcdoc",
    "srcset",
    "sizes",
    "formaction",
    "ping",
    "http-equiv",
];

/// The `<input>` types a screen may use. Each one keeps a state in CSS
/// through `:checked` and needs no script.
const INPUT_TYPES: [&str; 2] = ["checkbox", "radio"];

/// Attributes whose value is a URL.
const URL_ATTRIBUTES: [&str; 8] = [
    "src",
    "poster",
    "data",
    "action",
    "background",
    "cite",
    "longdesc",
    "manifest",
];

/// Elements that never have a closing tag.
const VOID_TAGS: [&str; 13] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Elements whose closing tag may be omitted.
const OPTIONAL_END_TAGS: [&str; 10] = [
    "p", "li", "dt", "dd", "tr", "td", "th", "thead", "tbody", "tfoot",
];

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
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// Every problem in a screen's HTML. Empty means the fragment is safe
/// and well formed.
pub fn html_problems(html: &str) -> Vec<String> {
    let mut checker = HtmlChecker::new(html);
    checker.run();
    checker.problems
}

/// Counts real `data-swift-design-window` markers in an HTML fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowMarkerSummary {
    /// Top-level non-void elements with the marker.
    pub top_level: usize,
    /// Nested non-void elements with the marker.
    pub nested: usize,
}

/// Counts app window markers while ignoring comments and attribute values.
pub fn window_marker_summary(html: &str) -> WindowMarkerSummary {
    let mut checker = HtmlChecker::new(html);
    checker.run();
    checker.window_summary
}

/// Remaps local screen links in valid HTML with a zero-based old-to-new index map.
/// Leaves text, styles, external URLs, invalid targets, and malformed HTML unchanged.
pub fn remap_screen_links(html: &str, old_to_new: &[usize]) -> String {
    let mut checker = HtmlChecker::new(html);
    checker.run();
    if !checker.problems.is_empty() {
        return html.to_owned();
    }
    let mut result = String::new();
    let mut previous = 0;
    for (start, end, target) in &checker.screen_links {
        let Some(number) = target
            .checked_sub(1)
            .and_then(|index| old_to_new.get(index))
            .and_then(|index| index.checked_add(1))
            .filter(|number| *number <= old_to_new.len())
        else {
            continue;
        };
        result.extend(checker.characters[previous..*start].iter());
        result.push_str(&format!("#screen-{number}"));
        previous = *end;
    }
    result.extend(checker.characters[previous..].iter());
    result
}

/// The upload file name in a `/uploads/{name}` or `uploads/{name}`
/// value, when the name is one the upload store can hold.
pub fn upload_name(value: &str) -> Option<&str> {
    let name = value
        .strip_prefix("/uploads/")
        .or_else(|| value.strip_prefix("uploads/"))?;
    let is_valid = !name.is_empty()
        && !name.starts_with('.')
        && !name.contains("..")
        && name.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || character == '.'
        });
    is_valid.then_some(name)
}

/// Returns declared layout checks in source order, using real HTML attributes.
pub fn layout_declarations(html: &str) -> Vec<(String, String)> {
    let mut checker = HtmlChecker::new(html);
    checker.run();
    checker.layout_declarations
}

/// Reads the exact numbered fragment the navigation script accepts.
fn screen_link_target(value: &str) -> Option<usize> {
    let decoded = decode_entities(value);
    let digits = decoded.strip_prefix("#screen-")?;
    if digits.is_empty() || !digits.bytes().all(|character| character.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A flat HTML tokenizer that reports forbidden and malformed markup.
struct HtmlChecker<'html> {
    characters: Vec<char>,
    position: usize,
    problems: Vec<String>,
    open: Vec<String>,
    svg_depth: usize,
    /// The `type` of the `<input>` being read, once seen.
    input_type: Option<String>,
    has_window_marker: bool,
    window_summary: WindowMarkerSummary,
    screen_links: Vec<(usize, usize, usize)>,
    layout_declarations: Vec<(String, String)>,
    source: &'html str,
}

impl<'html> HtmlChecker<'html> {
    fn new(source: &'html str) -> Self {
        Self {
            characters: source.chars().collect(),
            position: 0,
            problems: Vec::new(),
            open: Vec::new(),
            svg_depth: 0,
            input_type: None,
            has_window_marker: false,
            window_summary: WindowMarkerSummary::default(),
            screen_links: Vec::new(),
            layout_declarations: Vec::new(),
            source,
        }
    }

    fn run(&mut self) {
        if self.source.contains('\0') {
            self.problems
                .push("contains a NUL character: remove it".to_owned());
        }
        while self.position < self.characters.len() {
            if self.characters[self.position] != '<' {
                self.position += 1;
                continue;
            }
            match self.characters.get(self.position + 1) {
                Some('!') | Some('?') => {
                    self.problems.push(
                        "contains a comment, doctype, or processing instruction: remove it"
                            .to_owned(),
                    );
                    self.skip_to_tag_end();
                }
                Some('/') => {
                    self.position += 2;
                    let name = self.read_name();
                    self.skip_to_tag_end();
                    self.close_tag(&name);
                }
                Some(character) if character.is_ascii_alphabetic() => {
                    self.position += 1;
                    let name = self.read_name();
                    self.open_tag(name);
                }
                _ => self.position += 1,
            }
        }
        let unclosed: Vec<String> = self
            .open
            .iter()
            .filter(|name| !OPTIONAL_END_TAGS.contains(&name.as_str()))
            .map(|name| format!("<{name}>"))
            .collect();
        if !unclosed.is_empty() {
            self.problems.push(format!(
                "unclosed tags: {}: close every tag",
                unclosed.join(", ")
            ));
        }
    }

    /// Reads a tag name at the cursor, lowercased.
    fn read_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(character) = self.characters.get(self.position) {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | ':') {
                name.push(character.to_ascii_lowercase());
                self.position += 1;
            } else {
                break;
            }
        }
        name
    }

    /// Moves the cursor past the next `>`.
    fn skip_to_tag_end(&mut self) {
        while let Some(character) = self.characters.get(self.position) {
            self.position += 1;
            if *character == '>' {
                return;
            }
        }
    }

    /// True for a `<title>` inside an `<svg>`: the accessible name of a
    /// chart, not the document title `FORBIDDEN_TAGS` blocks.
    fn is_svg_title(&self, name: &str) -> bool {
        name == "title" && self.svg_depth > 0
    }

    /// Reads the attributes of an open tag, checks them, and records the
    /// element on the open stack.
    fn open_tag(&mut self, name: String) {
        if matches!(name.as_str(), "html" | "head" | "body") {
            self.problems.push(format!(
                "contains <{name}>: write a fragment, not a document"
            ));
        } else if FORBIDDEN_TAGS.contains(&name.as_str()) && !self.is_svg_title(&name) {
            self.problems
                .push(format!("contains <{name}>: this tag is not allowed"));
        }
        let mut is_self_closing = false;
        self.input_type = None;
        self.has_window_marker = false;
        loop {
            self.skip_whitespace_and_slashes(&mut is_self_closing);
            match self.characters.get(self.position) {
                None => {
                    self.problems
                        .push(format!("unterminated <{name}> tag: close it with >"));
                    return;
                }
                Some('>') => {
                    self.position += 1;
                    break;
                }
                _ => {}
            }
            let attribute = self.read_attribute_name();
            if attribute.is_empty() {
                // An unexpected character such as a quote; skip it.
                self.position += 1;
                continue;
            }
            self.skip_whitespace();
            let value = if self.characters.get(self.position) == Some(&'=') {
                self.position += 1;
                self.skip_whitespace();
                let is_quoted = matches!(self.characters.get(self.position), Some('\'' | '"'));
                let start = self.position + usize::from(is_quoted);
                match self.read_attribute_value() {
                    Some(value) => {
                        if name == "a"
                            && attribute == "href"
                            && let Some(target) = screen_link_target(&value)
                        {
                            let end = self.position - usize::from(is_quoted);
                            self.screen_links.push((start, end, target));
                        }
                        Some(value)
                    }
                    None => {
                        self.problems.push(format!(
                            "unterminated value for `{attribute}` on <{name}>: close the quote"
                        ));
                        return;
                    }
                }
            } else {
                None
            };
            self.check_attribute(&name, &attribute, value.as_deref());
        }
        if name == "input" {
            self.check_input_type();
        }
        if name == "svg" {
            self.svg_depth += 1;
        }
        if VOID_TAGS.contains(&name.as_str()) {
            self.has_window_marker = false;
            return;
        }
        self.implicitly_close_before(&name);
        self.record_window_marker();
        if is_self_closing && self.svg_depth > 0 {
            if name == "svg" {
                self.svg_depth -= 1;
            }
            self.has_window_marker = false;
            return;
        }
        self.has_window_marker = false;
        self.open.push(name);
    }

    fn record_window_marker(&mut self) {
        if !self.has_window_marker {
            return;
        }
        if self.open.is_empty() {
            self.window_summary.top_level += 1;
        } else {
            self.window_summary.nested += 1;
        }
    }

    /// Skips whitespace and stray `/` inside a tag. Records a `/` right
    /// before `>` as self-closing.
    fn skip_whitespace_and_slashes(&mut self, is_self_closing: &mut bool) {
        while let Some(character) = self.characters.get(self.position) {
            if character.is_whitespace() {
                self.position += 1;
            } else if *character == '/' {
                self.position += 1;
                *is_self_closing = self.characters.get(self.position) == Some(&'>');
            } else {
                break;
            }
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .characters
            .get(self.position)
            .is_some_and(|character| character.is_whitespace())
        {
            self.position += 1;
        }
    }

    /// Reads an attribute name at the cursor, lowercased.
    fn read_attribute_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(character) = self.characters.get(self.position) {
            if character.is_whitespace() || matches!(character, '/' | '>' | '=') {
                break;
            }
            name.push(character.to_ascii_lowercase());
            self.position += 1;
        }
        name
    }

    /// Reads a quoted or unquoted attribute value. `None` when a quote
    /// is never closed.
    fn read_attribute_value(&mut self) -> Option<String> {
        let mut value = String::new();
        match self.characters.get(self.position) {
            Some(&quote) if quote == '"' || quote == '\'' => {
                self.position += 1;
                loop {
                    let character = *self.characters.get(self.position)?;
                    self.position += 1;
                    if character == quote {
                        return Some(value);
                    }
                    value.push(character);
                }
            }
            _ => {
                while let Some(character) = self.characters.get(self.position) {
                    if character.is_whitespace() || *character == '>' {
                        break;
                    }
                    value.push(*character);
                    self.position += 1;
                }
                Some(value)
            }
        }
    }

    /// Reports an `<input>` whose type is not `checkbox` or `radio`. A
    /// missing type means a text field, which needs a script to do
    /// anything.
    fn check_input_type(&mut self) {
        let input_type = self.input_type.take().unwrap_or_default();
        if INPUT_TYPES.contains(&input_type.as_str()) {
            return;
        }
        let shown = if input_type.is_empty() {
            "no type".to_owned()
        } else {
            format!("type `{}`", truncate(&input_type, 30))
        };
        self.problems.push(format!(
            "contains <input> with {shown}: only type='checkbox' and type='radio' are allowed"
        ));
    }

    /// Applies the attribute rules: no handlers, no renderer hooks, no
    /// forbidden names, safe URLs, safe inline styles.
    fn check_attribute(&mut self, tag: &str, attribute: &str, value: Option<&str>) {
        if tag == "input" && attribute == "type" {
            self.input_type = Some(value.unwrap_or_default().trim().to_ascii_lowercase());
        }
        if attribute == "data-swift-design-window" {
            self.has_window_marker = true;
        }
        if matches!(attribute, "data-layout-align" | "data-layout-gap") {
            self.layout_declarations.push((
                attribute.to_owned(),
                decode_entities(value.unwrap_or_default()),
            ));
        }
        if attribute.starts_with("on") {
            self.problems.push(format!(
                "<{tag}> has attribute `{attribute}`: event handler attributes are not allowed"
            ));
            return;
        }
        if attribute.starts_with("data-swift-design") && attribute != "data-swift-design-window" {
            self.problems.push(format!(
                "<{tag}> has attribute `{attribute}`: names starting with data-swift-design are reserved"
            ));
            return;
        }
        if FORBIDDEN_ATTRIBUTES.contains(&attribute) {
            self.problems.push(format!(
                "<{tag}> has attribute `{attribute}`: this attribute is not allowed"
            ));
            return;
        }
        let is_url = URL_ATTRIBUTES.contains(&attribute) || attribute.ends_with("href");
        if is_url {
            let value = value.unwrap_or_default();
            if let Some(problem) = url_problem(tag, attribute, value) {
                self.problems.push(problem);
            }
            return;
        }
        if attribute == "style"
            && let Some(value) = value
        {
            for problem in css_problems(&format!("x{{{}}}", decode_entities(value))) {
                self.problems
                    .push(format!("<{tag}> style attribute: {problem}"));
            }
        }
    }

    /// Pops elements that an open tag closes implicitly.
    fn implicitly_close_before(&mut self, name: &str) {
        let Some(top) = self.open.last().cloned() else {
            return;
        };
        let closes_same =
            matches!(name, "p" | "li" | "tr" | "thead" | "tbody" | "tfoot") && top == name;
        let closes_cell = matches!(name, "td" | "th") && matches!(top.as_str(), "td" | "th");
        let closes_term = matches!(name, "dt" | "dd") && matches!(top.as_str(), "dt" | "dd");
        let closes_paragraph = top == "p" && BLOCK_TAGS.contains(&name);
        if closes_same || closes_cell || closes_term || closes_paragraph {
            self.open.pop();
        }
    }

    /// Pops the stack down to `name`. Elements popped on the way must be
    /// ones whose end tag is optional.
    fn close_tag(&mut self, name: &str) {
        if name.is_empty() || VOID_TAGS.contains(&name) {
            return;
        }
        if name == "svg" {
            self.svg_depth = self.svg_depth.saturating_sub(1);
        }
        let Some(position) = self.open.iter().rposition(|open| open == name) else {
            self.problems
                .push(format!("stray </{name}>: remove it or open the tag first"));
            return;
        };
        let skipped: Vec<String> = self.open[position + 1..]
            .iter()
            .filter(|open| !OPTIONAL_END_TAGS.contains(&open.as_str()))
            .map(|open| format!("<{open}>"))
            .collect();
        if !skipped.is_empty() {
            self.problems.push(format!(
                "</{name}> closes over unclosed {}: close them first",
                skipped.join(", ")
            ));
        }
        self.open.truncate(position);
    }
}

/// The problem with a URL attribute value, if any. Values are decoded
/// and normalized before the check, so `java&#09;script:` is caught.
fn url_problem(tag: &str, attribute: &str, value: &str) -> Option<String> {
    let decoded = decode_entities(value);
    let normalized: String = decoded
        .chars()
        .filter(|character| *character > '\u{20}')
        .collect::<String>()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return Some(format!(
            "<{tag}> has an empty `{attribute}`: set a value or remove the attribute"
        ));
    }
    if normalized.starts_with('#') || upload_name(&normalized).is_some() {
        return None;
    }
    let is_link = tag == "a"
        && ["https:", "http:", "mailto:", "tel:"]
            .iter()
            .any(|scheme| normalized.starts_with(scheme));
    if is_link {
        return None;
    }
    Some(format!(
        "<{tag}> `{attribute}` points at `{}`: use /uploads/{{name}} for images, or an https: link on <a>",
        truncate(value, 60)
    ))
}

/// The first `limit` characters of `text`.
fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Decodes numeric and common named HTML entities.
pub fn decode_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find(';').filter(|end| *end <= 10) else {
            decoded.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        let replacement = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            "tab" | "Tab" => Some('\t'),
            "newline" | "NewLine" => Some('\n'),
            "colon" => Some(':'),
            "semi" => Some(';'),
            "sol" => Some('/'),
            "num" => Some('#'),
            _ => numeric_entity(entity),
        };
        match replacement {
            Some(character) => {
                decoded.push(character);
                rest = &after[end + 1..];
            }
            None => {
                decoded.push('&');
                rest = after;
            }
        }
    }
    decoded.push_str(rest);
    decoded
}

/// The character of a `#NN` or `#xHH` entity body.
fn numeric_entity(entity: &str) -> Option<char> {
    let digits = entity.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}
