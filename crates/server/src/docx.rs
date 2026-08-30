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
use crate::uploads::UploadStore;

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
        if !PICTURE_EXTENSIONS.contains(&extension.as_str())
            || !crate::uploads::is_stored_name(name)
        {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Read;

    use design_model::Paper;

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
        assert!(
            matches!(&blocks[2], Block::Paragraph(runs) if plain(runs).starts_with("Edits count"))
        );
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
}
