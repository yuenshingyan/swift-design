//! PPTX export: real DrawingML shapes over a screenshot of each slide.
//!
//! `GET /decks/{id}/export.pptx` renders the deck in the user's Chrome
//! with a measurement script, reads one record per text element and
//! per image out of the dumped DOM, screenshots every slide, and packs
//! the result as a `.pptx` ZIP. Each slide part holds the screenshot as
//! its background picture, then one picture per measured image, then
//! one text box per measured text element. The screenshot carries what
//! has no shape: gradients, transforms, inline SVG, shadows, and
//! pseudo-elements. Notes go to a notes slide. The XML build takes
//! measurement records and bytes only, so it runs in tests with no
//! browser.

use std::io::{Cursor, Write};

use design_model::{DECK_VIEWPORT, Deck};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::deck_render::{RenderOptions, render_deck_with};
use crate::screenshots;

/// EMU per CSS pixel: 12192000 EMU across a 1920 px canvas.
const EMU_PER_PIXEL: f64 = 6350.0;

/// Slide width in EMU.
const SLIDE_WIDTH_EMU: i64 = 12_192_000;

/// Slide height in EMU.
const SLIDE_HEIGHT_EMU: i64 = 6_858_000;

/// Hundredths of a point per CSS pixel: one px is 0.75 pt.
const FONT_UNITS_PER_PIXEL: f64 = 75.0;

/// The `id` of the hidden element the measurement script fills.
const MEASURE_ELEMENT_ID: &str = "swift-design-measure";

/// Image extensions PowerPoint opens. Others stay in the screenshot.
const PICTURE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "gif"];

const NAMESPACES: &str = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";
const RELATIONSHIP_BASE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// The empty group every shape tree starts with.
const GROUP_HEADER: &str = "<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>";

/// The measurement script. Loaded with `RenderOptions::is_measuring`.
/// After fonts load, it records every text element and image of every
/// slide root in CSS pixels relative to that root, and writes the JSON
/// into a hidden element with the id `swift-design-measure`.
pub(crate) const MEASURE_SCRIPT: &str = r##"(async () => {
  if (document.readyState !== 'complete') {
    await new Promise((resolve) => window.addEventListener('load', resolve, { once: true }));
  }
  if (document.fonts && document.fonts.ready) { try { await document.fonts.ready; } catch (error) {} }
  const slides = [];
  document.querySelectorAll('[data-swift-design-root]').forEach((root) => {
    const rootRect = root.getBoundingClientRect();
    const scale = rootRect.width / 1920 || 1;
    const section = root.closest('[data-swift-design-screen]');
    const box = (element) => {
      const rect = element.getBoundingClientRect();
      return { x: (rect.left - rootRect.left) / scale, y: (rect.top - rootRect.top) / scale, width: rect.width / scale, height: rect.height / scale };
    };
    const texts = [], images = [];
    root.querySelectorAll('*').forEach((element) => {
      if (element.closest('svg')) { return; }
      const style = getComputedStyle(element);
      const rect = box(element);
      if (style.display === 'none' || style.visibility === 'hidden' || rect.width <= 0 || rect.height <= 0) { return; }
      if (element.tagName.toLowerCase() === 'img') { images.push({ ...rect, src: element.getAttribute('src') || '' }); return; }
      const text = Array.from(element.childNodes).filter((node) => node.nodeType === 3).map((node) => node.textContent).join('').replace(/\s+/g, ' ').trim();
      if (!text) { return; }
      const size = parseFloat(style.fontSize);
      texts.push({ ...rect, font_family: style.fontFamily, font_size: size, font_weight: parseInt(style.fontWeight, 10) || 400, font_style: style.fontStyle, color: style.color, text_align: style.textAlign, line_height: parseFloat(style.lineHeight) || size * 1.2, text });
    });
    slides.push({ background: getComputedStyle(section).backgroundColor, texts, images });
  });
  const output = document.createElement('div');
  output.id = 'swift-design-measure';
  output.hidden = true;
  output.textContent = JSON.stringify(slides);
  document.body.appendChild(output);
})();
"##;

/// One measured text element, in CSS pixels relative to the slide root.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct TextRecord {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Box width.
    pub width: f64,
    /// Box height.
    pub height: f64,
    /// The computed `font-family` list.
    pub font_family: String,
    /// The computed `font-size` in px.
    pub font_size: f64,
    /// The computed `font-weight` as a number.
    pub font_weight: u32,
    /// The computed `font-style`: `normal`, `italic`, or `oblique`.
    pub font_style: String,
    /// The computed `color` as `rgb()` or `rgba()`.
    pub color: String,
    /// The computed `text-align`.
    pub text_align: String,
    /// The computed `line-height` in px.
    pub line_height: f64,
    /// The element's own text, whitespace collapsed.
    pub text: String,
}

