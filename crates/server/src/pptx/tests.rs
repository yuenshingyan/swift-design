#![allow(clippy::unwrap_used)]

use std::io::Read;

use super::*;

fn sample_deck() -> Deck {
    serde_json::from_str(include_str!("../../../../fixtures/sample-deck.json")).unwrap()
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
        shape.contains("<a:off x=\"609600\" y=\"1270000\"/><a:ext cx=\"5080000\" cy=\"762000\"/>")
    );
    assert!(shape.contains("<a:bodyPr wrap=\"square\" lIns=\"0\" tIns=\"0\" rIns=\"0\" bIns=\"0\" anchor=\"t\"><a:noAutofit/></a:bodyPr>"));
    assert!(
        shape.contains("<a:pPr algn=\"ctr\"><a:lnSpc><a:spcPts val=\"4500\"/></a:lnSpc></a:pPr>")
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
        shape.contains("<a:off x=\"635000\" y=\"317500\"/><a:ext cx=\"2540000\" cy=\"1905000\"/>")
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
    assert!(presentation.contains("<p:sldSz cx=\"12192000\" cy=\"6858000\" type=\"screen16x9\"/>"));
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
    assert!(read_part(&bytes, "[Content_Types].xml").contains("/ppt/notesSlides/notesSlide1.xml"));
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
