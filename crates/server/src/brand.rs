//! A template from brand material: a website or the user's files.
//!
//! A saved template is the style of an artifact the user liked. This
//! module makes one without an artifact: it shows the model a page
//! capture or the user's brand files and asks for a theme (palette and
//! fonts) plus a short style note. `templates.rs` saves the answer as
//! a template with no example screens, and `template_note` puts the
//! theme and the note into every candidate prompt that names it.

use design_model::Theme;
use serde::Deserialize;

use crate::capture::{capture_problem, page_text};
use crate::model_client::ModelClient;
use crate::screenshots::{dump_url, find_chrome, screenshot_url, supports_vision};
use crate::uploads::UploadStore;

/// Most files one extraction reads.
pub(crate) const MATERIAL_FILE_LIMIT: usize = 8;

/// Longest text one extraction sends, in bytes.
const MATERIAL_TEXT_LIMIT_BYTES: usize = 60 * 1024;

/// The window a website is captured in.
const CAPTURE_VIEWPORT: design_model::Viewport = design_model::Viewport {
    width: 1440,
    height: 900,
};

/// What the model reads: text and images from the source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BrandMaterial {
    /// Where the material came from, for the template record.
    pub(crate) source: String,
    /// Page text and text files, one block each.
    pub(crate) text: String,
    /// Screenshots and image files, as (name, content type, bytes).
    pub(crate) images: Vec<(String, String, Vec<u8>)>,
}

/// The model's answer: a theme and a note on the style.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(crate) struct ExtractedStyle {
    /// The palette and the fonts.
    pub(crate) theme: Theme,
    /// How the brand looks beyond the theme: spacing, shapes, imagery,
    /// voice. Two or three sentences.
    #[serde(default)]
    pub(crate) note: String,
}

/// The instruction the model gets with the material.
const EXTRACT_PROMPT: &str = "You read brand material and write its design theme as JSON.\n\
Reply with one JSON object only, no prose, no code fence:\n\
{\"theme\":{\"name\":\"short theme name\",\"colors\":{\"background\":\"#rrggbb\",\"text\":\"#rrggbb\",\"accent\":\"#rrggbb\",\"muted\":\"#rrggbb\"},\"fonts\":{\"heading\":\"font family\",\"body\":\"font family\",\"mono\":\"font family\"}},\"note\":\"two or three sentences\"}\n\
Rules:\n\
- Take the colors from the material. Use the page background as background, the main copy color as text, the primary button or link color as accent, and a secondary text color as muted.\n\
- Name Google Fonts families that match the material. Use `Inter` when the font is unknown.\n\
- In note, say how the brand looks: the spacing, the corner radius, the imagery, the tone of the copy. Do not repeat the colors.\n";

/// The material of a website: its text and one screenshot.
pub(crate) async fn material_from_url(url: &str) -> Result<BrandMaterial, String> {
    if let Some(problem) = capture_problem(url) {
        return Err(format!("`{url}` cannot be captured: {problem}"));
    }
    let chrome = find_chrome().ok_or_else(|| {
        "capturing a website needs Chrome or Chromium on the server machine".to_owned()
    })?;
    let screenshot = screenshot_url(&chrome, url, CAPTURE_VIEWPORT)
        .await
        .map_err(|error| format!("capturing `{url}` failed: {error:#}"))?;
    let dom = dump_url(&chrome, url, CAPTURE_VIEWPORT)
        .await
        .map_err(|error| format!("reading `{url}` failed: {error:#}"))?;
    Ok(BrandMaterial {
        source: url.to_owned(),
        text: cut(page_text(url, &dom), MATERIAL_TEXT_LIMIT_BYTES),
        images: vec![(
            "screenshot.png".to_owned(),
            "image/png".to_owned(),
            screenshot,
        )],
    })
}