/// One measured `<img>`, in CSS pixels relative to the slide root.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct ImageRecord {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Box width.
    pub width: f64,
    /// Box height.
    pub height: f64,
    /// The `src` attribute as written, like `/uploads/chart.png`.
    pub src: String,
}

/// Everything the measurement script records for one slide.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct SlideMeasure {
    /// The slide background color as `rgb()` or `rgba()`.
    pub background: String,
    /// Text elements in document order.
    pub texts: Vec<TextRecord>,
    /// Images in document order.
    pub images: Vec<ImageRecord>,
}

/// One image file that goes into the package as a picture shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    /// Where the picture sits.
    pub record: ImageRecord,
    /// File extension without the dot: `png`, `jpg`, `jpeg`, or `gif`.
    pub extension: String,
    /// The image bytes.
    pub bytes: Vec<u8>,
}

/// Everything the XML build needs for one slide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlideInput {
    /// The measurement records.
    pub measure: SlideMeasure,
    /// The slide screenshot, a PNG. It is the background layer.
    pub screenshot: Vec<u8>,
    /// Pictures whose bytes could be read.
    pub pictures: Vec<Picture>,
    /// Presenter notes, when the slide has any.
    pub notes: Option<String>,
}

/// Where the export reads image bytes and runs Chrome.
pub struct ExportSources<'a> {
    /// Upload storage, for `<img src='/uploads/...'>` bytes.
    pub uploads: &'a crate::uploads::UploadStore,
    /// The server URL Chrome loads images from.
    pub base_url: String,
}

/// Measures the deck in Chrome: one `SlideMeasure` per slide, in order.
pub async fn measure_deck(deck: &Deck, base_url: &str) -> anyhow::Result<Vec<SlideMeasure>> {
    let html = render_deck_with(
        deck,
        RenderOptions {
            is_measuring: true,
            asset_origin: Some(base_url.to_owned()),
            ..RenderOptions::default()
        },
    );
    let dom = screenshots::dump_rendered_dom(&html, base_url, DECK_VIEWPORT).await?;
    let measures = parse_measurements(&dom)?;
    if measures.len() != deck.slides.len() {
        anyhow::bail!(
            "the measurement script reported {} slides for a deck of {}",
            measures.len(),
            deck.slides.len()
        );
    }
    Ok(measures)
}

/// Reads the measurement JSON out of a dumped DOM: the text of the
/// element with the id `swift-design-measure`.
pub fn parse_measurements(dom: &str) -> anyhow::Result<Vec<SlideMeasure>> {
    let marker = format!("id=\"{MEASURE_ELEMENT_ID}\"");
    let start = dom
        .find(&marker)
        .ok_or_else(|| anyhow::anyhow!("the measurement script left no report in the page"))?;
    let rest = &dom[start..];
    let content_start = rest
        .find('>')
        .ok_or_else(|| anyhow::anyhow!("the measurement element has no closing bracket"))?
        + 1;
    let content = &rest[content_start..];
    let end = content
        .find("</div>")
        .ok_or_else(|| anyhow::anyhow!("the measurement element is not closed"))?;
    let json = content[..end]
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", "\u{a0}")
        .replace("&amp;", "&");
    Ok(serde_json::from_str(&json)?)
}

/// Measures, screenshots, and packs the deck as a `.pptx`.
pub async fn export_deck(deck: &Deck, sources: &ExportSources<'_>) -> anyhow::Result<Vec<u8>> {
    let measures = measure_deck(deck, &sources.base_url).await?;
    let mut slides = Vec::with_capacity(deck.slides.len());
    for (index, (slide, measure)) in deck.slides.iter().zip(measures).enumerate() {
        let screenshot = screenshots::screenshot_slide(deck, index, &sources.base_url).await?;
        let pictures = load_pictures(&measure.images, sources.uploads).await?;
        slides.push(SlideInput {
            measure,
            screenshot,
            pictures,
            notes: slide.notes.clone(),
        });
    }
    build_package(&slides)
}

/// Reads the bytes behind every `/uploads/{name}` image that PowerPoint
/// can show. Other images stay in the screenshot only.
async fn load_pictures(
    images: &[ImageRecord],
    uploads: &crate::uploads::UploadStore,
) -> anyhow::Result<Vec<Picture>> {
    let mut pictures = Vec::new();
    for record in images {
        let Some(name) = record.src.strip_prefix("/uploads/") else {
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
                record: record.clone(),
                extension,
                bytes,
            });
        }
    }
    Ok(pictures)
}

/// EMU for a length in CSS pixels: 1920 px is 12192000 EMU.
pub fn emu_from_pixels(pixels: f64) -> i64 {
    (pixels * EMU_PER_PIXEL).round() as i64
}

/// DrawingML `sz` for a font size in px: hundredths of a point.
pub fn font_size_from_pixels(pixels: f64) -> u32 {
    (pixels * FONT_UNITS_PER_PIXEL).round().max(100.0) as u32
}

