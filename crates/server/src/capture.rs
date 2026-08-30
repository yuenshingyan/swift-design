//! Web capture: a URL in a message becomes two source files.
//!
//! A user who pastes a link wants the model to see that page. The
//! prompt cannot follow links, so the server opens each `http(s)://`
//! address in the message in Chrome and saves a PNG of the first
//! viewport and the page text into the session's uploads, as
//! `capture-{host}.png` and `capture-{host}.txt`. `load_attachments`
//! then carries both into the prompt like any upload.
//!
//! Addresses on the local machine and on private networks are refused:
//! the server must not be turned into a reader of its own network.

use std::net::IpAddr;

use design_model::Viewport;

use crate::generation::ATTACHMENT_TEXT_LIMIT_BYTES;
use crate::office::unescape;
use crate::screenshots::{dump_url, find_chrome, screenshot_url};
use crate::uploads::UploadStore;

/// Most URLs captured from one message.
pub(crate) const CAPTURE_LIMIT: usize = 3;

/// The window Chrome opens a page in: the default desktop canvas.
const CAPTURE_VIEWPORT: Viewport = Viewport {
    width: 1440,
    height: 900,
};

/// The `http(s)://` addresses in `text`, in order, without repeats, at
/// most `CAPTURE_LIMIT`. Trailing punctuation of the sentence around
/// an address is not part of it.
pub(crate) fn urls_in(text: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for token in text.split(|character: char| {
        character.is_whitespace()
            || character == '<'
            || character == '>'
            || character == '"'
            || character == '\''
    }) {
        let start = match token.find("https://").or_else(|| token.find("http://")) {
            Some(start) => start,
            None => continue,
        };
        let url = token[start..].trim_end_matches(['.', ',', ';', ':', ')', ']', '!', '?']);
        if url.len() <= "https://".len() || urls.iter().any(|known| known == url) {
            continue;
        }
        urls.push(url.to_owned());
        if urls.len() == CAPTURE_LIMIT {
            break;
        }
    }
    urls
}

/// The host of `url`, without the port.
fn host_of(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then_some(host)
}

/// Why `url` is not captured: no host, or a host on this machine or a
/// private network.
pub(crate) fn capture_problem(url: &str) -> Option<String> {
    let Some(host) = host_of(url) else {
        return Some(format!("`{url}` has no host"));
    };
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return Some(format!("`{host}` is a local address"));
    }
    if let Ok(address) = lower.parse::<IpAddr>()
        && is_private_address(address)
    {
        return Some(format!("`{host}` is a private address"));
    }
    if !lower.contains('.') && lower.parse::<IpAddr>().is_err() {
        return Some(format!("`{host}` is not a public host name"));
    }
    None
}

/// True for loopback, link-local, private-range, and unspecified
/// addresses.
fn is_private_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| is_private_address(IpAddr::V4(v4)))
        }
    }
}

/// The upload names for a capture of `url`: `capture-{host}.png` and
/// `capture-{host}.txt`, with the host reduced to the characters an
/// upload name allows.
pub(crate) fn capture_names(url: &str) -> (String, String) {
    let host: String = host_of(url)
        .unwrap_or("page")
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '.' || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect();
    let host = host.trim_matches(['.', '-']);
    let host = if host.is_empty() { "page" } else { host };
    (format!("capture-{host}.png"), format!("capture-{host}.txt"))
}

/// The readable text of an HTML page: the title first, then the text
/// of the body with scripts, styles, and SVG dropped, one line per
/// block, cut at `ATTACHMENT_TEXT_LIMIT_BYTES`.
pub(crate) fn page_text(url: &str, html: &str) -> String {
    let without_hidden = drop_elements(html, &["script", "style", "noscript", "svg", "template"]);
    let title = element_text(&without_hidden, "title");
    let body = without_hidden
        .find("<body")
        .map(|start| &without_hidden[start..])
        .unwrap_or(&without_hidden);
    let mut text = format!("Page: {url}\n");
    if let Some(title) = title {
        text.push_str(&format!("Title: {title}\n"));
    }
    text.push('\n');
    text.push_str(&block_lines(body).join("\n"));
    cut_at_boundary(text, ATTACHMENT_TEXT_LIMIT_BYTES)
}

