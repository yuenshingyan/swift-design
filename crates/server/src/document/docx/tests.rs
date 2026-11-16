#![allow(clippy::unwrap_used)]

use std::io::Read;

use design_model::Paper;

use super::package::document_relationships_part;
use super::*;
use crate::test_support::sample_document;

fn plain(runs: &[Run]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect()
}

#[test]
fn headings_paragraphs_and_lists_come_out_in_order() {
    let blocks = blocks_from_html(&sample_document().pages[1].html);
    assert_eq!(blocks.len(), 8);
    assert!(matches!(&blocks[0], Block::Heading(2, runs) if plain(runs) == "Summary"));
    assert!(
        matches!(&blocks[1], Block::Paragraph(runs) if plain(runs).starts_with("The harness asks"))
    );
    assert!(matches!(&blocks[2], Block::Heading(3, runs) if plain(runs) == "What changed"));
    assert!(matches!(
        &blocks[3],
        Block::ListItem { number: None, runs } if plain(runs) == "Link capture reads a website into the run."
    ));
    assert!(matches!(&blocks[5], Block::ListItem { number: None, .. }));
    assert!(matches!(&blocks[6], Block::Heading(3, runs) if plain(runs) == "What is next"));
    assert!(matches!(&blocks[7], Block::Paragraph(..)));
}

#[test]
fn a_table_keeps_its_header_and_its_rows() {
    let blocks = blocks_from_html(&sample_document().pages[2].html);
    let Some(Block::Table { has_header, rows }) = blocks.get(1) else {
        panic!("expected a table, got {blocks:?}");
    };
    assert!(has_header);
    assert_eq!(rows.len(), 5);
    assert_eq!(plain(&rows[0][1]), "Runs");
    assert_eq!(plain(&rows[4][2]), "240");
    assert!(matches!(&blocks[2], Block::Paragraph(runs) if plain(runs).starts_with("Edits count")));
}

#[test]
fn inline_styles_breaks_ordered_lists_quotes_and_code_are_read() {
    let html = "<p>One <strong>bold</strong> and <em>soft &amp; <code>x</code></em>.<br>Next</p>\
                    <ol><li>first</li><li>second</li></ol>\
                    <blockquote>Quoted</blockquote>\
                    <pre><code>let a = 1;\nlet b = 2;</code></pre>\
                    <svg viewBox='0 0 1 1'><text>ignored</text></svg>\
                    <div>Loose <span>text</span></div>";
    let blocks = blocks_from_html(html);
    let Block::Paragraph(runs) = &blocks[0] else {
        panic!("expected a paragraph");
    };
    assert_eq!(
        runs,
        &[
            Run {
                text: "One ".to_owned(),
                ..Run::default()
            },
            Run {
                text: "bold".to_owned(),
                is_bold: true,
                ..Run::default()
            },
            Run {
                text: " and ".to_owned(),
                ..Run::default()
            },
            Run {
                text: "soft & ".to_owned(),
                is_italic: true,
                ..Run::default()
            },
            Run {
                text: "x".to_owned(),
                is_italic: true,
                is_code: true,
                ..Run::default()
            },
            Run {
                text: ".".to_owned(),
                ..Run::default()
            },
            Run {
                text: "\n".to_owned(),
                ..Run::default()
            },
            Run {
                text: "Next".to_owned(),
                ..Run::default()
            },
        ]
    );
    assert!(matches!(
        &blocks[1],
        Block::ListItem {
            number: Some(1),
            ..
        }
    ));
    assert!(matches!(
        &blocks[2],
        Block::ListItem {
            number: Some(2),
            ..
        }
    ));
    assert!(matches!(&blocks[3], Block::Quote(runs) if plain(runs) == "Quoted"));
    assert_eq!(blocks[4], Block::Code("let a = 1;\nlet b = 2;".to_owned()));
    assert!(matches!(&blocks[5], Block::Paragraph(runs) if plain(runs) == "Loose text"));
    assert_eq!(blocks.len(), 6);
}

#[test]
fn images_become_blocks_and_attributes_are_read_in_any_quote() {
    let blocks = blocks_from_html(
        "<p>Before</p><img src='/uploads/chart.png' alt='x'><img src=\"/uploads/b.jpg\"><p>After</p>",
    );
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[1], Block::Image("/uploads/chart.png".to_owned()));
    assert_eq!(blocks[2], Block::Image("/uploads/b.jpg".to_owned()));
    assert_eq!(attribute("img src=x.png", "src"), Some("x.png".to_owned()));
    assert_eq!(attribute("img data-src='y'", "src"), None);
}

#[test]
fn pages_are_joined_by_page_breaks() {
    let blocks = document_blocks(&sample_document());
    let breaks = blocks
        .iter()
        .filter(|block| **block == Block::PageBreak)
        .count();
    assert_eq!(breaks, 2);
    assert!(matches!(&blocks[0], Block::Paragraph(runs) if plain(runs) == "Quarterly report"));
    assert!(matches!(&blocks[1], Block::Heading(1, runs) if plain(runs) == "Swift Design"));
}

#[test]
fn image_sizes_come_from_the_headers() {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&800u32.to_be_bytes());
    png.extend_from_slice(&600u32.to_be_bytes());
    assert_eq!(image_size(&png), Some((800, 600)));
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&320u16.to_le_bytes());
    gif.extend_from_slice(&200u16.to_le_bytes());
    assert_eq!(image_size(&gif), Some((320, 200)));
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
    jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    jpeg.extend_from_slice(&480u16.to_be_bytes());
    jpeg.extend_from_slice(&640u16.to_be_bytes());
    jpeg.extend_from_slice(&[0; 12]);
    assert_eq!(image_size(&jpeg), Some((640, 480)));
    assert_eq!(image_size(b"not an image"), None);
}