/// The material of the named uploads in `scope`: text files as text,
/// images as images. A name outside the scope is refused.
pub(crate) async fn material_from_uploads(
    uploads: &UploadStore,
    scope: &str,
    names: &[String],
) -> Result<BrandMaterial, String> {
    if names.is_empty() {
        return Err("name at least one file".to_owned());
    }
    if names.len() > MATERIAL_FILE_LIMIT {
        return Err(format!("name at most {MATERIAL_FILE_LIMIT} files"));
    }
    let listing = uploads
        .list(scope)
        .await
        .map_err(|error| format!("listing the files failed: {error:#}"))?;
    let mut material = BrandMaterial {
        source: format!("files: {}", names.join(", ")),
        ..BrandMaterial::default()
    };
    for name in names {
        let Some(summary) = listing.iter().find(|summary| &summary.name == name) else {
            return Err(format!("`{name}` is not a file of this session"));
        };
        let bytes = match uploads.read(name).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(format!("`{name}` is no longer there")),
            Err(error) => return Err(format!("reading `{name}` failed: {error:#}")),
        };
        let content_type = summary.content_type.to_owned();
        if content_type.starts_with("image/") {
            material.images.push((name.clone(), content_type, bytes));
        } else if content_type.starts_with("text/") || content_type == "application/json" {
            material.text.push_str(&format!(
                "File {name}:\n{}\n\n",
                String::from_utf8_lossy(&bytes)
            ));
        } else if crate::office::is_office_type(&content_type) {
            let text = crate::office::office_text(&content_type, &bytes)
                .map_err(|error| format!("reading `{name}` failed: {error:#}"))?;
            material.text.push_str(&format!("File {name}:\n{text}\n\n"));
        } else {
            return Err(format!(
                "`{name}` is not a text or image file: attach a PNG, a JPEG, a PDF converted to images, or a text file"
            ));
        }
    }
    material.text = cut(material.text, MATERIAL_TEXT_LIMIT_BYTES);
    Ok(material)
}

/// Asks the model for the style of `material`.
pub(crate) async fn extract_style(
    client: &ModelClient,
    http: &reqwest::Client,
    material: &BrandMaterial,
) -> Result<ExtractedStyle, String> {
    let can_see_images = supports_vision(client.model());
    let mut parts = vec![serde_json::json!({
        "type": "text",
        "text": format!("Brand material from {}.\n{}", material.source, material.text),
    })];
    for (name, content_type, bytes) in &material.images {
        if !can_see_images {
            parts.push(serde_json::json!({
                "type": "text",
                "text": format!("Image {name}: this model cannot see it."),
            }));
            continue;
        }
        parts.push(serde_json::json!({
            "type": "text",
            "text": format!("Image {name}:"),
        }));
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!(
                    "data:{content_type};base64,{}",
                    crate::export::base64_encode(bytes)
                ),
            },
        }));
    }
    let messages = vec![
        serde_json::json!({ "role": "system", "content": EXTRACT_PROMPT }),
        serde_json::json!({ "role": "user", "content": parts }),
    ];
    let reply = client.chat(http, &messages, "low").await?;
    parse_extracted_style(&reply)
}

/// The style in a model reply: the first JSON object in it.
pub(crate) fn parse_extracted_style(reply: &str) -> Result<ExtractedStyle, String> {
    let start = reply
        .find('{')
        .ok_or_else(|| "the model reply holds no JSON object".to_owned())?;
    let end = reply
        .rfind('}')
        .filter(|end| *end > start)
        .ok_or_else(|| "the model reply holds no JSON object".to_owned())?;
    let style: ExtractedStyle = serde_json::from_str(&reply[start..=end])
        .map_err(|error| format!("the model reply is not a theme: {error}"))?;
    let theme = &style.theme;
    for (field, value) in [
        ("background", &theme.colors.background),
        ("text", &theme.colors.text),
        ("accent", &theme.colors.accent),
        ("muted", &theme.colors.muted),
    ] {
        if !is_hex_color(value) {
            return Err(format!(
                "the color `{field}` is `{value}`, not a #rrggbb value"
            ));
        }
    }
    for (field, value) in [
        ("heading", &theme.fonts.heading),
        ("body", &theme.fonts.body),
        ("mono", &theme.fonts.mono),
    ] {
        if value.trim().is_empty() {
            return Err(format!("the font `{field}` is empty"));
        }
    }
    if theme.name.trim().is_empty() {
        return Err("the theme has no name".to_owned());
    }
    Ok(style)
}

