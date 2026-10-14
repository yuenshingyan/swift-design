//! DOCX export: a flowing Word document from the pages of a document.
//!
//! `GET /documents/{id}/export.docx` walks the HTML of every page into
//! paragraphs, headings, list items, quotes, code, tables, and pictures,
//! and packs them as a `.docx` ZIP with a page break between pages. A
//! Word file flows, so the page CSS is not carried: the theme's fonts
//! and colors go into the styles part, and the structure of the HTML
//! decides the rest. Inline SVG has no Word shape and is left out.
//! Notes are not exported. The XML build takes blocks and bytes only,
//! so it runs in tests with no browser. No XML crate: the walk is the
//! same string scan `office.rs` reads Office files with.

use std::io::{Cursor, Write};

use design_model::{Document, Paper, Theme, markup::decode_entities};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::pptx::escape_xml;
use crate::uploads::{UploadStore, is_stored_name};

/// The content type of a `.docx` download.
pub const DOCX_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// EMU per twip (a twentieth of a point).
const EMU_PER_TWIP: i64 = 635;

/// EMU per CSS pixel at 96 dpi.
const EMU_PER_PIXEL: i64 = 9525;

/// The page margin on every side, in twips: one inch.
const PAGE_MARGIN_TWIPS: i64 = 1440;

/// The indent of a list item or a quote, in twips: half an inch.
const INDENT_TWIPS: i64 = 720;

/// Image extensions Word opens. Others are left out.
const PICTURE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "gif"];

/// Tags that start a block of their own, so text before and after them
/// lands in different paragraphs.
const BLOCK_TAGS: [&str; 20] = [
    "address",
    "article",
    "aside",
    "blockquote",
    "div",
    "dd",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "li",
    "p",
    "section",
];

const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const WORD_NAMESPACES: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";
const RELATIONSHIP_BASE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// One run of text with its inline style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// The text. A newline is a line break inside the paragraph.
    pub text: String,
    /// True inside `<strong>` or `<b>`.
    pub is_bold: bool,
    /// True inside `<em>` or `<i>`.
    pub is_italic: bool,
    /// True inside `<code>`.
    pub is_code: bool,
}

/// One block of the Word body, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A heading of level 1 to 6.
    Heading(u8, Vec<Run>),
    /// A paragraph.
    Paragraph(Vec<Run>),
    /// One list item. `number` is `Some` in an ordered list.
    ListItem {
        /// The 1-based position in an ordered list, or `None` for a bullet.
        number: Option<usize>,
        /// The item text.
        runs: Vec<Run>,
    },
    /// A block quote.
    Quote(Vec<Run>),
    /// Preformatted code, lines joined by newlines.
    Code(String),
    /// A table: rows of cells, each cell its runs. The first row is the
    /// header when the HTML marked it with `<th>`.
    Table {
        /// True when the first row is a header row.
        has_header: bool,
        /// The rows.
        rows: Vec<Vec<Vec<Run>>>,
    },
    /// An image by its `src`, as written.
    Image(String),
    /// A page break: the next block starts a new sheet.
    PageBreak,
}

/// One image file that goes into the package as an inline picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// The `src` the blocks name it by.
    pub src: String,
    /// File extension without the dot: `png`, `jpg`, `jpeg`, or `gif`.
    pub extension: String,
    /// The image bytes.
    pub bytes: Vec<u8>,
}

/// Everything the XML build needs.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentInput {
    /// The document title, for the core metadata.
    pub title: String,
    /// The theme: fonts and colors for the styles part.
    pub theme: Theme,
    /// The paper: the page size.
    pub paper: Paper,
    /// The body, page by page, with page breaks between them.
    pub blocks: Vec<Block>,
    /// Pictures whose bytes could be read.
    pub pictures: Vec<Picture>,
}