/// `RRGGBB` for an `rgb(r, g, b)` or `rgba(r, g, b, a)` color. `None`
/// for anything else, and for a fully transparent color.
pub fn hex_from_rgb(color: &str) -> Option<String> {
    let inner = color
        .trim()
        .strip_prefix("rgba(")
        .or_else(|| color.trim().strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<f64> = inner
        .split(',')
        .map(|part| part.trim().parse::<f64>().ok())
        .collect::<Option<Vec<f64>>>()?;
    if parts.len() < 3 || parts.get(3).is_some_and(|alpha| *alpha == 0.0) {
        return None;
    }
    let channel = |value: f64| value.round().clamp(0.0, 255.0) as u8;
    Some(format!(
        "{:02X}{:02X}{:02X}",
        channel(parts[0]),
        channel(parts[1]),
        channel(parts[2])
    ))
}

/// Escapes the five XML special characters in a text or attribute value.
pub fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// The first family in a `font-family` list, without quotes.
fn first_font_family(families: &str) -> String {
    families
        .split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(['"', '\''])
        .to_owned()
}

/// The `algn` value for a CSS `text-align`.
fn alignment_of(text_align: &str) -> &'static str {
    match text_align {
        "center" => "ctr",
        "right" | "end" => "r",
        "justify" => "just",
        _ => "l",
    }
}

/// An `<a:xfrm>` for a box in CSS pixels.
fn transform(x: f64, y: f64, width: f64, height: f64) -> String {
    format!(
        "<a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm>",
        emu_from_pixels(x),
        emu_from_pixels(y),
        emu_from_pixels(width),
        emu_from_pixels(height)
    )
}

/// One text box for a measured text element.
pub fn text_shape(id: u32, record: &TextRecord) -> String {
    let color = hex_from_rgb(&record.color).unwrap_or_else(|| "000000".to_owned());
    let is_bold = u8::from(record.font_weight >= 600);
    let is_italic = u8::from(record.font_style != "normal");
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{id}\" name=\"Text {id}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr>\
         <p:spPr>{transform}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/></p:spPr>\
         <p:txBody><a:bodyPr wrap=\"square\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" anchor=\"t\"><a:noAutofit/></a:bodyPr><a:lstStyle/>\
         <a:p><a:pPr algn=\"{align}\"><a:lnSpc><a:spcPts val=\"{line_height}\"/></a:lnSpc></a:pPr>\
         <a:r><a:rPr lang=\"en-US\" sz=\"{size}\" b=\"{is_bold}\" i=\"{is_italic}\" dirty=\"0\">\
         <a:solidFill><a:srgbClr val=\"{color}\"/></a:solidFill><a:latin typeface=\"{family}\"/></a:rPr>\
         <a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>",
        transform = transform(record.x, record.y, record.width, record.height),
        align = alignment_of(&record.text_align),
        line_height = font_size_from_pixels(record.line_height),
        size = font_size_from_pixels(record.font_size),
        family = escape_xml(&first_font_family(&record.font_family)),
        text = escape_xml(&record.text),
    )
}

/// One picture shape that shows the relationship `relationship_id` in
/// the box `record`.
pub fn picture_shape(id: u32, relationship_id: &str, record: &ImageRecord) -> String {
    format!(
        "<p:pic><p:nvPicPr><p:cNvPr id=\"{id}\" name=\"Picture {id}\"/><p:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></p:cNvPicPr><p:nvPr/></p:nvPicPr>\
         <p:blipFill><a:blip r:embed=\"{relationship_id}\"/><a:stretch><a:fillRect/></a:stretch></p:blipFill>\
         <p:spPr>{transform}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:pic>",
        transform = transform(record.x, record.y, record.width, record.height),
    )
}

/// The full-slide picture record for the screenshot layer.
fn background_record() -> ImageRecord {
    ImageRecord {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
        src: String::new(),
    }
}

/// The slide part: background color, the screenshot, one picture per
/// image, one text box per text element.
pub fn slide_part(input: &SlideInput) -> String {
    let background = hex_from_rgb(&input.measure.background).unwrap_or_else(|| "FFFFFF".to_owned());
    let mut shapes = String::new();
    shapes.push_str(&picture_shape(2, "rId2", &background_record()));
    let mut id = 3;
    for (index, picture) in input.pictures.iter().enumerate() {
        shapes.push_str(&picture_shape(
            id,
            &format!("rId{}", index + 3),
            &picture.record,
        ));
        id += 1;
    }
    for record in &input.measure.texts {
        shapes.push_str(&text_shape(id, record));
        id += 1;
    }
    format!(
        "{XML_HEADER}<p:sld {NAMESPACES}><p:cSld>\
         <p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"{background}\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>\
         <p:spTree>{GROUP_HEADER}{shapes}</p:spTree></p:cSld>\
         <p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
    )
}

