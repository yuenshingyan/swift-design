//! Text from Office files: DOCX, PPTX, and XLSX.
//!
//! All three are zip archives of XML parts. The prompt cannot carry
//! the archive, so `office_text` reads the text out of the parts: one
//! line per paragraph in a document, one block per slide in a
//! presentation, one tab-separated line per row in a workbook. The
//! scan is a plain string walk over the text elements (`<w:t>`,
//! `<a:t>`, `<t>`), which is enough for text and needs no XML crate.

use std::io::{Cursor, Read};

use anyhow::Context;

/// Content type of a Word document.
pub(crate) const DOCX: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
/// Content type of a PowerPoint presentation.
pub(crate) const PPTX: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";
/// Content type of an Excel workbook.
pub(crate) const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// True when `content_type` is one this module reads.
pub(crate) fn is_office_type(content_type: &str) -> bool {
    matches!(content_type, DOCX | PPTX | XLSX)
}

/// The text of an Office file, by its content type.
pub(crate) fn office_text(content_type: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).context("the file is not a zip archive")?;
    match content_type {
        DOCX => document_text(&mut archive),
        PPTX => presentation_text(&mut archive),
        XLSX => workbook_text(&mut archive),
        other => anyhow::bail!("`{other}` is not an Office content type"),
    }
}

type Archive<'bytes> = zip::ZipArchive<Cursor<&'bytes [u8]>>;

/// One part of the archive as text, or `None` when it is absent.
fn read_part(archive: &mut Archive<'_>, name: &str) -> anyhow::Result<Option<String>> {
    let mut part = match archive.by_name(name) {
        Ok(part) => part,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading `{name}`")),
    };
    let mut text = String::new();
    part.read_to_string(&mut text)
        .with_context(|| format!("`{name}` is not UTF-8"))?;
    Ok(Some(text))
}

/// The parts named `{prefix}{n}.xml`, in the order of `n`.
fn numbered_parts(archive: &Archive<'_>, prefix: &str) -> Vec<(usize, String)> {
    let mut parts: Vec<(usize, String)> = archive
        .file_names()
        .filter_map(|name| {
            let number = name.strip_prefix(prefix)?.strip_suffix(".xml")?;
            number.parse().ok().map(|number| (number, name.to_owned()))
        })
        .collect();
    parts.sort();
    parts
}

/// The paragraphs of `word/document.xml`, one per line.
fn document_text(archive: &mut Archive<'_>) -> anyhow::Result<String> {
    let part = read_part(archive, "word/document.xml")?
        .context("the document has no `word/document.xml` part")?;
    Ok(paragraphs(&part, "w:t", "</w:p>").join("\n"))
}

/// The slides of `ppt/slides/slideN.xml`, in order, each headed
/// `Slide N:`.
fn presentation_text(archive: &mut Archive<'_>) -> anyhow::Result<String> {
    let mut blocks = Vec::new();
    for (number, name) in numbered_parts(archive, "ppt/slides/slide") {
        let Some(part) = read_part(archive, &name)? else {
            continue;
        };
        let lines = paragraphs(&part, "a:t", "</a:p>");
        blocks.push(format!("Slide {number}:\n{}", lines.join("\n")));
    }
    if blocks.is_empty() {
        anyhow::bail!("the presentation has no slide parts");
    }
    Ok(blocks.join("\n\n"))
}

/// The sheets of `xl/worksheets/sheetN.xml`, in order, each headed
/// `Sheet N:`, with one tab-separated line per row.
fn workbook_text(archive: &mut Archive<'_>) -> anyhow::Result<String> {
    let shared: Vec<String> = read_part(archive, "xl/sharedStrings.xml")?
        .map(|part| {
            sections(&part, "<si", "</si>")
                .iter()
                .map(|(_, item)| element_texts(item, "t").concat())
                .collect()
        })
        .unwrap_or_default();
    let mut blocks = Vec::new();
    for (number, name) in numbered_parts(archive, "xl/worksheets/sheet") {
        let Some(part) = read_part(archive, &name)? else {
            continue;
        };
        let rows: Vec<String> = sections(&part, "<row", "</row>")
            .iter()
            .map(|(_, row)| row_text(row, &shared))
            .collect();
        blocks.push(format!("Sheet {number}:\n{}", rows.join("\n")));
    }
    if blocks.is_empty() {
        anyhow::bail!("the workbook has no sheet parts");
    }
    Ok(blocks.join("\n\n"))
}

