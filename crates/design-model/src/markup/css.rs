use super::{truncate, upload_name};

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