/// One `<Relationship>` element.
fn relationship(id: &str, kind: &str, target: &str) -> String {
    format!("<Relationship Id=\"{id}\" Type=\"{RELATIONSHIP_BASE}/{kind}\" Target=\"{target}\"/>")
}

/// A relationships part around `relationships`.
fn relationships_part(relationships: &str) -> String {
    format!(
        "{XML_HEADER}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
    )
}

/// The relationships of slide `number`: its layout, its screenshot,
/// its pictures, and its notes slide when it has one.
pub fn slide_relationships_part(number: usize, input: &SlideInput) -> String {
    let mut relationships = relationship("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml");
    relationships.push_str(&relationship(
        "rId2",
        "image",
        &format!("../media/slide{number}.png"),
    ));
    for (index, picture) in input.pictures.iter().enumerate() {
        relationships.push_str(&relationship(
            &format!("rId{}", index + 3),
            "image",
            &format!(
                "../media/slide{number}-image{}.{}",
                index + 1,
                picture.extension
            ),
        ));
    }
    if input.notes.is_some() {
        relationships.push_str(&relationship(
            &format!("rId{}", input.pictures.len() + 3),
            "notesSlide",
            &format!("../notesSlides/notesSlide{number}.xml"),
        ));
    }
    relationships_part(&relationships)
}

/// The notes slide for slide `number`: one paragraph per line of notes.
pub fn notes_part(notes: &str) -> String {
    let paragraphs: String = notes
        .lines()
        .map(|line| {
            format!(
                "<a:p><a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t>{}</a:t></a:r></a:p>",
                escape_xml(line)
            )
        })
        .collect();
    format!(
        "{XML_HEADER}<p:notes {NAMESPACES}><p:cSld><p:spTree>{GROUP_HEADER}\
         <p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Slide Image Placeholder 1\"/><p:cNvSpPr><a:spLocks noGrp=\"1\" noRot=\"1\" noChangeAspect=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"sldImg\"/></p:nvPr></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"685800\" y=\"1143000\"/><a:ext cx=\"5486400\" cy=\"3086100\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr></p:sp>\
         <p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Notes Placeholder 2\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr><p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr>\
         <p:spPr><a:xfrm><a:off x=\"685800\" y=\"4343400\"/><a:ext cx=\"5486400\" cy=\"4114800\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></p:spPr>\
         <p:txBody><a:bodyPr/><a:lstStyle/>{paragraphs}</p:txBody></p:sp>\
         </p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"
    )
}

/// The relationships of the notes slide for slide `number`.
fn notes_relationships_part(number: usize) -> String {
    let mut relationships = relationship("rId1", "slide", &format!("../slides/slide{number}.xml"));
    relationships.push_str(&relationship(
        "rId2",
        "notesMaster",
        "../notesMasters/notesMaster1.xml",
    ));
    relationships_part(&relationships)
}

/// The presentation part: the master list, the slide list, and the
/// 16:9 slide size.
pub fn presentation_part(slide_count: usize, has_notes: bool) -> String {
    let slides: String = (1..=slide_count)
        .map(|number| {
            format!(
                "<p:sldId id=\"{}\" r:id=\"rId{}\"/>",
                255 + number,
                number + 1
            )
        })
        .collect();
    let notes_master = if has_notes {
        format!(
            "<p:notesMasterIdLst><p:notesMasterId r:id=\"rId{}\"/></p:notesMasterIdLst>",
            slide_count + 3
        )
    } else {
        String::new()
    };
    format!(
        "{XML_HEADER}<p:presentation {NAMESPACES}>\
         <p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
         {notes_master}<p:sldIdLst>{slides}</p:sldIdLst>\
         <p:sldSz cx=\"{SLIDE_WIDTH_EMU}\" cy=\"{SLIDE_HEIGHT_EMU}\" type=\"screen16x9\"/>\
         <p:notesSz cx=\"6858000\" cy=\"9144000\"/></p:presentation>"
    )
}

/// The presentation relationships: master, slides, theme, notes master.
fn presentation_relationships_part(slide_count: usize, has_notes: bool) -> String {
    let mut relationships = relationship("rId1", "slideMaster", "slideMasters/slideMaster1.xml");
    for number in 1..=slide_count {
        relationships.push_str(&relationship(
            &format!("rId{}", number + 1),
            "slide",
            &format!("slides/slide{number}.xml"),
        ));
    }
    relationships.push_str(&relationship(
        &format!("rId{}", slide_count + 2),
        "theme",
        "theme/theme1.xml",
    ));
    if has_notes {
        relationships.push_str(&relationship(
            &format!("rId{}", slide_count + 3),
            "notesMaster",
            "notesMasters/notesMaster1.xml",
        ));
    }
    relationships_part(&relationships)
}