/// The cells of one `<row>`, tab-separated. A cell with `t="s"` holds
/// an index into the shared strings; an inline string holds its text
/// under `<is>`; any other cell holds its value in `<v>`.
fn row_text(row: &str, shared: &[String]) -> String {
    sections(row, "<c", "</c>")
        .iter()
        .map(|(attributes, cell)| {
            if attributes.contains("t=\"s\"") {
                let index: Option<usize> = element_texts(cell, "v").concat().trim().parse().ok();
                return index
                    .and_then(|index| shared.get(index))
                    .cloned()
                    .unwrap_or_default();
            }
            if attributes.contains("t=\"inlineStr\"") {
                return element_texts(cell, "t").concat();
            }
            element_texts(cell, "v").concat()
        })
        .collect::<Vec<String>>()
        .join("\t")
}

/// The text of each block ended by `break_tag`, with the text elements
/// named `text_tag` inside it joined. Empty blocks are dropped.
fn paragraphs(xml: &str, text_tag: &str, break_tag: &str) -> Vec<String> {
    xml.split(break_tag)
        .map(|block| element_texts(block, text_tag).concat())
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Every element that starts with the `open` marker (`<row`) and ends
/// with `close` (`</row>`), as (attributes, content). A self-closing
/// element (`<w:t/>`) has no content and is left out.
fn sections<'xml>(xml: &'xml str, open: &str, close: &str) -> Vec<(&'xml str, &'xml str)> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = find_tag_start(rest, open) {
        let after = &rest[start + open.len()..];
        let Some(open_end) = after.find('>') else {
            break;
        };
        let attributes = &after[..open_end];
        let body = &after[open_end + 1..];
        if attributes.ends_with('/') {
            rest = body;
            continue;
        }
        let Some(end) = body.find(close) else {
            break;
        };
        found.push((attributes, &body[..end]));
        rest = &body[end + close.len()..];
    }
    found
}

/// The position of `open` where it starts a tag: the marker followed
/// by `>`, a space, or `/`, so `<c` does not match `<cols`.
fn find_tag_start(xml: &str, open: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = xml[from..].find(open) {
        let start = from + found;
        let next = xml[start + open.len()..].chars().next();
        if matches!(
            next,
            Some('>') | Some(' ') | Some('/') | Some('\n') | Some('\t')
        ) {
            return Some(start);
        }
        from = start + open.len();
    }
    None
}

/// The unescaped text of every `<tag ...>...</tag>` element in `xml`.
fn element_texts(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    sections(xml, &open, &close)
        .iter()
        .map(|(_, content)| unescape(content))
        .collect()
}