/// True for `#rgb`, `#rrggbb`, and `#rrggbbaa`.
fn is_hex_color(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 6 | 8)
        && digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

/// `text` cut to `limit` bytes on a character boundary.
fn cut(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[cut]", &text[..end])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::FakeModelServer;

    const REPLY: &str = r##"Here it is:
{"theme":{"name":"Acme","colors":{"background":"#ffffff","text":"#111111","accent":"#635bff","muted":"#6b7280"},"fonts":{"heading":"Inter","body":"Inter","mono":"JetBrains Mono"}},"note":"Generous whitespace, 8px corners, photography of people."}"##;

    #[test]
    fn a_reply_parses_into_a_theme_and_a_note() {
        let style = parse_extracted_style(REPLY).unwrap();
        assert_eq!(style.theme.name, "Acme");
        assert_eq!(style.theme.colors.accent, "#635bff");
        assert_eq!(style.theme.fonts.mono, "JetBrains Mono");
        assert!(style.note.starts_with("Generous whitespace"));
    }

    #[test]
    fn a_reply_with_a_bad_color_or_no_json_is_refused() {
        let bad = REPLY.replace("#635bff", "purple");
        assert!(parse_extracted_style(&bad).unwrap_err().contains("accent"));
        assert!(
            parse_extracted_style("no json")
                .unwrap_err()
                .contains("no JSON")
        );
        let empty_font = REPLY.replace("\"body\":\"Inter\"", "\"body\":\"\"");
        assert!(
            parse_extracted_style(&empty_font)
                .unwrap_err()
                .contains("body")
        );
        assert!(is_hex_color("#abc"));
        assert!(!is_hex_color("#abcd"));
    }

    #[tokio::test]
    async fn the_model_sees_the_material_and_its_theme_is_saved() {
        let server = FakeModelServer::start().await;
        server.push_text(REPLY);
        let client = ModelClient::new(server.configuration(), None);
        let http = ModelClient::build_http_client().unwrap();
        let material = BrandMaterial {
            source: "https://acme.com".to_owned(),
            text: "Title: Acme\nPricing from $9".to_owned(),
            images: vec![(
                "screenshot.png".to_owned(),
                "image/png".to_owned(),
                b"PNG".to_vec(),
            )],
        };
        let style = extract_style(&client, &http, &material).await.unwrap();
        assert_eq!(style.theme.name, "Acme");
        let request = server.requests()[0].to_string();
        assert!(request.contains("Brand material from https://acme.com"));
        assert!(request.contains("Pricing from $9"));
        assert!(request.contains("Reply with one JSON object only"));
        // The fake model has no vision, so the image is named, not sent.
        assert!(request.contains("this model cannot see it"));
        assert!(!request.contains("image_url"));
    }

    #[tokio::test]
    async fn uploads_outside_the_scope_and_binary_files_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let uploads = UploadStore::new(directory.path().join("uploads"));
        uploads.save("brand", "logo.png", b"PNG").await.unwrap();
        uploads
            .save("brand", "voice.md", b"Plain and warm.")
            .await
            .unwrap();
        uploads
            .save("other", "secret.md", b"Not yours.")
            .await
            .unwrap();
        let material = material_from_uploads(
            &uploads,
            "brand",
            &["logo.png".to_owned(), "voice.md".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(material.images.len(), 1);
        assert!(material.text.contains("Plain and warm."));
        assert!(material.source.contains("logo.png"));
        let error = material_from_uploads(&uploads, "brand", &["secret.md".to_owned()])
            .await
            .unwrap_err();
        assert!(error.contains("not a file of this session"));
        assert!(
            material_from_uploads(&uploads, "brand", &[])
                .await
                .unwrap_err()
                .contains("at least one")
        );
    }

    #[tokio::test]
    async fn a_private_website_is_refused_before_chrome_runs() {
        let error = material_from_url("http://127.0.0.1:1/").await.unwrap_err();
        assert!(error.contains("cannot be captured"));
    }
}
