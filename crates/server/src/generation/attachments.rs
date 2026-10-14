use super::*;

/// A user message content: the text alone, or OpenAI-style parts with
/// one `image_url` data URL per PNG when images are present.
pub(crate) fn user_content_with_images(text: &str, images: &[Vec<u8>]) -> serde_json::Value {
    if images.is_empty() {
        return serde_json::Value::String(text.to_owned());
    }
    let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
    for (index, image) in images.iter().enumerate() {
        parts.push(serde_json::json!({ "type": "text", "text": format!("Screen {}:", index + 1) }));
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": format!("data:image/png;base64,{}", base64_encode(image)),
            },
        }));
    }
    serde_json::Value::Array(parts)
}

/// One upload attached to a model request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadAttachment {
    /// Stored upload name, as in `/uploads/{name}`.
    pub name: String,
    /// Content type from the extension.
    pub content_type: String,
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// The uploads a request carries, plus the ones the size caps left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachments {
    /// Files sent as content parts, in listing order.
    pub files: Vec<UploadAttachment>,
    /// `name (size)` of every file skipped over the caps.
    pub skipped: Vec<String>,
}

/// Largest single upload attached to a request.
pub(super) const ATTACHMENT_FILE_LIMIT_BYTES: usize = 20 * 1024 * 1024;

/// Largest total of uploads attached to one request.
pub(super) const ATTACHMENT_TOTAL_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// Longest text file inlined into a request; the rest is cut with a
/// note.
pub(crate) const ATTACHMENT_TEXT_LIMIT_BYTES: usize = 100 * 1024;

/// Most text inlined into one request over every file. A codebase of
/// source files adds up to more than a context window holds, so once
/// the budget is spent the remaining text files are named only.
pub(crate) const ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES: usize = 256 * 1024;

/// A short human size: `512 B`, `3.4 KB`, `1.2 MB`.
pub(super) fn describe_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// A user message content: the text alone when nothing is attached, or
/// OpenAI-style parts: the text, then each file. Images become
/// `image_url` data URLs when `can_see_images`, PDFs become `file`
/// parts, text files are inlined, and anything else is named only.
pub(super) fn user_content_with_attachments(
    text: &str,
    attachments: &Attachments,
    can_see_images: bool,
) -> serde_json::Value {
    if attachments.files.is_empty() && attachments.skipped.is_empty() {
        return serde_json::Value::String(text.to_owned());
    }
    let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
    parts.push(serde_json::json!({
        "type": "text",
        "text": "The user's source files follow. Use their content for the design. \
                 Use an image file as <img src='/uploads/{name}'>.",
    }));
    let mut budget = ATTACHMENT_TEXT_TOTAL_LIMIT_BYTES;
    let mut left_out = Vec::new();
    for file in &attachments.files {
        let before = budget;
        parts.extend(attachment_parts(file, can_see_images, &mut budget));
        if before == 0 && is_text_attachment(file) {
            left_out.push(file.name.clone());
        }
    }
    if !left_out.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": format!(
                "Named only, the request has no room for more text: {}. \
                 Ask the user for the part you need.",
                left_out.join(", ")
            ),
        }));
    }
    if !attachments.skipped.is_empty() {
        parts.push(serde_json::json!({
            "type": "text",
            "text": format!(
                "Not attached, over the size cap: {}.",
                attachments.skipped.join(", ")
            ),
        }));
    }
    serde_json::Value::Array(parts)
}

/// The content parts for one attached file. `budget` is the text the
/// request may still inline; an inlined file takes its share.
pub(super) fn attachment_parts(
    file: &UploadAttachment,
    can_see_images: bool,
    budget: &mut usize,
) -> Vec<serde_json::Value> {
    let size = describe_size(file.bytes.len());
    let label = |detail: &str| {
        serde_json::json!({
            "type": "text",
            "text": format!("File {} ({}, {size}){detail}", file.name, file.content_type),
        })
    };
    if file.content_type.starts_with("image/") {
        if !can_see_images {
            return vec![label(&format!(
                ": an image this model cannot see here. Use it as <img src='/uploads/{}'> when it fits.",
                file.name
            ))];
        }
        return vec![
            label(":"),
            serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!(
                        "data:{};base64,{}",
                        file.content_type,
                        base64_encode(&file.bytes)
                    ),
                },
            }),
        ];
    }
    if file.content_type == "application/pdf" {
        return vec![
            label(":"),
            serde_json::json!({
                "type": "file",
                "file": {
                    "filename": file.name,
                    "file_data": format!(
                        "data:application/pdf;base64,{}",
                        base64_encode(&file.bytes)
                    ),
                },
            }),
        ];
    }
    if file.content_type.starts_with("text/") || file.content_type == "application/json" {
        let text = String::from_utf8_lossy(&file.bytes);
        return vec![inlined_text_part(file, &text, &size, budget)];
    }
    if is_office_type(&file.content_type) {
        return match office_text(&file.content_type, &file.bytes) {
            Ok(text) => vec![inlined_text_part(file, &text, &size, budget)],
            Err(error) => vec![label(&format!(
                ": the file could not be read ({error:#}). Tell the user if you need its content."
            ))],
        };
    }
    // The type comes from the extension, and no table names every
    // source or configuration file. A file that reads as text is text.
    if let Some(text) = text_of(&file.bytes) {
        return vec![inlined_text_part(file, text, &size, budget)];
    }
    vec![label(
        ": a file this request cannot carry. Tell the user if you need its content.",
    )]
}

/// The bytes as text when they are valid UTF-8 with no NUL byte, which
/// is what a source file, a configuration file, or a log looks like.
/// A binary file fails one of the two checks in its first bytes.
pub(super) fn text_of(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

/// True when the file reaches the prompt as text: a text or JSON
/// type, an Office file, or an unknown type that reads as UTF-8.
pub(super) fn is_text_attachment(file: &UploadAttachment) -> bool {
    file.content_type.starts_with("text/")
        || file.content_type == "application/json"
        || is_office_type(&file.content_type)
        || (!file.content_type.starts_with("image/")
            && file.content_type != "application/pdf"
            && text_of(&file.bytes).is_some())
}

/// One text part with the file's text inlined, cut at
/// `ATTACHMENT_TEXT_LIMIT_BYTES` or at what is left of `budget`, on a
/// character boundary. With no budget left the file is named only.
pub(super) fn inlined_text_part(
    file: &UploadAttachment,
    text: &str,
    size: &str,
    budget: &mut usize,
) -> serde_json::Value {
    if *budget == 0 {
        return serde_json::json!({
            "type": "text",
            "text": format!(
                "File {} ({}, {size}): named only, the request has no room for its text.",
                file.name, file.content_type
            ),
        });
    }
    let limit = ATTACHMENT_TEXT_LIMIT_BYTES.min(*budget);
    let (shown, note) = if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        (&text[..end], "\n[cut: the file continues]")
    } else {
        (text, "")
    };
    *budget -= shown.len();
    serde_json::json!({
        "type": "text",
        "text": format!("File {} ({}, {size}):\n{shown}{note}", file.name, file.content_type),
    })
}
