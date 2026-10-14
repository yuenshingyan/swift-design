use super::*;

/// One zip with `{id}-email-{n}.html` per email, 1-based, in order,
/// plus `{id}-subjects.txt` with the subject and preheader lines.
pub(super) fn pack_email_html_files(
    id: &str,
    files: &[email_html::EmailHtmlFile],
) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    let mut subjects = String::new();
    for (index, file) in files.iter().enumerate() {
        writer.start_file(format!("{id}-email-{}.html", index + 1), options)?;
        writer.write_all(file.html.as_bytes())?;
        subjects.push_str(&format!(
            "Email {}
Subject: {}
Preheader: {}

",
            index + 1,
            file.subject.as_deref().unwrap_or(""),
            file.preheader.as_deref().unwrap_or("")
        ));
    }
    writer.start_file(format!("{id}-subjects.txt"), options)?;
    writer.write_all(subjects.as_bytes())?;
    Ok(writer.finish()?.into_inner())
}

/// One zip with `{id}-{unit}-{n}.png` per image, 1-based, in order. A
/// PNG is compressed already, so the entries are stored as they are.
pub(super) fn pack_unit_images(
    id: &str,
    unit: &str,
    images: &[Vec<u8>],
) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (index, image) in images.iter().enumerate() {
        writer.start_file(format!("{id}-{unit}-{}.png", index + 1), options)?;
        writer.write_all(image)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// The mailing twin of `inline_uploaded_images`: rewrites email html
/// and css.
pub(super) async fn inline_uploaded_email_images(
    mailing: &mut Mailing,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = mailing
        .emails
        .iter()
        .flat_map(|email| [Some(email.html.as_str()), email.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for email in &mut mailing.emails {
        email.html = replace_upload_references(&email.html, &data_uris);
        if let Some(css) = &email.css {
            email.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// Replaces every `/uploads/{name}` reference in screen html and css
/// with a `data:` URI: `<img src>`, `href`, inline `style` backgrounds,
/// and `url()` in the screen CSS. Names that are missing or unsafe stay
/// as written, so the export still succeeds.
pub(super) async fn inline_uploaded_images(
    design: &mut Design,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = design
        .screens
        .iter()
        .flat_map(|screen| [Some(screen.html.as_str()), screen.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for screen in &mut design.screens {
        screen.html = replace_upload_references(&screen.html, &data_uris);
        if let Some(css) = &screen.css {
            screen.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// The deck twin of `inline_uploaded_images`: rewrites slide html and
/// css.
pub(super) async fn inline_uploaded_slide_images(
    deck: &mut Deck,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = deck
        .slides
        .iter()
        .flat_map(|slide| [Some(slide.html.as_str()), slide.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for slide in &mut deck.slides {
        slide.html = replace_upload_references(&slide.html, &data_uris);
        if let Some(css) = &slide.css {
            slide.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// The document twin of `inline_uploaded_images`: rewrites page html
/// and css.
pub(super) async fn inline_uploaded_page_images(
    document: &mut Document,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = document
        .pages
        .iter()
        .flat_map(|page| [Some(page.html.as_str()), page.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for page in &mut document.pages {
        page.html = replace_upload_references(&page.html, &data_uris);
        if let Some(css) = &page.css {
            page.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// The social twin of `inline_uploaded_images`: rewrites frame html and
/// css.
pub(super) async fn inline_uploaded_frame_images(
    social: &mut Social,
    uploads: &UploadStore,
) -> anyhow::Result<()> {
    let texts: Vec<&str> = social
        .frames
        .iter()
        .flat_map(|frame| [Some(frame.html.as_str()), frame.css.as_deref()])
        .flatten()
        .collect();
    let data_uris = collect_data_uris(&texts, uploads).await?;
    if data_uris.is_empty() {
        return Ok(());
    }
    for frame in &mut social.frames {
        frame.html = replace_upload_references(&frame.html, &data_uris);
        if let Some(css) = &frame.css {
            frame.css = Some(replace_upload_references(css, &data_uris));
        }
    }
    Ok(())
}

/// One `data:` URI per stored upload that `texts` reference. Names that
/// are unsafe or missing get no entry.
pub(super) async fn collect_data_uris(
    texts: &[&str],
    uploads: &UploadStore,
) -> anyhow::Result<HashMap<String, String>> {
    let mut names: Vec<String> = Vec::new();
    for text in texts {
        collect_upload_names(text, &mut names);
    }
    let mut data_uris: HashMap<String, String> = HashMap::new();
    for name in names {
        if !is_stored_name(&name) {
            continue;
        }
        if let Some(bytes) = uploads.read(&name).await? {
            data_uris.insert(
                name.clone(),
                format!(
                    "data:{content_type};base64,{data}",
                    content_type = content_type_of(&name),
                    data = base64_encode(&bytes),
                ),
            );
        }
    }
    Ok(data_uris)
}

/// Adds every upload name referenced in `text` to `names`, once.
pub(super) fn collect_upload_names(text: &str, names: &mut Vec<String>) {
    for (_, name, _) in upload_references(text) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
}

/// Every `/uploads/{name}` reference in `text` as (start, name, end),
/// where `end` is the index right after the name.
pub(super) fn upload_references(text: &str) -> Vec<(usize, String, usize)> {
    let mut references = Vec::new();
    let mut search_from = 0;
    while let Some(found) = text[search_from..].find("uploads/") {
        let start = search_from + found;
        let name_start = start + "uploads/".len();
        let name: String = text[name_start..]
            .chars()
            .take_while(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || *character == '-'
                    || *character == '.'
            })
            .collect();
        let reference_start = if start > 0 && text.as_bytes()[start - 1] == b'/' {
            start - 1
        } else {
            start
        };
        let end = name_start + name.len();
        if !name.is_empty() {
            references.push((reference_start, name, end));
        }
        search_from = end.max(start + 1);
    }
    references
}

/// Substitutes data URIs for the upload references that have one.
pub(super) fn replace_upload_references(text: &str, data_uris: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, name, end) in upload_references(text) {
        if start < cursor {
            continue;
        }
        result.push_str(&text[cursor..start]);
        match data_uris.get(&name) {
            Some(data_uri) => result.push_str(data_uri),
            None => result.push_str(&text[start..end]),
        }
        cursor = end;
    }
    result.push_str(&text[cursor..]);
    result
}

/// Standard base64 with padding. Written here to keep the export free
/// of a dependency for one small encoding. Also used by the login
/// flow's PKCE values.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[(group >> 18) as usize & 0x3f] as char);
        encoded.push(ALPHABET[(group >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(group >> 6) as usize & 0x3f] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[group as usize & 0x3f] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}