/// `html` with every `<tag …>…</tag>` of the named tags removed.
fn drop_elements(html: &str, tags: &[&str]) -> String {
    let mut text = html.to_owned();
    for tag in tags {
        let open = format!("<{tag}");
        let close = format!("</{tag}");
        while let Some(start) = find_tag_case_insensitive(&text, &open) {
            let Some(close_start) = find_tag_case_insensitive(&text[start..], &close) else {
                text.truncate(start);
                break;
            };
            let close_start = start + close_start;
            let end = text[close_start..]
                .find('>')
                .map_or(text.len(), |offset| close_start + offset + 1);
            text.replace_range(start..end, " ");
        }
    }
    text
}

/// The first position of `needle` in `haystack`, whatever the case,
/// where the character after it is not a letter, so `<b` does not
/// match `<body`.
fn find_tag_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let lower = haystack.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find(needle) {
        let start = from + found;
        let next = lower[start + needle.len()..].chars().next();
        if !next.is_some_and(|character| character.is_ascii_alphanumeric()) {
            return Some(start);
        }
        from = start + needle.len();
    }
    None
}

/// The text of the first `<tag>…</tag>`, unescaped and trimmed.
fn element_text(html: &str, tag: &str) -> Option<String> {
    let start = find_tag_case_insensitive(html, &format!("<{tag}"))?;
    let after_open = &html[start..];
    let content_start = after_open.find('>')? + 1;
    let content = &after_open[content_start..];
    let end = find_tag_case_insensitive(content, &format!("</{tag}"))?;
    let text = collapse_whitespace(&unescape(&strip_tags(&content[..end])));
    (!text.is_empty()).then_some(text)
}

/// The text of `html`, one line per block-level element, with the
/// tags removed and the whitespace collapsed. Empty lines are dropped.
fn block_lines(html: &str) -> Vec<String> {
    let mut with_breaks = html.to_owned();
    for tag in [
        "p",
        "div",
        "section",
        "article",
        "header",
        "footer",
        "nav",
        "main",
        "aside",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "tr",
        "br",
        "hr",
        "blockquote",
        "pre",
        "figcaption",
        "dt",
        "dd",
        "label",
        "button",
        "summary",
        "td",
        "th",
    ] {
        with_breaks = with_breaks
            .replace(&format!("<{tag}>"), &format!("\n<{tag}>"))
            .replace(&format!("<{tag} "), &format!("\n<{tag} "))
            .replace(&format!("</{tag}>"), &format!("</{tag}>\n"));
    }
    unescape(&strip_tags(&with_breaks))
        .lines()
        .map(collapse_whitespace)
        .filter(|line| !line.is_empty())
        .collect()
}