/// Walks the pages and packs the document as a `.docx`.
pub async fn export_document(
    document: &Document,
    uploads: &UploadStore,
) -> anyhow::Result<Vec<u8>> {
    let blocks = document_blocks(document);
    let pictures = load_pictures(&blocks, uploads).await?;
    build_package(&DocumentInput {
        title: document.title.clone(),
        theme: document.theme.clone(),
        paper: document.paper,
        blocks,
        pictures,
    })
}

/// The body blocks of the whole document: every page's blocks, with a
/// page break between pages.
pub fn document_blocks(document: &Document) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (index, page) in document.pages.iter().enumerate() {
        if index > 0 {
            blocks.push(Block::PageBreak);
        }
        blocks.extend(blocks_from_html(&page.html));
    }
    blocks
}

/// Reads the bytes behind every `/uploads/{name}` image the blocks
/// name and Word can show, once per image.
async fn load_pictures(blocks: &[Block], uploads: &UploadStore) -> anyhow::Result<Vec<Picture>> {
    let mut pictures: Vec<Picture> = Vec::new();
    for block in blocks {
        let Block::Image(src) = block else {
            continue;
        };
        if pictures.iter().any(|picture| &picture.src == src) {
            continue;
        }
        let Some(name) = src.strip_prefix("/uploads/") else {
            continue;
        };
        let extension = name
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !PICTURE_EXTENSIONS.contains(&extension.as_str()) || !is_stored_name(name) {
            continue;
        }
        if let Some(bytes) = uploads.read(name).await? {
            pictures.push(Picture {
                src: src.clone(),
                extension,
                bytes,
            });
        }
    }
    Ok(pictures)
}

/// The blocks of one HTML fragment, in reading order.
pub fn blocks_from_html(html: &str) -> Vec<Block> {
    let mut walker = Walker::default();
    walker.walk(html);
    walker.finish()
}

/// What an open block tag makes of the text inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading(u8),
    ListItem,
    Quote,
}

/// A table under construction: its rows and the open cell.
#[derive(Debug, Default)]
struct TableBuilder {
    has_header: bool,
    rows: Vec<Vec<Vec<Run>>>,
    cell: Option<Vec<Run>>,
}

/// The state of one walk over an HTML fragment.
#[derive(Debug, Default)]
struct Walker {
    blocks: Vec<Block>,
    /// The runs of the open block, when text has started one.
    current: Vec<Run>,
    /// What the open block is. `None` outside every block tag: text
    /// there starts a paragraph.
    kind: Option<BlockKind>,
    /// The open lists, innermost last: the next number of an ordered
    /// list, or `None` for a bullet list.
    lists: Vec<Option<usize>>,
    /// The number the open list item took, when it is ordered.
    item_number: Option<usize>,
    bold_depth: usize,
    italic_depth: usize,
    code_depth: usize,
    /// Nesting depth inside `<pre>`: the text keeps its whitespace.
    pre_depth: usize,
    /// The text inside `<pre>`.
    code: String,
    table: Option<TableBuilder>,
    /// True while the last text ended in whitespace, so the next run
    /// adds none.
    is_after_space: bool,
}

impl Walker {
    /// Walks `html` tag by tag.
    fn walk(&mut self, html: &str) {
        let mut rest = html;
        while !rest.is_empty() {
            let Some(open) = rest.find('<') else {
                self.text(rest);
                break;
            };
            self.text(&rest[..open]);
            let Some(close) = rest[open..].find('>') else {
                break;
            };
            let tag = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            rest = self.tag(tag, rest);
        }
    }