/// The content types part. `notes_numbers` lists the slides with notes.
pub fn content_types_part(slide_count: usize, notes_numbers: &[usize]) -> String {
    let presentation_base = "application/vnd.openxmlformats-officedocument.presentationml";
    let mut overrides = String::new();
    for number in 1..=slide_count {
        overrides.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{number}.xml\" ContentType=\"{presentation_base}.slide+xml\"/>"
        ));
    }
    for number in notes_numbers {
        overrides.push_str(&format!(
            "<Override PartName=\"/ppt/notesSlides/notesSlide{number}.xml\" ContentType=\"{presentation_base}.notesSlide+xml\"/>"
        ));
    }
    if !notes_numbers.is_empty() {
        overrides.push_str(&format!(
            "<Override PartName=\"/ppt/notesMasters/notesMaster1.xml\" ContentType=\"{presentation_base}.notesMaster+xml\"/>\
             <Override PartName=\"/ppt/theme/theme2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>"
        ));
    }
    format!(
        "{XML_HEADER}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Default Extension=\"jpg\" ContentType=\"image/jpeg\"/>\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
         <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
         <Override PartName=\"/ppt/presentation.xml\" ContentType=\"{presentation_base}.presentation.main+xml\"/>\
         <Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"{presentation_base}.slideMaster+xml\"/>\
         <Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"{presentation_base}.slideLayout+xml\"/>\
         <Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>\
         {overrides}</Types>"
    )
}

/// The package relationships: one, to the presentation part.
fn root_relationships_part() -> String {
    relationships_part(&relationship(
        "rId1",
        "officeDocument",
        "ppt/presentation.xml",
    ))
}

/// The slide master: a white background, one layout, empty text styles.
fn slide_master_part() -> String {
    format!(
        "{XML_HEADER}<p:sldMaster {NAMESPACES}><p:cSld>\
         <p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>\
         <p:spTree>{GROUP_HEADER}</p:spTree></p:cSld>{}\
         <p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst>\
         <p:txStyles><p:titleStyle><a:lvl1pPr/></p:titleStyle><p:bodyStyle><a:lvl1pPr/></p:bodyStyle><p:otherStyle><a:lvl1pPr/></p:otherStyle></p:txStyles>\
         </p:sldMaster>",
        color_map()
    )
}

/// The color map every master carries.
fn color_map() -> &'static str {
    "<p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>"
}

/// The one blank layout.
fn slide_layout_part() -> String {
    format!(
        "{XML_HEADER}<p:sldLayout {NAMESPACES} type=\"blank\" preserve=\"1\"><p:cSld name=\"Blank\">\
         <p:spTree>{GROUP_HEADER}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"
    )
}

/// The notes master: a white page with the shared color map.
fn notes_master_part() -> String {
    format!(
        "{XML_HEADER}<p:notesMaster {NAMESPACES}><p:cSld>\
         <p:bg><p:bgPr><a:solidFill><a:srgbClr val=\"FFFFFF\"/></a:solidFill><a:effectLst/></p:bgPr></p:bg>\
         <p:spTree>{GROUP_HEADER}</p:spTree></p:cSld>{}<p:notesStyle><a:lvl1pPr/></p:notesStyle></p:notesMaster>",
        color_map()
    )
}