/// `html` without its tags. Text inside `<…>` is dropped; a stray `<`
/// with no closing `>` is kept.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find('>') {
            Some(end) => rest = &after[end + 1..],
            None => {
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` with runs of whitespace as one space, trimmed.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// `text` cut to at most `limit` bytes on a character boundary, with a
/// note when it was cut.
fn cut_at_boundary(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut: the page continues]", &text[..end])
}

/// Captures every address in `text` into `session_id`'s uploads and
/// returns the stored names. A refused address, a missing Chrome, and
/// a failed capture are logged and skipped: the message still posts.
pub(crate) async fn capture_urls(
    uploads: &UploadStore,
    session_id: &str,
    text: &str,
) -> Vec<String> {
    let urls = urls_in(text);
    if urls.is_empty() {
        return Vec::new();
    }
    let Some(chrome) = find_chrome() else {
        tracing::warn!(
            session_id,
            count = urls.len(),
            "web capture skipped: no Chrome"
        );
        return Vec::new();
    };
    let mut stored = Vec::new();
    for url in urls {
        if let Some(problem) = capture_problem(&url) {
            tracing::warn!(session_id, %problem, "web capture refused");
            continue;
        }
        let (png_name, text_name) = capture_names(&url);
        let screenshot = screenshot_url(&chrome, &url, CAPTURE_VIEWPORT).await;
        let dom = dump_url(&chrome, &url, CAPTURE_VIEWPORT).await;
        let (png, dom) = match (screenshot, dom) {
            (Ok(png), Ok(dom)) => (png, dom),
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!(session_id, host = host_of(&url).unwrap_or(""), %error, "web capture failed");
                continue;
            }
        };
        let text = page_text(&url, &dom);
        for (name, bytes) in [(png_name, png), (text_name, text.into_bytes())] {
            match uploads.save(session_id, &name, &bytes).await {
                Ok(Some(saved)) => stored.push(saved),
                Ok(None) => tracing::warn!(session_id, name, "web capture name refused"),
                Err(error) => tracing::warn!(session_id, name, %error, "web capture save failed"),
            }
        }
        tracing::info!(
            session_id,
            host = host_of(&url).unwrap_or(""),
            "web page captured"
        );
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_read_out_of_a_sentence_without_repeats() {
        let text = "Like https://stripe.com/pricing, and http://example.org. \
                    Again https://stripe.com/pricing (see <https://a.dev/x>).";
        assert_eq!(
            urls_in(text),
            [
                "https://stripe.com/pricing",
                "http://example.org",
                "https://a.dev/x"
            ]
        );
        assert!(urls_in("no links here").is_empty());
        assert!(urls_in("https://").is_empty());
        let many = "https://a.com https://b.com https://c.com https://d.com";
        assert_eq!(urls_in(many).len(), CAPTURE_LIMIT);
    }

    #[test]
    fn local_and_private_addresses_are_refused() {
        for url in [
            "http://localhost:3000/",
            "http://127.0.0.1:1/x",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://169.254.169.254/latest",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://printer.local/",
            "http://intranet/",
            "http://0.0.0.0/",
            "http://100.64.0.1/",
            "http:///nohost",
        ] {
            assert!(capture_problem(url).is_some(), "{url}");
        }
        for url in [
            "https://stripe.com/pricing",
            "http://93.184.216.34/",
            "https://user@example.org:8443/path?q=1",
        ] {
            assert_eq!(capture_problem(url), None, "{url}");
        }
    }

    #[test]
    fn capture_names_come_from_the_host() {
        assert_eq!(
            capture_names("https://www.Stripe.com/pricing"),
            (
                "capture-www.stripe.com.png".to_owned(),
                "capture-www.stripe.com.txt".to_owned()
            )
        );
        assert_eq!(
            capture_names("http://[2001:db8::1]/"),
            (
                "capture-2001-db8--1.png".to_owned(),
                "capture-2001-db8--1.txt".to_owned()
            )
        );
        assert_eq!(
            capture_names("http:///"),
            ("capture-page.png".to_owned(), "capture-page.txt".to_owned())
        );
    }

    #[test]
    fn page_text_keeps_the_title_and_the_blocks_and_drops_the_scripts() {
        let html = "<html><head><title> Acme &amp; Co </title><style>p{color:red}</style>\
                    <script>var x = '<p>no</p>';</script></head>\
                    <body><header><nav><a href='/'>Home</a> <a href='/b'>Blog</a></nav></header>\
                    <h1>Pricing</h1><p>From <b>$9</b> a month.</p>\
                    <svg><text>ignored</text></svg>\
                    <ul><li>One</li><li>Two</li></ul><noscript>Enable JS</noscript></body></html>";
        let text = page_text("https://acme.com/pricing", html);
        assert_eq!(
            text,
            "Page: https://acme.com/pricing\nTitle: Acme & Co\n\n\
             Home Blog\nPricing\nFrom $9 a month.\nOne\nTwo"
        );
    }

    #[test]
    fn a_long_page_is_cut_on_a_character_boundary() {
        let long = format!(
            "<body><p>{}</p></body>",
            "é".repeat(ATTACHMENT_TEXT_LIMIT_BYTES)
        );
        let text = page_text("https://acme.com/", &long);
        assert!(text.len() <= ATTACHMENT_TEXT_LIMIT_BYTES + "\n[cut: the page continues]".len());
        assert!(text.ends_with("[cut: the page continues]"));
    }

    #[test]
    fn a_tag_prefix_is_not_a_tag() {
        assert_eq!(find_tag_case_insensitive("<body><b>x</b>", "<b"), Some(6));
        assert_eq!(strip_tags("a <b>bold</b> c < d"), "a bold c < d");
        assert_eq!(drop_elements("x<SCRIPT>1</script>y", &["script"]), "x y");
    }
}