#[test]
fn a_wide_picture_shrinks_to_the_text_column() {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&2000u32.to_be_bytes());
    png.extend_from_slice(&1000u32.to_be_bytes());
    let (width, height) = picture_extent(&png, Paper::A4);
    assert_eq!(width, (11906 - 2880) * 635);
    assert_eq!(height, width / 2);
    let (small_width, small_height) = picture_extent(
        &{
            let mut small = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            small.extend_from_slice(&100u32.to_be_bytes());
            small.extend_from_slice(&50u32.to_be_bytes());
            small
        },
        Paper::Letter,
    );
    assert_eq!(small_width, 100 * 9525);
    assert_eq!(small_height, 50 * 9525);
    let (unknown_width, unknown_height) = picture_extent(b"?", Paper::Letter);
    assert_eq!(unknown_width, (12240 - 2880) * 635);
    assert_eq!(unknown_height, unknown_width * 3 / 4);
}

#[test]
fn the_body_carries_styles_breaks_and_the_paper_size() {
    let document = sample_document();
    let input = DocumentInput {
        title: document.title.clone(),
        theme: document.theme.clone(),
        paper: Paper::Letter,
        blocks: document_blocks(&document),
        pictures: Vec::new(),
    };
    let body = body_xml(&input);
    assert!(body.contains("<w:pStyle w:val=\"Heading1\"/>"));
    assert!(body.contains("<w:pStyle w:val=\"ListParagraph\"/>"));
    assert!(body.contains("<w:t xml:space=\"preserve\">•\t</w:t>"));
    assert!(body.contains("<w:tbl>"));
    assert_eq!(body.matches("<w:br w:type=\"page\"/>").count(), 2);
    assert!(body.contains("<w:pgSz w:w=\"12240\" w:h=\"15840\"/>"));
    assert!(body.contains("Swift Design"));
    assert!(!body.contains("<div"));
}

#[test]
fn the_styles_part_carries_the_theme() {
    let styles = styles_part(&sample_document().theme);
    assert!(styles.contains("w:ascii=\"Inter\""));
    assert!(styles.contains("w:ascii=\"JetBrains Mono\""));
    assert!(styles.contains("<w:color w:val=\"1A1D21\"/>"));
    assert!(styles.contains("<w:color w:val=\"2F6FDD\"/>"));
    assert!(styles.contains("w:styleId=\"Heading6\""));
}

#[test]
fn text_is_escaped_and_pictures_are_related() {
    let picture = Picture {
        src: "/uploads/a<b>.png".to_owned(),
        extension: "png".to_owned(),
        bytes: Vec::new(),
    };
    let xml = picture_xml("rId2", 1, &picture, Paper::A4);
    assert!(xml.contains("r:embed=\"rId2\""));
    assert!(xml.contains("name=\"/uploads/a&lt;b&gt;.png\""));
    let relationships = document_relationships_part(&[picture]);
    assert!(relationships.contains("Target=\"styles.xml\""));
    assert!(relationships.contains("Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image1.png\""));
    let runs = runs_xml(
        &[Run {
            text: "a & b".to_owned(),
            is_bold: true,
            ..Run::default()
        }],
        &sample_document().theme,
    );
    assert_eq!(
        runs,
        "<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">a &amp; b</w:t></w:r>"
    );
}

#[test]
fn the_package_unzips_with_every_part() {
    let document = sample_document();
    let input = DocumentInput {
        title: document.title.clone(),
        theme: document.theme.clone(),
        paper: document.paper,
        blocks: document_blocks(&document),
        pictures: vec![Picture {
            src: "/uploads/chart.png".to_owned(),
            extension: "png".to_owned(),
            bytes: b"PNGDATA".to_vec(),
        }],
    };
    let bytes = build_package(&input).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "[Content_Types].xml",
            "_rels/.rels",
            "docProps/core.xml",
            "word/_rels/document.xml.rels",
            "word/document.xml",
            "word/media/image1.png",
            "word/styles.xml",
        ]
    );
    let mut body = String::new();
    archive
        .by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut body)
        .unwrap();
    assert!(body.starts_with(XML_HEADER));
    assert!(body.contains("<w:pgSz w:w=\"11906\" w:h=\"16838\"/>"));
    assert_eq!(body.matches("<w:br w:type=\"page\"/>").count(), 2);
    let mut core = String::new();
    archive
        .by_name("docProps/core.xml")
        .unwrap()
        .read_to_string(&mut core)
        .unwrap();
    assert!(core.contains("<dc:title>Swift Design Quarterly Report</dc:title>"));
}

#[tokio::test]
async fn the_export_reads_pictures_from_the_uploads() {
    let directory = tempfile::tempdir().unwrap();
    let store = UploadStore::new(directory.path().to_path_buf());
    store.save("report", "chart.png", b"PNGDATA").await.unwrap();
    let mut document = sample_document();
    document.pages[0].html =
        "<h1>T</h1><img src='/uploads/chart.png'><img src='/uploads/missing.png'>".to_owned();
    let bytes = export_document(&document, &store).await.unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    assert!(archive.by_name("word/media/image1.png").is_ok());
    assert!(archive.by_name("word/media/image2.png").is_err());
    let mut body = String::new();
    archive
        .by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut body)
        .unwrap();
    assert_eq!(body.matches("<w:drawing>").count(), 1);
}