    /// Handles one tag and returns the remaining input. An `<svg>` is
    /// skipped whole: Word has no shape for it.
    fn tag<'html>(&mut self, tag: &str, rest: &'html str) -> &'html str {
        let is_closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|character: char| character.is_whitespace() || character == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if name == "svg" && !is_closing {
            tracing::debug!("docx export: an inline svg is left out");
            return rest
                .find("</svg>")
                .map(|end| &rest[end + "</svg>".len()..])
                .unwrap_or("");
        }
        if is_closing {
            self.close(&name);
        } else {
            self.open(&name, tag);
        }
        rest
    }

    /// Handles an opening tag.
    fn open(&mut self, name: &str, tag: &str) {
        match name {
            "strong" | "b" => self.bold_depth += 1,
            "em" | "i" => self.italic_depth += 1,
            "code" if self.pre_depth == 0 => self.code_depth += 1,
            "br" => self.line_break(),
            "img" => {
                self.flush();
                if let Some(src) = attribute(tag, "src") {
                    self.blocks.push(Block::Image(src));
                }
            }
            "pre" => {
                self.flush();
                self.pre_depth += 1;
            }
            "ul" => self.lists.push(None),
            "ol" => self.lists.push(Some(1)),
            "table" => {
                self.flush();
                self.table = Some(TableBuilder::default());
            }
            "tr" => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            "td" | "th" => self.open_cell(name == "th"),
            "hr" => {
                self.flush();
                self.blocks.push(Block::Paragraph(Vec::new()));
            }
            _ if BLOCK_TAGS.contains(&name) => self.open_block(name),
            _ => {}
        }
    }

    /// Handles a closing tag.
    fn close(&mut self, name: &str) {
        match name {
            "strong" | "b" => self.bold_depth = self.bold_depth.saturating_sub(1),
            "em" | "i" => self.italic_depth = self.italic_depth.saturating_sub(1),
            "code" if self.pre_depth == 0 => self.code_depth = self.code_depth.saturating_sub(1),
            "pre" => {
                self.pre_depth = self.pre_depth.saturating_sub(1);
                if self.pre_depth == 0 {
                    let code = std::mem::take(&mut self.code);
                    let code = code.trim_matches('\n').to_owned();
                    if !code.is_empty() {
                        self.blocks.push(Block::Code(code));
                    }
                }
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.pop();
            }
            "td" | "th" => self.close_cell(),
            "table" => {
                self.close_cell();
                if let Some(table) = self.table.take() {
                    let rows: Vec<Vec<Vec<Run>>> = table
                        .rows
                        .into_iter()
                        .filter(|row| !row.is_empty())
                        .collect();
                    if !rows.is_empty() {
                        self.blocks.push(Block::Table {
                            has_header: table.has_header,
                            rows,
                        });
                    }
                }
            }
            _ if BLOCK_TAGS.contains(&name) => {
                self.flush();
                self.kind = None;
            }
            _ => {}
        }
    }

    /// Starts a block for a block tag. A wrapper like `<div>` opens no
    /// block of its own, so its text starts a paragraph.
    fn open_block(&mut self, name: &str) {
        self.flush();
        self.kind = match name {
            "h1" => Some(BlockKind::Heading(1)),
            "h2" => Some(BlockKind::Heading(2)),
            "h3" => Some(BlockKind::Heading(3)),
            "h4" => Some(BlockKind::Heading(4)),
            "h5" => Some(BlockKind::Heading(5)),
            "h6" => Some(BlockKind::Heading(6)),
            "li" => {
                self.item_number = match self.lists.last_mut() {
                    Some(Some(next)) => {
                        let number = *next;
                        *next += 1;
                        Some(number)
                    }
                    _ => None,
                };
                Some(BlockKind::ListItem)
            }
            "blockquote" => Some(BlockKind::Quote),
            "p" | "dd" | "dt" | "figcaption" => Some(BlockKind::Paragraph),
            _ => None,
        };
    }

    /// Starts a table cell.
    fn open_cell(&mut self, is_header: bool) {
        self.close_cell();
        if let Some(table) = &mut self.table {
            if table.rows.is_empty() {
                table.rows.push(Vec::new());
            }
            if is_header && table.rows.len() == 1 {
                table.has_header = true;
            }
            table.cell = Some(Vec::new());
            self.is_after_space = true;
        }
    }

    /// Ends the open table cell, if any.
    fn close_cell(&mut self) {
        if let Some(table) = &mut self.table
            && let Some(cell) = table.cell.take()
            && let Some(row) = table.rows.last_mut()
        {
            row.push(trimmed(cell));
        }
    }

    /// Adds a line break to the open block.
    fn line_break(&mut self) {
        if self.pre_depth > 0 {
            self.code.push('\n');
            return;
        }
        self.push_run("\n");
        self.is_after_space = true;
    }

    /// Adds text: raw inside `<pre>`, collapsed elsewhere.
    fn text(&mut self, raw: &str) {
        if raw.is_empty() {
            return;
        }
        let decoded = decode_entities(raw);
        if self.pre_depth > 0 {
            self.code.push_str(&decoded);
            return;
        }
        let collapsed = collapse_whitespace(&decoded, self.is_after_space);
        if collapsed.is_empty() {
            return;
        }
        self.is_after_space = collapsed.ends_with(' ');
        self.push_run(&collapsed);
    }

    /// Adds one run with the open inline style to the open cell or
    /// block.
    fn push_run(&mut self, text: &str) {
        let run = Run {
            text: text.to_owned(),
            is_bold: self.bold_depth > 0,
            is_italic: self.italic_depth > 0,
            is_code: self.code_depth > 0,
        };
        if let Some(table) = &mut self.table
            && let Some(cell) = &mut table.cell
        {
            cell.push(run);
            return;
        }
        self.current.push(run);
    }

    /// Ends the open block, when it holds text.
    fn flush(&mut self) {
        let runs = trimmed(std::mem::take(&mut self.current));
        self.is_after_space = true;
        if runs.is_empty() {
            return;
        }
        let block = match self.kind.unwrap_or(BlockKind::Paragraph) {
            BlockKind::Paragraph => Block::Paragraph(runs),
            BlockKind::Heading(level) => Block::Heading(level, runs),
            BlockKind::ListItem => Block::ListItem {
                number: self.item_number,
                runs,
            },
            BlockKind::Quote => Block::Quote(runs),
        };
        self.blocks.push(block);
    }

    /// The blocks, after the last open block is closed.
    fn finish(mut self) -> Vec<Block> {
        self.close_cell();
        self.flush();
        self.blocks
    }
}

