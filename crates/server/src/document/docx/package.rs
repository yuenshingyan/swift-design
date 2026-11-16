use super::*;

/// One relationship element.
fn relationship(id: &str, kind: &str, target: &str) -> String {
    format!("<Relationship Id=\"{id}\" Type=\"{RELATIONSHIP_BASE}/{kind}\" Target=\"{target}\"/>")
}

/// A relationships part around `relationships`.
fn relationships_part(relationships: &str) -> String {
    format!(
        "{XML_HEADER}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
    )
}

/// The document part's relationships: the styles, then one per picture.
pub fn document_relationships_part(pictures: &[Picture]) -> String {
    let mut relationships = relationship("rId1", "styles", "styles.xml");
    for (index, picture) in pictures.iter().enumerate() {
        relationships.push_str(&relationship(
            &picture_relationship(index),
            "image",
            &format!("media/image{}.{}", index + 1, picture.extension),
        ));
    }
    relationships_part(&relationships)
}

/// The content types part.
pub fn content_types_part() -> String {
    format!(
        "{XML_HEADER}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
         <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
         <Override PartName=\"/word/document.xml\" ContentType=\"{DOCX_CONTENT_TYPE}.main+xml\"/>\
         <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
         <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
         </Types>"
    )
}

/// The core properties part: the title.
fn core_part(title: &str) -> String {
    format!(
        "{XML_HEADER}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
         <dc:title>{}</dc:title><dc:creator>Swift Design</dc:creator></cp:coreProperties>",
        escape_xml(title)
    )
}

/// Adds one file to the ZIP.
fn add_part(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    name: &str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    writer.start_file(name, options)?;
    writer.write_all(bytes)?;
    Ok(())
}

/// Writes the whole package as ZIP bytes.
pub fn build_package(input: &DocumentInput) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    add_part(
        &mut writer,
        "[Content_Types].xml",
        content_types_part().as_bytes(),
    )?;
    add_part(
        &mut writer,
        "_rels/.rels",
        relationships_part(
            &(relationship("rId1", "officeDocument", "word/document.xml")
                + &relationship("rId2", "metadata/core-properties", "docProps/core.xml")),
        )
        .as_bytes(),
    )?;
    add_part(
        &mut writer,
        "docProps/core.xml",
        core_part(&input.title).as_bytes(),
    )?;
    add_part(
        &mut writer,
        "word/document.xml",
        document_part(input).as_bytes(),
    )?;
    add_part(
        &mut writer,
        "word/_rels/document.xml.rels",
        document_relationships_part(&input.pictures).as_bytes(),
    )?;
    add_part(
        &mut writer,
        "word/styles.xml",
        styles_part(&input.theme).as_bytes(),
    )?;
    for (index, picture) in input.pictures.iter().enumerate() {
        add_part(
            &mut writer,
            &format!("word/media/image{}.{}", index + 1, picture.extension),
            &picture.bytes,
        )?;
    }
    Ok(writer.finish()?.into_inner())
}