/// The text with the XML entities replaced. HTML shares the five named
/// entities and the numeric form, so `capture.rs` uses it too.
pub(crate) fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(';') else {
            out.push_str(after);
            return out;
        };
        let entity = &after[1..end];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => match numeric_entity(entity) {
                Some(character) => out.push(character),
                None => out.push_str(&after[..=end]),
            },
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The character of a `#nnn` or `#xhh` entity body.
fn numeric_entity(entity: &str) -> Option<char> {
    let body = entity.strip_prefix('#')?;
    let code = match body.strip_prefix('x').or_else(|| body.strip_prefix('X')) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => body.parse().ok()?,
    };
    char::from_u32(code)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    fn archive_of(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in parts {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_document_reads_one_line_per_paragraph() {
        let bytes = archive_of(&[(
            "word/document.xml",
            "<w:document><w:body>\
             <w:p><w:r><w:t>Quarterly </w:t></w:r><w:r><w:t xml:space=\"preserve\">plan &amp; budget</w:t></w:r></w:p>\
             <w:p><w:r><w:t/></w:r></w:p>\
             <w:p><w:r><w:t>Second &#8220;quoted&#x201D;</w:t></w:r></w:p>\
             </w:body></w:document>",
        )]);
        let text = office_text(DOCX, &bytes).unwrap();
        assert_eq!(
            text,
            "Quarterly plan & budget\nSecond \u{201c}quoted\u{201d}"
        );
    }

    #[test]
    fn a_presentation_reads_the_slides_in_order() {
        let slide = |title: &str| {
            format!(
                "<p:sld><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p>\
                 <a:p><a:r><a:t>One</a:t></a:r><a:r><a:t> point</a:t></a:r></a:p></p:txBody></p:sld>"
            )
        };
        let second = slide("Two");
        let tenth = slide("Ten");
        let first = slide("One");
        let bytes = archive_of(&[
            ("ppt/slides/slide2.xml", second.as_str()),
            ("ppt/slides/slide10.xml", tenth.as_str()),
            ("ppt/slides/slide1.xml", first.as_str()),
            ("ppt/slides/_rels/slide1.xml.rels", "<Relationships/>"),
        ]);
        let text = office_text(PPTX, &bytes).unwrap();
        assert_eq!(
            text,
            "Slide 1:\nOne\nOne point\n\nSlide 2:\nTwo\nOne point\n\nSlide 10:\nTen\nOne point"
        );
    }

    #[test]
    fn a_workbook_reads_shared_and_inline_strings_and_numbers() {
        let bytes = archive_of(&[
            (
                "xl/sharedStrings.xml",
                "<sst><si><t>Region</t></si><si><r><t>Re</t></r><r><t>venue</t></r></si><si><t>North</t></si></sst>",
            ),
            (
                "xl/worksheets/sheet1.xml",
                "<worksheet><cols><col min=\"1\" max=\"2\"/></cols><sheetData>\
                 <row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>\
                 <row r=\"2\"><c r=\"A2\" t=\"s\"><v>2</v></c><c r=\"B2\"><v>1200.5</v></c>\
                 <c r=\"C2\" t=\"inlineStr\"><is><t>note</t></is></c></row>\
                 </sheetData></worksheet>",
            ),
        ]);
        let text = office_text(XLSX, &bytes).unwrap();
        assert_eq!(text, "Sheet 1:\nRegion\tRevenue\nNorth\t1200.5\tnote");
    }

    #[test]
    fn a_file_that_is_not_a_zip_is_reported() {
        let error = office_text(DOCX, b"plain text").unwrap_err();
        assert!(error.to_string().contains("not a zip archive"));
        let empty = archive_of(&[("other.xml", "<x/>")]);
        let error = office_text(DOCX, &empty).unwrap_err();
        assert!(error.to_string().contains("word/document.xml"));
        let error = office_text(PPTX, &empty).unwrap_err();
        assert!(error.to_string().contains("no slide parts"));
        let error = office_text("text/plain", &empty).unwrap_err();
        assert!(error.to_string().contains("not an Office content type"));
    }

    #[test]
    fn only_the_three_office_types_are_read() {
        assert!(is_office_type(DOCX));
        assert!(is_office_type(PPTX));
        assert!(is_office_type(XLSX));
        assert!(!is_office_type("application/pdf"));
        assert!(!is_office_type("application/zip"));
    }

    #[test]
    fn a_tag_prefix_does_not_match_a_longer_tag() {
        assert_eq!(element_texts("<cols><c>x</c></cols>", "c"), ["x"]);
        assert_eq!(element_texts("<w:t/><w:t>y</w:t>", "w:t"), ["y"]);
        assert_eq!(
            element_texts("<w:t xml:space=\"preserve\"/><w:t>z</w:t>", "w:t"),
            ["z"]
        );
        assert_eq!(
            unescape("a &lt; b &amp;&amp; c &unknown; &#65;"),
            "a < b && c &unknown; A"
        );
    }
}