/// The value of `attribute` in `tag`, when the tag has it.
fn attribute(tag: &str, attribute: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(found) = lower[search_from..].find(attribute) {
        let start = search_from + found;
        let is_word_start = start == 0
            || lower.as_bytes()[start - 1].is_ascii_whitespace()
            || lower.as_bytes()[start - 1] == b'/';
        let after = &tag[start + attribute.len()..];
        let after_trimmed = after.trim_start();
        if is_word_start && after_trimmed.starts_with('=') {
            let value = after_trimmed[1..].trim_start();
            let quote = value.chars().next()?;
            if quote == '"' || quote == '\'' {
                let inner = &value[1..];
                let end = inner.find(quote)?;
                return Some(decode_entities(&inner[..end]));
            }
            let end = value
                .find(|character: char| character.is_whitespace() || character == '>')
                .unwrap_or(value.len());
            return Some(decode_entities(&value[..end]));
        }
        search_from = start + attribute.len();
    }
    None
}

/// Collapses runs of whitespace to one space. The leading space goes
/// when the text before it ended in one.
fn collapse_whitespace(text: &str, is_after_space: bool) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut is_space = is_after_space;
    for character in text.chars() {
        if character.is_whitespace() {
            if !is_space {
                collapsed.push(' ');
            }
            is_space = true;
        } else {
            collapsed.push(character);
            is_space = false;
        }
    }
    collapsed
}

/// The runs without leading and trailing whitespace, and without
/// empty runs.
fn trimmed(mut runs: Vec<Run>) -> Vec<Run> {
    if let Some(first) = runs.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = runs.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
    runs.retain(|run| !run.text.is_empty());
    runs
}