/// A theme part: a neutral color scheme, Calibri, and the three fill,
/// line, effect, and background styles the schema requires.
fn theme_part(name: &str) -> String {
    let solid = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let line = "<a:ln w=\"9525\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>";
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        "{XML_HEADER}<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"{name}\"><a:themeElements>\
         <a:clrScheme name=\"{name}\"><a:dk1><a:srgbClr val=\"000000\"/></a:dk1><a:lt1><a:srgbClr val=\"FFFFFF\"/></a:lt1>\
         <a:dk2><a:srgbClr val=\"1F1F1F\"/></a:dk2><a:lt2><a:srgbClr val=\"EEEEEE\"/></a:lt2>\
         <a:accent1><a:srgbClr val=\"4F8CFF\"/></a:accent1><a:accent2><a:srgbClr val=\"ED7D31\"/></a:accent2>\
         <a:accent3><a:srgbClr val=\"A5A5A5\"/></a:accent3><a:accent4><a:srgbClr val=\"FFC000\"/></a:accent4>\
         <a:accent5><a:srgbClr val=\"5B9BD5\"/></a:accent5><a:accent6><a:srgbClr val=\"70AD47\"/></a:accent6>\
         <a:hlink><a:srgbClr val=\"0563C1\"/></a:hlink><a:folHlink><a:srgbClr val=\"954F72\"/></a:folHlink></a:clrScheme>\
         <a:fontScheme name=\"{name}\"><a:majorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme>\
         <a:fmtScheme name=\"{name}\"><a:fillStyleLst>{solid}{solid}{solid}</a:fillStyleLst>\
         <a:lnStyleLst>{line}{line}{line}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst>\
         <a:bgFillStyleLst>{solid}{solid}{solid}</a:bgFillStyleLst></a:fmtScheme>\
         </a:themeElements></a:theme>"
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

/// Adds the parts of slide `number`: the slide, its relationships, its
/// screenshot, its pictures, and its notes slide.
fn add_slide_parts(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    number: usize,
    input: &SlideInput,
) -> anyhow::Result<()> {
    add_part(
        writer,
        &format!("ppt/slides/slide{number}.xml"),
        slide_part(input).as_bytes(),
    )?;
    add_part(
        writer,
        &format!("ppt/slides/_rels/slide{number}.xml.rels"),
        slide_relationships_part(number, input).as_bytes(),
    )?;
    add_part(
        writer,
        &format!("ppt/media/slide{number}.png"),
        &input.screenshot,
    )?;
    for (index, picture) in input.pictures.iter().enumerate() {
        add_part(
            writer,
            &format!(
                "ppt/media/slide{number}-image{}.{}",
                index + 1,
                picture.extension
            ),
            &picture.bytes,
        )?;
    }
    if let Some(notes) = &input.notes {
        add_part(
            writer,
            &format!("ppt/notesSlides/notesSlide{number}.xml"),
            notes_part(notes).as_bytes(),
        )?;
        add_part(
            writer,
            &format!("ppt/notesSlides/_rels/notesSlide{number}.xml.rels"),
            notes_relationships_part(number).as_bytes(),
        )?;
    }
    Ok(())
}

/// Writes the whole package as ZIP bytes.
pub fn build_package(slides: &[SlideInput]) -> anyhow::Result<Vec<u8>> {
    let notes_numbers: Vec<usize> = slides
        .iter()
        .enumerate()
        .filter(|(_, slide)| slide.notes.is_some())
        .map(|(index, _)| index + 1)
        .collect();
    let has_notes = !notes_numbers.is_empty();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    add_part(
        &mut writer,
        "[Content_Types].xml",
        content_types_part(slides.len(), &notes_numbers).as_bytes(),
    )?;
    add_part(
        &mut writer,
        "_rels/.rels",
        root_relationships_part().as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/presentation.xml",
        presentation_part(slides.len(), has_notes).as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/_rels/presentation.xml.rels",
        presentation_relationships_part(slides.len(), has_notes).as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/slideMasters/slideMaster1.xml",
        slide_master_part().as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        relationships_part(
            &(relationship("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")
                + &relationship("rId2", "theme", "../theme/theme1.xml")),
        )
        .as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/slideLayouts/slideLayout1.xml",
        slide_layout_part().as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        relationships_part(&relationship(
            "rId1",
            "slideMaster",
            "../slideMasters/slideMaster1.xml",
        ))
        .as_bytes(),
    )?;
    add_part(
        &mut writer,
        "ppt/theme/theme1.xml",
        theme_part("Swift Design").as_bytes(),
    )?;
    if has_notes {
        add_notes_master_parts(&mut writer)?;
    }
    for (index, slide) in slides.iter().enumerate() {
        add_slide_parts(&mut writer, index + 1, slide)?;
    }
    Ok(writer.finish()?.into_inner())
}

/// Adds the notes master, its relationships, and its own theme.
fn add_notes_master_parts(writer: &mut ZipWriter<Cursor<Vec<u8>>>) -> anyhow::Result<()> {
    add_part(
        writer,
        "ppt/notesMasters/notesMaster1.xml",
        notes_master_part().as_bytes(),
    )?;
    add_part(
        writer,
        "ppt/notesMasters/_rels/notesMaster1.xml.rels",
        relationships_part(&relationship("rId1", "theme", "../theme/theme2.xml")).as_bytes(),
    )?;
    add_part(
        writer,
        "ppt/theme/theme2.xml",
        theme_part("Swift Design Notes").as_bytes(),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::io::Read;

    use super::*;

    fn sample_deck() -> Deck {
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json")).unwrap()
    }

    fn text_record() -> TextRecord {
        TextRecord {
            x: 96.0,
            y: 200.0,
            width: 800.0,
            height: 120.0,
            font_family: "\"Inter\", sans-serif".to_owned(),
            font_size: 48.0,
            font_weight: 700,
            font_style: "italic".to_owned(),
            color: "rgb(245, 245, 245)".to_owned(),
            text_align: "center".to_owned(),
            line_height: 60.0,
            text: "Tom & Jerry <3".to_owned(),
        }
    }

    fn inputs_from(deck: &Deck) -> Vec<SlideInput> {
        deck.slides
            .iter()
            .map(|slide| SlideInput {
                measure: SlideMeasure {
                    background: "rgb(16, 20, 24)".to_owned(),
                    texts: vec![text_record()],
                    images: Vec::new(),
                },
                screenshot: b"PNG".to_vec(),
                pictures: Vec::new(),
                notes: slide.notes.clone(),
            })
            .collect()
    }

    fn part_names(bytes: &[u8]) -> Vec<String> {
        let archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        archive.file_names().map(ToOwned::to_owned).collect()
    }

    fn read_part(bytes: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).unwrap();
        let mut part = String::new();
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut part)
            .unwrap();
        part
    }

    #[test]
    fn emu_from_pixels_maps_the_canvas_to_the_slide() {
        assert_eq!(emu_from_pixels(1920.0), 12_192_000);
        assert_eq!(emu_from_pixels(1080.0), 6_858_000);
        assert_eq!(emu_from_pixels(0.0), 0);
        assert_eq!(emu_from_pixels(1.5), 9525);
    }

    #[test]
    fn font_size_from_pixels_is_hundredths_of_a_point() {
        assert_eq!(font_size_from_pixels(48.0), 3600);
        assert_eq!(font_size_from_pixels(32.0), 2400);
        assert_eq!(font_size_from_pixels(0.5), 100);
    }

    #[test]
    fn hex_from_rgb_reads_rgb_and_rgba() {
        assert_eq!(hex_from_rgb("rgb(255, 0, 128)").unwrap(), "FF0080");
        assert_eq!(hex_from_rgb("rgba(16, 20, 24, 0.5)").unwrap(), "101418");
        assert!(hex_from_rgb("rgba(0, 0, 0, 0)").is_none());
        assert!(hex_from_rgb("#fff").is_none());
        assert!(hex_from_rgb("rgb(1, 2)").is_none());
    }

    #[test]
    fn escape_xml_escapes_all_five_characters() {
        assert_eq!(
            escape_xml("a & b < c > d \" e ' f"),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
        assert_eq!(escape_xml("plain"), "plain");
    }

    #[test]
    fn a_text_record_becomes_a_text_box_at_its_offset_and_extent() {
        let shape = text_shape(5, &text_record());
        assert!(shape.starts_with("<p:sp><p:nvSpPr><p:cNvPr id=\"5\" name=\"Text 5\"/>"));
        assert!(
            shape.contains(
                "<a:off x=\"609600\" y=\"1270000\"/><a:ext cx=\"5080000\" cy=\"762000\"/>"
            )
        );
        assert!(shape.contains("<a:bodyPr wrap=\"square\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" anchor=\"t\"><a:noAutofit/></a:bodyPr>"));
        assert!(
            shape.contains(
                "<a:pPr algn=\"ctr\"><a:lnSpc><a:spcPts val=\"4500\"/></a:lnSpc></a:pPr>"
            )
        );
        assert!(shape.contains("sz=\"3600\" b=\"1\" i=\"1\""));
        assert!(shape.contains("<a:srgbClr val=\"F5F5F5\"/>"));
        assert!(shape.contains("<a:latin typeface=\"Inter\"/>"));
        assert!(shape.contains("<a:t>Tom &amp; Jerry &lt;3</a:t>"));
    }

    #[test]
    fn an_image_record_becomes_a_picture() {
        let record = ImageRecord {
            x: 100.0,
            y: 50.0,
            width: 400.0,
            height: 300.0,
            src: "/uploads/chart.png".to_owned(),
        };
        let shape = picture_shape(3, "rId3", &record);
        assert!(shape.starts_with("<p:pic><p:nvPicPr><p:cNvPr id=\"3\" name=\"Picture 3\"/>"));
        assert!(shape.contains("<a:blip r:embed=\"rId3\"/>"));
        assert!(
            shape.contains(
                "<a:off x=\"635000\" y=\"317500\"/><a:ext cx=\"2540000\" cy=\"1905000\"/>"
            )
        );
    }

    #[test]
    fn the_slide_part_orders_background_pictures_and_text() {
        let mut input = inputs_from(&sample_deck()).remove(0);
        input.pictures.push(Picture {
            record: ImageRecord {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
                src: "/uploads/a.png".to_owned(),
            },
            extension: "png".to_owned(),
            bytes: b"IMG".to_vec(),
        });
        let part = slide_part(&input);
        let background = part.find("name=\"Picture 2\"").unwrap();
        let picture = part.find("name=\"Picture 3\"").unwrap();
        let text = part.find("name=\"Text 4\"").unwrap();
        assert!(background < picture && picture < text);
        assert!(part.contains("<a:srgbClr val=\"101418\"/>"));
        assert!(part.contains("<a:ext cx=\"12192000\" cy=\"6858000\"/>"));
        let relationships = slide_relationships_part(1, &input);
        assert!(relationships.contains("Target=\"../media/slide1.png\""));
        assert!(relationships.contains("Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/slide1-image1.png\""));
        assert!(relationships.contains("Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\""));
    }

    #[test]
    fn the_built_zip_holds_every_required_part() {
        let deck = sample_deck();
        let bytes = build_package(&inputs_from(&deck)).unwrap();
        let names = part_names(&bytes);
        for required in [
            "[Content_Types].xml",
            "_rels/.rels",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            "ppt/theme/theme1.xml",
            "ppt/slides/slide1.xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/media/slide1.png",
            "ppt/slides/slide3.xml",
            "ppt/media/slide3.png",
        ] {
            assert!(names.iter().any(|name| name == required), "{required}");
        }
        let presentation = read_part(&bytes, "ppt/presentation.xml");
        assert!(
            presentation.contains("<p:sldSz cx=\"12192000\" cy=\"6858000\" type=\"screen16x9\"/>")
        );
        assert_eq!(presentation.matches("<p:sldId ").count(), 3);
        assert!(read_part(&bytes, "[Content_Types].xml").contains("/ppt/slides/slide3.xml"));
    }

    #[test]
    fn a_deck_with_notes_adds_the_notes_parts() {
        let deck = sample_deck();
        assert!(deck.slides[0].notes.is_some());
        let bytes = build_package(&inputs_from(&deck)).unwrap();
        let names = part_names(&bytes);
        for required in [
            "ppt/notesMasters/notesMaster1.xml",
            "ppt/notesMasters/_rels/notesMaster1.xml.rels",
            "ppt/notesSlides/notesSlide1.xml",
            "ppt/notesSlides/_rels/notesSlide1.xml.rels",
        ] {
            assert!(names.iter().any(|name| name == required), "{required}");
        }
        let notes = read_part(&bytes, "ppt/notesSlides/notesSlide1.xml");
        assert!(notes.contains("<a:t>Open with the one-line pitch.</a:t>"));
        assert!(read_part(&bytes, "ppt/presentation.xml").contains("<p:notesMasterIdLst>"));
        assert!(
            read_part(&bytes, "[Content_Types].xml").contains("/ppt/notesSlides/notesSlide1.xml")
        );
    }

    #[test]
    fn a_deck_with_no_notes_adds_no_notes_part() {
        let mut deck = sample_deck();
        for slide in &mut deck.slides {
            slide.notes = None;
        }
        let bytes = build_package(&inputs_from(&deck)).unwrap();
        assert!(
            part_names(&bytes)
                .iter()
                .all(|name| !name.contains("notes"))
        );
        assert!(!read_part(&bytes, "ppt/presentation.xml").contains("notesMasterIdLst"));
        assert!(!read_part(&bytes, "[Content_Types].xml").contains("notes"));
        assert!(!read_part(&bytes, "ppt/slides/_rels/slide1.xml.rels").contains("notesSlide"));
    }

    #[test]
    fn the_measurement_json_parses_into_the_record_type() {
        let dom = "<html><body><main></main><div id=\"swift-design-measure\" hidden=\"\">[{\"background\":\"rgb(16, 20, 24)\",\"texts\":[{\"x\":96,\"y\":200.5,\"width\":800,\"height\":120,\"font_family\":\"Inter, sans-serif\",\"font_size\":48,\"font_weight\":700,\"font_style\":\"normal\",\"color\":\"rgb(245, 245, 245)\",\"text_align\":\"start\",\"line_height\":57.6,\"text\":\"a &lt; b &amp; c\"}],\"images\":[{\"x\":1,\"y\":2,\"width\":3,\"height\":4,\"src\":\"/uploads/editor.png\"}]}]</div></body></html>";
        let measures = parse_measurements(dom).unwrap();
        assert_eq!(measures.len(), 1);
        assert_eq!(measures[0].background, "rgb(16, 20, 24)");
        assert_eq!(measures[0].texts[0].text, "a < b & c");
        assert_eq!(measures[0].texts[0].y, 200.5);
        assert_eq!(measures[0].texts[0].font_weight, 700);
        assert_eq!(measures[0].images[0].src, "/uploads/editor.png");
        assert!(parse_measurements("<html></html>").is_err());
    }

    #[test]
    fn measurement_records_round_trip_through_json() {
        let measure = SlideMeasure {
            background: "rgb(1, 2, 3)".to_owned(),
            texts: vec![text_record()],
            images: vec![ImageRecord::default()],
        };
        let json = serde_json::to_string(&measure).unwrap();
        assert_eq!(
            serde_json::from_str::<SlideMeasure>(&json).unwrap(),
            measure
        );
    }

    #[test]
    fn helpers_pick_the_first_family_and_map_alignment() {
        assert_eq!(
            first_font_family("\"JetBrains Mono\", monospace"),
            "JetBrains Mono"
        );
        assert_eq!(first_font_family("Inter"), "Inter");
        assert_eq!(alignment_of("center"), "ctr");
        assert_eq!(alignment_of("end"), "r");
        assert_eq!(alignment_of("justify"), "just");
        assert_eq!(alignment_of("start"), "l");
    }
}
