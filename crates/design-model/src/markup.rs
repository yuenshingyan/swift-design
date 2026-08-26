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

/// Tags a screen may not contain.
const FORBIDDEN_TAGS: [&str; 33] = [
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
    "input",
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

/// A flat HTML tokenizer that reports forbidden and malformed markup.
struct HtmlChecker<'html> {
    characters: Vec<char>,
    position: usize,
    problems: Vec<String>,
    open: Vec<String>,
    svg_depth: usize,
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
                match self.read_attribute_value() {
                    Some(value) => Some(value),
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
        if name == "svg" {
            self.svg_depth += 1;
        }
        if VOID_TAGS.contains(&name.as_str()) {
            return;
        }
        if is_self_closing && self.svg_depth > 0 {
            if name == "svg" {
                self.svg_depth -= 1;
            }
            return;
        }
        self.implicitly_close_before(&name);
        self.open.push(name);
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

    /// Applies the attribute rules: no handlers, no renderer hooks, no
    /// forbidden names, safe URLs, safe inline styles.
    fn check_attribute(&mut self, tag: &str, attribute: &str, value: Option<&str>) {
        if attribute.starts_with("on") {
            self.problems.push(format!(
                "<{tag}> has attribute `{attribute}`: event handler attributes are not allowed"
            ));
            return;
        }
        if attribute.starts_with("data-swift-design") {
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

/// Every problem in a screen's CSS (or an inline style wrapped as
/// `x{...}`). Empty means the CSS is safe.
pub fn css_problems(css: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let normalized = decode_css_escapes(&strip_css_comments(css)).to_ascii_lowercase();
    if normalized.contains('<') {
        problems.push("contains `<`: remove it".to_owned());
    }
    for (needle, rule) in [
        ("@import", "use no @import: the server loads theme fonts"),
        ("expression(", "expression() is not allowed"),
        ("behavior:", "behavior: is not allowed"),
        ("-moz-binding", "-moz-binding is not allowed"),
        ("javascript:", "javascript: is not allowed"),
        ("vbscript:", "vbscript: is not allowed"),
        ("data:", "data: URLs are not allowed: use /uploads/{name}"),
    ] {
        if normalized.contains(needle) {
            problems.push(format!("contains `{needle}`: {rule}"));
        }
    }
    check_css_structure(&normalized, &mut problems);
    check_css_urls(&normalized, &mut problems);
    check_viewport_units(&normalized, &mut problems);
    problems
}

/// Removes `/* ... */` comments. An unterminated comment removes the rest.
fn strip_css_comments(css: &str) -> String {
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

/// Decodes CSS backslash escapes: `\75 rl(` becomes `url(`, `\:` becomes `:`.
fn decode_css_escapes(css: &str) -> String {
    let characters: Vec<char> = css.chars().collect();
    let mut decoded = String::with_capacity(css.len());
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '\\' {
            decoded.push(characters[index]);
            index += 1;
            continue;
        }
        let mut hex = String::new();
        let mut cursor = index + 1;
        while cursor < characters.len() && hex.len() < 6 && characters[cursor].is_ascii_hexdigit() {
            hex.push(characters[cursor]);
            cursor += 1;
        }
        if hex.is_empty() {
            if let Some(next) = characters.get(index + 1) {
                decoded.push(*next);
            }
            index += 2;
            continue;
        }
        if let Some(character) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            decoded.push(character);
        }
        if characters
            .get(cursor)
            .is_some_and(|next| next.is_whitespace())
        {
            cursor += 1;
        }
        index = cursor;
    }
    decoded
}

/// Braces must balance and strings must close.
fn check_css_structure(css: &str, problems: &mut Vec<String>) {
    let mut depth: i32 = 0;
    let mut quote: Option<char> = None;
    for character in css.chars() {
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
                    depth -= 1;
                    if depth < 0 {
                        problems.push("has a `}` with no matching `{`: remove it".to_owned());
                        return;
                    }
                }
                _ => {}
            },
        }
    }
    if quote.is_some() {
        problems.push("has an unterminated string: close the quote".to_owned());
    }
    if depth > 0 {
        problems.push("has an unclosed `{`: add the missing `}`".to_owned());
    }
}

/// `url(...)` may point only at fragments, uploads, and Google font files.
fn check_css_urls(css: &str, problems: &mut Vec<String>) {
    let mut rest = css;
    while let Some(start) = rest.find("url(") {
        let after = &rest[start + 4..];
        let Some(end) = after.find(')') else {
            problems.push("has an unclosed url(: add the `)`".to_owned());
            return;
        };
        let argument: String = after[..end]
            .chars()
            .filter(|character| {
                !character.is_whitespace() && *character != '"' && *character != '\''
            })
            .collect();
        let is_allowed = argument.starts_with('#')
            || upload_name(&argument).is_some()
            || argument.starts_with("https://fonts.gstatic.com/")
            || argument.starts_with("https://fonts.googleapis.com/");
        if !is_allowed {
            problems.push(format!(
                "url({}) is not allowed: use /uploads/{{name}} or a Google Fonts file",
                truncate(&argument, 60)
            ));
        }
        rest = &after[end + 1..];
    }
}

/// Viewport and container units break at thumbnail and editor sizes.
fn check_viewport_units(css: &str, problems: &mut Vec<String>) {
    const UNITS: [&str; 14] = [
        "vw", "vh", "vmin", "vmax", "svw", "svh", "lvw", "lvh", "dvw", "dvh", "cqw", "cqh", "cqi",
        "cqb",
    ];
    let bytes = css.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        if !byte.is_ascii_digit() {
            continue;
        }
        let rest = &css[index + 1..];
        for unit in UNITS {
            if rest.starts_with(unit)
                && !rest[unit.len()..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_alphanumeric())
            {
                problems.push(format!(
                    "uses the unit `{unit}`: use px against the design's fixed viewport"
                ));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(problems: Vec<String>) -> String {
        problems.into_iter().next().unwrap_or_default()
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
        assert!(
            first(html_problems("<img src=/uploads/a.png onerror=alert(1)>")).contains("onerror")
        );
        assert!(first(html_problems("<p data-swift-design-root>hi</p>")).contains("reserved"));
        assert!(
            first(html_problems("<img srcset='a 1x' src='/uploads/a.png'>")).contains("srcset")
        );
    }

    #[test]
    fn urls_are_decoded_and_restricted() {
        assert!(first(html_problems("<a href='javascript:alert(1)'>x</a>")).contains("points at"));
        assert!(
            first(html_problems("<a href='&#106;avascript:alert(1)'>x</a>")).contains("points at")
        );
        assert!(
            first(html_problems("<a href='java&#09;script:alert(1)'>x</a>")).contains("points at")
        );
        assert!(
            first(html_problems("<img src='data:image/png;base64,AAAA'>")).contains("points at")
        );
        assert!(
            first(html_problems("<img src='https://example.com/a.png'>")).contains("points at")
        );
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
            css_problems(".a { color: red; } @media (min-width: 10px) { .b { top: 1px; } }")
                .is_empty()
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
        assert!(
            first(css_problems(".a { background: url(https://evil/x.png); }")).contains("url(")
        );
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
}