/// The page size of `paper` in twips: (width, height).
pub fn page_size_twips(paper: Paper) -> (i64, i64) {
    match paper {
        Paper::A4 => (11906, 16838),
        Paper::Letter => (12240, 15840),
    }
}

/// The width in EMU of the text column of `paper`.
fn text_width_emu(paper: Paper) -> i64 {
    (page_size_twips(paper).0 - 2 * PAGE_MARGIN_TWIPS) * EMU_PER_TWIP
}

/// The pixel size of a PNG, GIF, or JPEG from its header. `None` when
/// the bytes are not one of them.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((width, height));
    }
    if (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) && bytes.len() >= 10 {
        let width = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let height = u32::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        return Some((width, height));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return jpeg_size(bytes);
    }
    None
}

/// The pixel size in a JPEG's first frame header.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut position = 2;
    while position + 9 < bytes.len() {
        if bytes[position] != 0xFF {
            position += 1;
            continue;
        }
        let marker = bytes[position + 1];
        let is_frame = (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker);
        if is_frame {
            let height = u32::from(u16::from_be_bytes([
                bytes[position + 5],
                bytes[position + 6],
            ]));
            let width = u32::from(u16::from_be_bytes([
                bytes[position + 7],
                bytes[position + 8],
            ]));
            return Some((width, height));
        }
        let length = usize::from(u16::from_be_bytes([
            bytes[position + 2],
            bytes[position + 3],
        ]));
        position += 2 + length.max(2);
    }
    None
}

/// The picture extent in EMU: the natural size at 96 dpi, shrunk to
/// the text column when wider. An image of unknown size takes the
/// column at 4:3.
pub fn picture_extent(bytes: &[u8], paper: Paper) -> (i64, i64) {
    let column = text_width_emu(paper);
    let Some((width, height)) =
        image_size(bytes).filter(|(width, height)| *width > 0 && *height > 0)
    else {
        return (column, column * 3 / 4);
    };
    let natural_width = i64::from(width) * EMU_PER_PIXEL;
    let shown_width = natural_width.min(column);
    let shown_height = shown_width * i64::from(height) / i64::from(width);
    (shown_width, shown_height)
}

/// `RRGGBB` for a theme `#rrggbb` color.
fn hex_of(color: &str) -> String {
    color.trim().trim_start_matches('#').to_ascii_uppercase()
}

/// The run properties for `run` with the theme's mono font for code.
fn run_properties(run: &Run, theme: &Theme) -> String {
    let mut properties = String::new();
    if run.is_bold {
        properties.push_str("<w:b/>");
    }
    if run.is_italic {
        properties.push_str("<w:i/>");
    }
    if run.is_code {
        let mono = escape_xml(&theme.fonts.mono);
        properties.push_str(&format!(
            "<w:rFonts w:ascii=\"{mono}\" w:hAnsi=\"{mono}\" w:cs=\"{mono}\"/>"
        ));
    }
    if properties.is_empty() {
        return String::new();
    }
    format!("<w:rPr>{properties}</w:rPr>")
}

/// The `w:r` elements for `runs`. A newline in a run is a line break.
pub fn runs_xml(runs: &[Run], theme: &Theme) -> String {
    let mut xml = String::new();
    for run in runs {
        let properties = run_properties(run, theme);
        for (index, line) in run.text.split('\n').enumerate() {
            if index > 0 {
                xml.push_str("<w:r><w:br/></w:r>");
            }
            if line.is_empty() {
                continue;
            }
            xml.push_str(&format!(
                "<w:r>{properties}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                escape_xml(line)
            ));
        }
    }
    xml
}

/// A paragraph with `style` and an optional indent.
fn paragraph_xml(style: &str, indent: Option<(i64, i64)>, runs: &str) -> String {
    let indent = indent
        .map(|(left, hanging)| format!("<w:ind w:left=\"{left}\" w:hanging=\"{hanging}\"/>"))
        .unwrap_or_default();
    format!("<w:p><w:pPr><w:pStyle w:val=\"{style}\"/>{indent}</w:pPr>{runs}</w:p>")
}

/// The picture paragraph for `picture`, embedded through `relationship`.
pub fn picture_xml(relationship: &str, id: usize, picture: &Picture, paper: Paper) -> String {
    let (width, height) = picture_extent(&picture.bytes, paper);
    let name = escape_xml(&picture.src);
    format!(
        "<w:p><w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
         <wp:extent cx=\"{width}\" cy=\"{height}\"/><wp:docPr id=\"{id}\" name=\"{name}\"/>\
         <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:pic><pic:nvPicPr><pic:cNvPr id=\"0\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"{relationship}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{width}\" cy=\"{height}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
         </a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
    )
}

/// The table element for `rows`. A table must be followed by a
/// paragraph, so one empty paragraph is appended.
pub fn table_xml(has_header: bool, rows: &[Vec<Vec<Run>>], theme: &Theme) -> String {
    let border = "<w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BBBBBB\"/>\
         <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BBBBBB\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BBBBBB\"/>\
         <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BBBBBB\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BBBBBB\"/>\
         <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"BBBBBB\"/>";
    let mut xml = format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblBorders>{border}</w:tblBorders>\
         <w:tblCellMar><w:left w:w=\"108\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar>\
         </w:tblPr>"
    );
    for (row_index, row) in rows.iter().enumerate() {
        let is_header = has_header && row_index == 0;
        xml.push_str("<w:tr>");
        for cell in row {
            let runs: Vec<Run> = cell
                .iter()
                .map(|run| Run {
                    is_bold: run.is_bold || is_header,
                    ..run.clone()
                })
                .collect();
            xml.push_str(&format!(
                "<w:tc><w:tcPr/><w:p>{}</w:p></w:tc>",
                runs_xml(&runs, theme)
            ));
        }
        xml.push_str("</w:tr>");
    }
    xml.push_str("</w:tbl><w:p/>");
    xml
}

/// The page break paragraph.
fn page_break_xml() -> &'static str {
    "<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"
}

/// The relationship id of the picture at `index` in the pictures list.
/// `rId1` is the styles part.
fn picture_relationship(index: usize) -> String {
    format!("rId{}", index + 2)
}

/// The body element of the document part: every block, then the
/// section with the page size.
pub fn body_xml(input: &DocumentInput) -> String {
    let theme = &input.theme;
    let mut body = String::new();
    for block in &input.blocks {
        match block {
            Block::Heading(level, runs) => body.push_str(&paragraph_xml(
                &format!("Heading{level}"),
                None,
                &runs_xml(runs, theme),
            )),
            Block::Paragraph(runs) => {
                body.push_str(&paragraph_xml("Normal", None, &runs_xml(runs, theme)))
            }
            Block::ListItem { number, runs } => {
                let marker = Run {
                    text: number.map_or("•\t".to_owned(), |number| format!("{number}.\t")),
                    ..Run::default()
                };
                let mut listed = vec![marker];
                listed.extend(runs.iter().cloned());
                body.push_str(&paragraph_xml(
                    "ListParagraph",
                    Some((INDENT_TWIPS, INDENT_TWIPS / 2)),
                    &runs_xml(&listed, theme),
                ));
            }
            Block::Quote(runs) => {
                let italic: Vec<Run> = runs
                    .iter()
                    .map(|run| Run {
                        is_italic: true,
                        ..run.clone()
                    })
                    .collect();
                body.push_str(&paragraph_xml(
                    "Quote",
                    Some((INDENT_TWIPS, 0)),
                    &runs_xml(&italic, theme),
                ));
            }
            Block::Code(code) => {
                let run = Run {
                    text: code.clone(),
                    is_code: true,
                    ..Run::default()
                };
                body.push_str(&paragraph_xml("Code", None, &runs_xml(&[run], theme)));
            }
            Block::Table { has_header, rows } => {
                body.push_str(&table_xml(*has_header, rows, theme))
            }
            Block::Image(src) => {
                let Some(index) = input
                    .pictures
                    .iter()
                    .position(|picture| &picture.src == src)
                else {
                    continue;
                };
                body.push_str(&picture_xml(
                    &picture_relationship(index),
                    index + 1,
                    &input.pictures[index],
                    input.paper,
                ));
            }
            Block::PageBreak => body.push_str(page_break_xml()),
        }
    }
    let (width, height) = page_size_twips(input.paper);
    format!(
        "<w:body>{body}<w:sectPr><w:pgSz w:w=\"{width}\" w:h=\"{height}\"/>\
         <w:pgMar w:top=\"{margin}\" w:right=\"{margin}\" w:bottom=\"{margin}\" w:left=\"{margin}\" \
         w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body>",
        margin = PAGE_MARGIN_TWIPS
    )
}

/// The document part.
pub fn document_part(input: &DocumentInput) -> String {
    format!(
        "{XML_HEADER}<w:document {WORD_NAMESPACES}>{}</w:document>",
        body_xml(input)
    )
}

/// One heading style: the heading font, bold, `size` in half-points,
/// and `color`.
fn heading_style(level: u8, size: u32, color: &str, heading_font: &str) -> String {
    format!(
        "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\"><w:name w:val=\"heading {level}\"/>\
         <w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:qFormat/>\
         <w:pPr><w:keepNext/><w:spacing w:before=\"320\" w:after=\"120\"/><w:outlineLvl w:val=\"{}\"/></w:pPr>\
         <w:rPr><w:rFonts w:ascii=\"{heading_font}\" w:hAnsi=\"{heading_font}\" w:cs=\"{heading_font}\"/>\
         <w:b/><w:color w:val=\"{color}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:style>",
        level - 1
    )
}

/// The styles part: the theme fonts and colors on the defaults, six
/// headings, and the list, quote, and code paragraphs.
pub fn styles_part(theme: &Theme) -> String {
    let body_font = escape_xml(&theme.fonts.body);
    let heading_font = escape_xml(&theme.fonts.heading);
    let mono_font = escape_xml(&theme.fonts.mono);
    let text = hex_of(&theme.colors.text);
    let accent = hex_of(&theme.colors.accent);
    let muted = hex_of(&theme.colors.muted);
    let mut headings = String::new();
    for (level, size) in [(1, 64), (2, 52), (3, 44), (4, 36), (5, 32), (6, 28)] {
        let color = if level <= 2 { &accent } else { &text };
        headings.push_str(&heading_style(level, size, color, &heading_font));
    }
    format!(
        "{XML_HEADER}<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:docDefaults><w:rPrDefault><w:rPr>\
         <w:rFonts w:ascii=\"{body_font}\" w:hAnsi=\"{body_font}\" w:cs=\"{body_font}\"/>\
         <w:color w:val=\"{text}\"/><w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/><w:lang w:val=\"en-US\"/>\
         </w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
         {headings}\
         <w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\"><w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/>\
         <w:pPr><w:spacing w:after=\"60\"/><w:contextualSpacing/></w:pPr></w:style>\
         <w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:qFormat/>\
         <w:pPr><w:pBdr><w:left w:val=\"single\" w:sz=\"18\" w:space=\"12\" w:color=\"{accent}\"/></w:pBdr></w:pPr>\
         <w:rPr><w:i/><w:color w:val=\"{muted}\"/></w:rPr></w:style>\
         <w:style w:type=\"paragraph\" w:styleId=\"Code\"><w:name w:val=\"Code\"/><w:basedOn w:val=\"Normal\"/>\
         <w:pPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F2F2F2\"/><w:spacing w:after=\"160\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
         <w:rPr><w:rFonts w:ascii=\"{mono_font}\" w:hAnsi=\"{mono_font}\" w:cs=\"{mono_font}\"/><w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/></w:rPr></w:style>\
         </w:styles>"
    )
}

mod package;

pub use package::build_package;

#[cfg(test)]
mod tests;
