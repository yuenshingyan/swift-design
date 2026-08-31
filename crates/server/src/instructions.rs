//! The agent contract, served by the app.
//!
//! Agents get everything over HTTP: `GET /instructions` returns the
//! full build procedure as JSON, and `GET /schemas/{design,deck,brief,
//! question-set}` return the JSON Schemas generated from the Rust types
//! at runtime, so they can never go stale. No repo file is part of the
//! agent interface. Instruction strings follow Simplified Technical
//! English.
//!
//! There are six artifact kinds. A demo session writes designs
//! (screens on a device viewport). A deck session writes decks (slides
//! on a 1920 by 1080 px canvas). A document session writes documents
//! (pages on A4 or Letter paper). A social session writes socials
//! (frames on a square, portrait, story, or landscape canvas). A print
//! session writes prints (sheets on an A5 to A3, Letter, or Tabloid
//! canvas). A mailing session writes mailings (emails on a 600 px wide
//! canvas). Each kind has its own rule list.

use axum::routing::get;
use axum::{Json, Router};
use design_model::{Deck, Design, Document, Mailing, Print, QUESTIONS_PER_TURN_LIMIT, Social};

/// Rules every screen, slide, page, and frame follow. Shared by every
/// kind.
const SHARED_RULES: [&str; 6] = [
    "The server scopes your CSS to the screen, slide, or page. Write plain selectors such as `.title` or `h1`. Do not write `html`, `body`, or `:root` selectors. Do not use `@import`. `@media`, `@keyframes`, and `@font-face` are allowed.",
    "Use the theme through CSS variables: `--background`, `--text`, `--accent`, `--muted`, `--heading-font`, `--body-font`, `--mono-font`. Write other colors as #rrggbb.",
    "The server loads the theme fonts from Google Fonts. Base styles: text is 32px in the body font and text color, headings use the heading font with margin 0, paragraphs and lists have margin 0, images are block and max-width 100%.",
    "Allowed HTML: headings, text, lists, tables, `<img>`, inline `<svg>`, `<pre><code>`, `<blockquote>`, `<a>`, `<details>` with `<summary>`, `<label>`, and `<input>` with type `checkbox` or `radio`. Close every tag. Do not write `<script>`, `<style>`, `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, `<form>`, `<button>`, `<select>`, `<textarea>`, other input types, media, comments, on* attributes, javascript: URLs, or data: URLs.",
    "Images: `<img src='/uploads/{name}'>` for files in GET /uploads?session={id}. Use no other image source. Draw charts, icons, and shapes as inline SVG or CSS.",
    "Keep html under 20,000 characters and css under 10,000 characters. Use single quotes for HTML attribute values inside the JSON string.",
];

/// Design (demo) content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const DEMO_RULES: &[&str] = &[
    "A screen is one HTML fragment in `html` and one CSS block in `css`.",
    "Design each screen for the px canvas in the design's `viewport`. The default is 1440 by 900 px (desktop web). Use 390 by 844 for a phone and 1024 by 768 for a tablet. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The screen root is position: relative, the viewport size, overflow: hidden. Do not add an outer box of your own with a fixed height and overflow: hidden. Such a box hides overflow from the fit.",
    SHARED_RULES[0],
    SHARED_RULES[1],
    SHARED_RULES[2],
    SHARED_RULES[3],
    SHARED_RULES[4],
    "Give every id and @keyframes name a prefix unique to the screen, such as `s3-`.",
    "Size text for the viewport. On a 1440 px desktop canvas: titles 48 to 72px, body 18 to 24px, captions 14 to 16px, margins of at least 64px. On a 390 px phone canvas: titles 28 to 36px, body 16 to 18px, captions 12 to 14px, margins of at least 20px. Give boxes enough height for every line.",
    SHARED_RULES[5],
    "Put one idea on each screen. Put intent, states, and handoff remarks in notes. The renderer does not show notes on the screen.",
    "Every control acts. A button or a link opens a screen: write `<a href='#screen-3'>` with the screen number, counted from 1. Write the screen the link opens. Give each step of a flow its own screen.",
    "A menu or a dropdown is `<details>` with a `<summary>`. A toggle, a tab set, or a modal is `<input type='checkbox'>` or `<input type='radio'>` with a `<label for>`, styled through `:checked`. Give the input an id with the screen prefix. Do not write `<button>`. Style the `<a>` or the `<label>` as the button. Do not write a control that does nothing: the audit reports it as `static_control`.",
    "`transition` is optional. Leave it out and the design scrolls. Set it to give the design a page transition: `effect` is `none`, `fade`, `push`, `cover`, or `zoom`; `axis` is `vertical` or `horizontal`; `duration_ms` is 0 to 3000. `axis` moves `push` and `cover` only. Set it only when the user asks for one.",
];

/// Deck content rules, shared by the agent instructions and the built-in
/// generation engine. Simplified Technical English.
pub const DECK_RULES: &[&str] = &[
    "A slide is one HTML fragment in `html` and one CSS block in `css`.",
    "Design each slide for a canvas of 1920 by 1080 px. A deck has no `viewport` field. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The slide root is position: relative, 1920 by 1080 px, overflow: hidden. Do not add an outer box of your own with a fixed height and overflow: hidden. Such a box hides overflow from the fit.",
    SHARED_RULES[0],
    SHARED_RULES[1],
    SHARED_RULES[2],
    SHARED_RULES[3],
    SHARED_RULES[4],
    "Give every id and @keyframes name a prefix unique to the slide, such as `s3-`.",
    "Font sizes: titles 80 to 120px, body 32 to 44px, captions 24 to 30px. Keep all text inside the slide with margins of at least 80px. Give boxes enough height for every line.",
    SHARED_RULES[5],
    "Put one idea on each slide. Put speaker text in notes. The renderer does not show notes on the slide. The presenter view at GET /decks/{id}/present shows the notes to the speaker.",
    "`transition` is optional. Leave it out and the deck scrolls. Set it to give the deck a page transition: `effect` is `none`, `fade`, `push`, `cover`, or `zoom`; `axis` is `vertical` or `horizontal`; `duration_ms` is 0 to 3000. `axis` moves `push` and `cover` only. Set it only when the user asks for one.",
];

/// Document content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const DOCUMENT_RULES: &[&str] = &[
    "A page is one HTML fragment in `html` and one CSS block in `css`.",
    "Design each page for the px canvas of the document's `paper`: 794 by 1123 px for `a4` (the default), 816 by 1056 px for `letter`. A document has no `viewport` field. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The page root is position: relative, the paper size, overflow: hidden. Do not add an outer box of your own with a fixed height and overflow: hidden. Such a box hides overflow from the fit.",
    SHARED_RULES[0],
    SHARED_RULES[1],
    SHARED_RULES[2],
    SHARED_RULES[3],
    SHARED_RULES[4],
    "Give every id and @keyframes name a prefix unique to the page, such as `p3-`.",
    "Font sizes for print: titles 28 to 40px, headings 18 to 24px, body 14 to 17px, captions 11 to 13px. Keep all text inside the page with margins of at least 64px. Give boxes enough height for every line. Fill the page: a page that ends half empty reads as unfinished, so move content between pages until each page is full.",
    SHARED_RULES[5],
    "Write the document as a reader reads it: a title on the first page, then sections in order, with headings, paragraphs, lists, and tables. Put sources, intent, and handoff remarks in notes. The renderer does not show notes on the page.",
    "A document is read, not clicked. Do not write links between pages, `<details>`, `<input>`, or any control.",
    "A document has no `transition` field.",
];

/// Social content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const SOCIAL_RULES: &[&str] = &[
    "A frame is one HTML fragment in `html` and one CSS block in `css`. One frame is a single post. Two or more frames are a carousel, in swipe order.",
    "Design each frame for the px canvas of the social's `format`: 1080 by 1080 px for `square`, 1080 by 1350 px for `portrait` (the default), 1080 by 1920 px for `story`, 1200 by 630 px for `landscape`. A social has no `viewport` field. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The frame root is position: relative, the format size, overflow: hidden. Do not add an outer box of your own with a fixed height and overflow: hidden. Such a box hides overflow from the fit.",
    SHARED_RULES[0],
    SHARED_RULES[1],
    SHARED_RULES[2],
    SHARED_RULES[3],
    SHARED_RULES[4],
    "Give every id and @keyframes name a prefix unique to the frame, such as `f3-`.",
    "Font sizes for a phone screen: titles 72 to 120px, body 36 to 48px, captions 28 to 32px. Keep all text inside the frame with margins of at least 96px. Give boxes enough height for every line. Put at most 25 words on one frame. A frame is read on a phone in two seconds.",
    SHARED_RULES[5],
    "The first frame is the hook: one claim in large type that stops the scroll. The last frame of a carousel is the call to action. Put the caption to post with the frame, hashtags included, in `notes`. The renderer does not show notes on the frame.",
    "Keep the branding the same on every frame: the same fonts, the same palette, the same corner mark or logo position, so the carousel reads as one post.",
    "A frame is a picture in a feed, not a page that is clicked. Do not write links between frames, `<details>`, `<input>`, or any control. Write a web address as plain text.",
    "A social has no `transition` field.",
];

/// Print content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const PRINT_RULES: &[&str] = &[
    "A sheet is one HTML fragment in `html` and one CSS block in `css`. One sheet is a poster. Two sheets are the front and the back of a flyer, in reading order.",
    "Design each sheet for the px canvas of the print's `size`: 559 by 794 px for `a5`, 794 by 1123 px for `a4` (the default), 1123 by 1587 px for `a3`, 816 by 1056 px for `letter`, 1056 by 1632 px for `tabloid`. `orientation: 'landscape'` swaps the width and the height. A print has no `viewport` field. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The sheet root is position: relative, the canvas size, overflow: hidden. Do not add an outer box of your own with a fixed height and overflow: hidden. Such a box hides overflow from the fit.",
    SHARED_RULES[0],
    SHARED_RULES[1],
    SHARED_RULES[2],
    SHARED_RULES[3],
    SHARED_RULES[4],
    "Give every id and @keyframes name a prefix unique to the sheet, such as `s3-`.",
    "Size type for the print kind. A poster is read at a distance: titles 90 to 160px, body 30 to 44px, and at most 40 words on the sheet. A flyer, a menu, a program, or a certificate is read in the hand: titles 40 to 64px, headings 24 to 32px, body 16 to 20px, captions 12 to 14px. Give boxes enough height for every line.",
    "Keep text and logos inside a safe margin of at least 5 percent of the short edge on every side: a print shop trims to the edge. Backgrounds and full-bleed imagery may run to the edge.",
    SHARED_RULES[5],
    "The first sheet is the face of the piece and must work alone. On a two-sheet flyer, the front carries the hook and the back carries the detail. Put print instructions, such as the paper stock or the bleed, in `notes`. The renderer does not show notes on the sheet.",
    "Pick colors that survive white paper in daylight. Do not rely on glow effects or a screen-dark background unless the user asks for one.",
    "A print is ink on paper, not a page that is clicked. Do not write links between sheets, `<details>`, `<input>`, or any control. Write a web address as plain text.",
    "A print has no `transition` field.",
];

/// Mailing content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const MAILING_RULES: &[&str] = &[
    "An email is one HTML fragment in `html` and one CSS block in `css`. One email is a single send. Two or more emails are a sequence, in send order.",
    "Design each email for the px canvas of the mailing's `format`: 600 by 800 px for `short`, 600 by 1200 px for `standard` (the default), 600 by 1800 px for `long`. A mailing has no `viewport` field. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out in one column by default. An email client is 600 px wide on a desktop and narrower on a phone. Stack the blocks and space them with padding and margin. Do not use flex, grid, gap, float, or position: absolute: email clients ignore them. border-radius and box-shadow are safe: a client that ignores them shows a plain box. The email root is position: relative, height: 100%, overflow: hidden. Do not set a px height on the root and do not add an outer box of your own with a fixed height and overflow: hidden. Such a box hides overflow from the fit, and the email export lets the email flow taller than the canvas.",
    "To place blocks side by side, write this exact pattern: a `<div class='columns'>` parent with two or three `<div class='column'>` children, no whitespace between the tags. Give `.columns` an explicit px width. Give each `.column` display: inline-block, vertical-align: top, and an explicit px width; the column widths sum to the `.columns` width. The email export compiles this pattern to table cells for Outlook and stacks the columns on a phone. Do not nest one `columns` div inside another.",
    "Write simple selectors: `tag`, `.class`, `tag.class`, descendant, and child. The email export inlines them into style attributes. A `:hover` rule, a pseudo-element rule, or a `@media` rule stays in a style block and reaches only some clients.",
    SHARED_RULES[0],
    SHARED_RULES[1],
    "The server loads the theme fonts from Google Fonts. Base styles: headings use the heading font with margin 0, paragraphs and lists have margin 0, images are block and max-width 100%. The studio default text size is 32px and the email export default is 16px: set an explicit px font-size on every element that shows text.",
    SHARED_RULES[3],
    SHARED_RULES[4],
    "Give every id a prefix unique to the email, such as `e3-`. Do not write `@keyframes`, `animation`, or `transition` rules: an email client does not run them and the email export drops them.",
    "Size type for the inbox: titles 28 to 40px, headings 22 to 26px, body 16 to 18px, captions 12 to 14px. Set the size in px on the element's own rule, not only on a parent. Give boxes enough height for every line.",
    "Write a call to action as a styled `<a>` link: a solid accent background, padding of at least 14px by 28px, and rounded corners.",
    SHARED_RULES[5],
    "The first email is the one the reader opens first. Lead with the hook: one claim and one call to action near the top. Put the subject line and the preheader text in `notes`, as a `Subject:` line and a `Preheader:` line. The renderer does not show notes on the email.",
    "Keep the branding the same on every email: the same fonts, the same palette, the same header, and the same footer, so the sequence reads as one campaign.",
    "End every email with a footer in muted small type: the sender name, a postal address line, and an unsubscribe line as a plain `<a>` link.",
    "An email is read in an inbox, not clicked between. Do not write links between emails, `<details>`, `<input>`, or any control. Write a web address that is not a call to action as plain text.",
    "A mailing has no `transition` field.",
];

/// Chart rules for data screens, slides, pages, frames, sheets, and
/// emails. Simplified Technical English.
pub const CHART_RULES: &[&str] = &[
    "Draw a chart as inline SVG in the screen, slide, page, or frame html.",
    "Do not load a chart library.",
    "Do not write a `<script>` element.",
    "Give the SVG a `viewBox` attribute. Do not set a fixed pixel width on the SVG.",
    "Take every chart color from the theme palette: `var(--accent)`, `var(--text)`, `var(--muted)`, and `var(--background)`.",
    "Label each axis.",
    "Write each value in the SVG as text. Do not compute a value at run time.",
    "Give the SVG a `role='img'` attribute and a `<title>` element. The `<title>` states what the chart shows.",
];

/// One bar chart that follows `CHART_RULES`: a `viewBox`, theme colors,
/// axis labels, written values, and a `<title>` element.
pub const CHART_EXAMPLE: &str = "<svg class='s4-chart' viewBox='0 0 800 500' role='img'>\n\
<title>Revenue by quarter, in millions</title>\n\
<line x1='80' y1='420' x2='760' y2='420' stroke='var(--muted)' stroke-width='2'/>\n\
<line x1='80' y1='40' x2='80' y2='420' stroke='var(--muted)' stroke-width='2'/>\n\
<rect x='140' y='300' width='100' height='120' fill='var(--accent)'/>\n\
<rect x='300' y='220' width='100' height='200' fill='var(--accent)'/>\n\
<rect x='460' y='140' width='100' height='280' fill='var(--accent)'/>\n\
<text x='190' y='290' text-anchor='middle' fill='var(--text)' font-size='24'>12</text>\n\
<text x='350' y='210' text-anchor='middle' fill='var(--text)' font-size='24'>20</text>\n\
<text x='510' y='130' text-anchor='middle' fill='var(--text)' font-size='24'>28</text>\n\
<text x='190' y='460' text-anchor='middle' fill='var(--muted)' font-size='24'>Q1</text>\n\
<text x='350' y='460' text-anchor='middle' fill='var(--muted)' font-size='24'>Q2</text>\n\
<text x='510' y='460' text-anchor='middle' fill='var(--muted)' font-size='24'>Q3</text>\n\
<text x='420' y='495' text-anchor='middle' fill='var(--text)' font-size='24'>Quarter</text>\n\
<text x='30' y='230' text-anchor='middle' fill='var(--text)' font-size='24' transform='rotate(-90 30 230)'>Millions</text>\n\
</svg>";

/// The `/instructions` and `/schemas` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/instructions", get(get_instructions))
        .route("/schemas/design", get(get_design_schema))
        .route("/schemas/deck", get(get_deck_schema))
        .route("/schemas/document", get(get_document_schema))
        .route("/schemas/social", get(get_social_schema))
        .route("/schemas/print", get(get_print_schema))
        .route("/schemas/mailing", get(get_mailing_schema))
        .route("/schemas/question-set", get(get_question_set_schema))
}

/// Returns the document JSON Schema, generated from the document types.
async fn get_document_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Document))
}

/// Returns the social JSON Schema, generated from the social types.
async fn get_social_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Social))
}

/// Returns the print JSON Schema, generated from the print types.
async fn get_print_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Print))
}

/// Returns the mailing JSON Schema, generated from the mailing types.
async fn get_mailing_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Mailing))
}

/// Returns the design JSON Schema, generated from the design types.
async fn get_design_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Design))
}

/// Returns the deck JSON Schema, generated from the deck types.
async fn get_deck_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Deck))
}

/// Returns the question set JSON Schema, generated from the question
/// types.
async fn get_question_set_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(design_model::BriefQuestionSet))
}

/// Returns the build procedure for agents as JSON.
async fn get_instructions() -> Json<serde_json::Value> {
    Json(instructions())
}

/// The build steps, in order. One instruction per sentence.
fn steps() -> Vec<String> {
    vec![
        "GET /schemas/design, GET /schemas/deck, GET /schemas/document, GET /schemas/social, GET /schemas/print, GET /schemas/mailing, and GET /schemas/question-set. Read the JSON Schemas.".to_owned(),
        "GET /sessions/{id}. Read the request, the artifact_kind, the options, the state, the messages, the question sets, and the answers. The artifact_kind is `demo`, `deck`, `document`, `social`, `print`, or `mailing`. A demo session writes designs. A deck session writes decks. A document session writes documents. A social session writes socials. A print session writes prints. A mailing session writes mailings. There is no brief: the request and the answers are the input.".to_owned(),
        format!("Plan the turn. When you need a choice from the user, PUT /sessions/{{id}}/question-set with a BriefQuestionSet: at most {limit} questions, each with 2 to 4 short options and allow_other true, none required, can_proceed_with_assumptions true. Use single_select when the options rule each other out. Use multi_select when the user can pick more than one at once. The app adds a skip choice. Write every option from the request and the source files, in their words. An option that would fit any other project is wrong. Name the real thing the option builds. The session moves to clarifying. Then stop. After {answered} answered questions about one request, do not ask more about it. A later request for a change starts fresh: ask when the change is unclear.", limit = QUESTIONS_PER_TURN_LIMIT, answered = crate::planner::ANSWERED_QUESTION_LIMIT),
        "The app asks the user for these, not you: the artifact kind, how the colors read, the number of variations; for a demo how much to build, what kind of product it is, what state the screens show, and the canvases; for a deck the audience, the tone, the scenario, the number of slides, how much goes on a slide, how much it leans on data, and how different the candidates are; for a document the audience, the tone, how much it leans on data, what kind of document it is, the paper, how much goes on a page, the number of pages, and how different the candidates are; for a social the audience, the tone, how much it leans on data, the platform, the format, what the post is for, the number of frames, and how different the candidates are; for a print the audience, the tone, how much it leans on data, what kind of print piece it is, the paper size, the orientation, the number of sheets, and how different the candidates are; for a mailing the audience, the tone, how much it leans on data, what kind of email it is, the email format, the number of emails, and how different the candidates are. Never ask about them, and never ask about them in other words. Ask only what this request raises and that list does not cover, such as the features to show or the data on a screen. Ask nothing when the request and the source files already say enough. Read them from the session options and follow them. A demo with several platforms wants one design per canvas: write one file per canvas, each with that canvas in `viewport`.".to_owned(),
        "When you know enough, write. First POST /sessions/{id}/generate: the session moves to generating and accepts artifact writes; in any other state the server answers 409. Demo session: write the design as JSON that conforms to GET /schemas/design. PUT it to /designs/{id}-candidate-1. Use the session id as the base. Number a later run after the candidates the session has: with candidates 1 to 3 present, write candidate 4. The browser shows the candidates.".to_owned(),
        "Deck session: write the deck as JSON that conforms to GET /schemas/deck. The deck has `slides`, not `screens`, and no `viewport`. PUT it to /decks/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.".to_owned(),
        "Document session: write the document as JSON that conforms to GET /schemas/document. The document has `pages` and `paper`, and no `viewport`. Set `paper` to the paper in the session options: `a4` or `letter`. PUT it to /documents/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.".to_owned(),
        "Social session: write the social as JSON that conforms to GET /schemas/social. The social has `frames` and `format`, and no `viewport`. Set `format` to the format in the session options: `square`, `portrait`, `story`, or `landscape`. PUT it to /socials/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.".to_owned(),
        "Print session: write the print as JSON that conforms to GET /schemas/print. The print has `sheets`, `size`, and `orientation`, and no `viewport`. Set `size` and `orientation` to the values in the session options. PUT it to /prints/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.".to_owned(),
        "Mailing session: write the mailing as JSON that conforms to GET /schemas/mailing. The mailing has `emails` and `format`, and no `viewport`. Set `format` to the format in the session options: `short`, `standard`, or `long`. PUT it to /mailings/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.".to_owned(),
        "A 422 response lists every problem in error.details. Fix each one. PUT again.".to_owned(),
        "When the user asks for a change in the chat while an artifact is open, edit that artifact and PUT it again. When no artifact is open, write new candidates. A message that pins two or more candidates and asks to combine parts of them is a merge: write one new candidate under the next free number, and take each part from the candidate the user names for it. A message with is_regenerate true names units like `[screen 2]`, `[slide 2]`, `[page 2]`, `[frame 2]`, `[sheet 2]`, or `[email 2]`: write those units anew, without their old markup, and keep the rest. POST /designs/{id}/fork, POST /decks/{id}/fork, POST /documents/{id}/fork, POST /socials/{id}/fork, POST /prints/{id}/fork, or POST /mailings/{id}/fork copies a candidate to the next free number; the user presses Fork for that, you do not.".to_owned(),
        "GET /uploads?session={id} lists the source files of that session. Each row has name, size_bytes, content_type, and is_image. GET /uploads/{name} returns the file. Use an image row as `<img src='/uploads/{name}'>`. A file belongs to one session: never read another session's files.".to_owned(),
        "After you save a design, look at it: GET /designs/{id}/screens/{n}.png returns a PNG of screen n, 1-based. It needs Chrome or Chromium and answers 503 without one. Review every screen for overlap, overflow, empty space, and weak contrast. Fix what you see and PUT the design again. GET /designs/{id}/export returns the design as one HTML file. A design has no PDF export.".to_owned(),
        "After you save a deck, look at it: GET /decks/{id}/slides/{n}.png returns a PNG of slide n, 1-based. Review every slide the same way and PUT the deck again. GET /decks/{id}/export returns the deck as one HTML file. GET /decks/{id}/export.pdf returns it as a PDF, one page per slide. GET /decks/{id}/export.pptx returns it as a PowerPoint file. GET /decks/{id}/present is the presenter view with the notes.".to_owned(),
        "After you save a document, look at it: GET /documents/{id}/pages/{n}.png returns a PNG of page n, 1-based. Review every page the same way and PUT the document again. GET /documents/{id}/export returns the document as one HTML file. GET /documents/{id}/export.pdf returns it as a PDF, one sheet per page. GET /documents/{id}/export.docx returns it as a Word file.".to_owned(),
        "After you save a social, look at it: GET /socials/{id}/frames/{n}.png returns a PNG of frame n, 1-based. Review every frame the same way and PUT the social again. GET /socials/{id}/export returns the social as one HTML file. GET /socials/{id}/export.pdf returns it as a PDF, one sheet per frame, the file a LinkedIn carousel takes. GET /socials/{id}/export.zip returns one PNG per frame in a zip, the files an Instagram carousel takes.".to_owned(),
        "After you save a print, look at it: GET /prints/{id}/sheets/{n}.png returns a PNG of sheet n, 1-based. Review every sheet the same way and PUT the print again. GET /prints/{id}/export returns the print as one HTML file. GET /prints/{id}/export.pdf returns it as a PDF, one PDF page per sheet, the file a print shop takes. GET /prints/{id}/export.zip returns one PNG per sheet in a zip.".to_owned(),
        "After you save a mailing, look at it: GET /mailings/{id}/emails/{n}.png returns a PNG of email n, 1-based. Review every email the same way and PUT the mailing again. GET /mailings/{id}/export returns the mailing as one HTML file. GET /mailings/{id}/export.pdf returns it as a PDF, one PDF page per email. GET /mailings/{id}/export.zip returns one PNG per email in a zip. GET /mailings/{id}/export.email.zip returns one email-client HTML file per email in a zip: inlined styles, a 600 px table shell, and Outlook support. Send that file from your email service.".to_owned(),
        "When the design, the deck, the document, the social, the print, or the mailing is written, POST /sessions/{id}/complete, or exit with code 0. The session moves to reviewing.".to_owned(),
        "GET /events returns {\"revision\": n}. The revision increases when data changes. To wait for a change in one run, call GET /events?after={revision}&wait=25 in a loop. Each call returns within the wait time, so loop; do not treat a timeout as an error.".to_owned(),
    ]
}

/// The route table of the instructions payload, as (name, route)
/// pairs. Kept apart from the payload: the `json!` macro cannot nest
/// this many keys in one call.
fn routes_map() -> serde_json::Value {
    let routes = [
        ("schema_design", "GET /schemas/design"),
        ("schema_deck", "GET /schemas/deck"),
        ("schema_document", "GET /schemas/document"),
        ("schema_social", "GET /schemas/social"),
        ("schema_print", "GET /schemas/print"),
        ("schema_mailing", "GET /schemas/mailing"),
        ("schema_question_set", "GET /schemas/question-set"),
        ("session", "GET /sessions/{id}"),
        ("question_set", "PUT /sessions/{id}/question-set"),
        ("answers", "GET /sessions/{id} (answers field)"),
        ("complete", "POST /sessions/{id}/complete"),
        ("events", "GET /events?after={revision}&wait={seconds}"),
        ("uploads", "GET /uploads?session={id}"),
        ("upload", "GET /uploads/{name}"),
        ("save_design", "PUT /designs/{id}"),
        ("check_design", "POST /designs/render"),
        ("render_design", "GET /designs/{id}/render"),
        ("screen_image", "GET /designs/{id}/screens/{n}.png"),
        ("export_html", "GET /designs/{id}/export"),
        ("save_deck", "PUT /decks/{id}"),
        ("check_deck", "POST /decks/render"),
        ("render_deck", "GET /decks/{id}/render"),
        ("render_audience", "GET /decks/{id}/render?audience=true"),
        ("slide_image", "GET /decks/{id}/slides/{n}.png"),
        ("present", "GET /decks/{id}/present"),
        ("export_deck_html", "GET /decks/{id}/export"),
        ("export_deck_pdf", "GET /decks/{id}/export.pdf"),
        ("export_pptx", "GET /decks/{id}/export.pptx"),
        ("save_document", "PUT /documents/{id}"),
        ("check_document", "POST /documents/render"),
        ("render_document", "GET /documents/{id}/render"),
        ("page_image", "GET /documents/{id}/pages/{n}.png"),
        ("export_document_html", "GET /documents/{id}/export"),
        ("export_document_pdf", "GET /documents/{id}/export.pdf"),
        ("export_docx", "GET /documents/{id}/export.docx"),
        ("save_social", "PUT /socials/{id}"),
        ("check_social", "POST /socials/render"),
        ("render_social", "GET /socials/{id}/render"),
        ("frame_image", "GET /socials/{id}/frames/{n}.png"),
        ("export_social_html", "GET /socials/{id}/export"),
        ("export_social_pdf", "GET /socials/{id}/export.pdf"),
        ("export_social_zip", "GET /socials/{id}/export.zip"),
        ("save_print", "PUT /prints/{id}"),
        ("check_print", "POST /prints/render"),
        ("render_print", "GET /prints/{id}/render"),
        ("sheet_image", "GET /prints/{id}/sheets/{n}.png"),
        ("export_print_html", "GET /prints/{id}/export"),
        ("export_print_pdf", "GET /prints/{id}/export.pdf"),
        ("export_print_zip", "GET /prints/{id}/export.zip"),
        ("save_mailing", "PUT /mailings/{id}"),
        ("check_mailing", "POST /mailings/render"),
        ("render_mailing", "GET /mailings/{id}/render"),
        ("email_image", "GET /mailings/{id}/emails/{n}.png"),
        ("email_client_html", "GET /mailings/{id}/emails/{n}.html"),
        ("export_mailing_html", "GET /mailings/{id}/export"),
        ("export_mailing_pdf", "GET /mailings/{id}/export.pdf"),
        ("export_mailing_zip", "GET /mailings/{id}/export.zip"),
        (
            "export_mailing_email_zip",
            "GET /mailings/{id}/export.email.zip",
        ),
        ("fork_design", "POST /designs/{id}/fork"),
        ("fork_deck", "POST /decks/{id}/fork"),
        ("fork_document", "POST /documents/{id}/fork"),
        ("fork_social", "POST /socials/{id}/fork"),
        ("fork_print", "POST /prints/{id}/fork"),
        ("fork_mailing", "POST /mailings/{id}/fork"),
        ("chooser", "GET /candidates/{base}"),
        ("templates", "GET /templates"),
        ("template", "GET /templates/{id}"),
    ];
    routes
        .into_iter()
        .map(|(name, route)| (name.to_owned(), serde_json::Value::String(route.to_owned())))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into()
}

/// The instructions payload. Kept in one function so tests can check it
/// without a request.
fn instructions() -> serde_json::Value {
    let example_design: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json"))
            .unwrap_or_default();
    let example_deck: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json"))
            .unwrap_or_default();
    let example_document: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-document.json"))
            .unwrap_or_default();
    let example_social: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-social.json"))
            .unwrap_or_default();
    let example_print: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-print.json"))
            .unwrap_or_default();
    let example_mailing: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-mailing.json"))
            .unwrap_or_default();
    serde_json::json!({
        "purpose": "Turn a request into HTML designs, decks, documents, social posts, print pieces, or emails: ask a few questions in the chat, write candidates, then edit them from the chat. Swift Design keeps the workflow state, validates, renders, and lets the user edit. Swift Design makes no LLM API calls.",
        "session": "The run works on one session. Its id is in the SWIFT_DESIGN_SESSION_ID environment variable. The mode is in SWIFT_DESIGN_RUN_MODE: generation. The artifact kind is in SWIFT_DESIGN_ARTIFACT_KIND: demo, deck, document, social, print, or mailing. GET /sessions/{id} returns the state, the artifact_kind, the options, the question sets, the answers, and the messages.",
        "kinds": {
            "demo": "A software demo: a landing page, app screens, or a similar layout on a device viewport. Written as a design with `screens` and a `viewport`. Saved under /designs.",
            "deck": "A slide presentation on a 1920 by 1080 px canvas. Written as a deck with `slides` and no `viewport`. Saved under /decks.",
            "document": "A paged document on A4 or Letter paper: a report, a memo, a proposal, a letter, or a guide. Written as a document with `pages` and `paper`, and no `viewport`. Saved under /documents.",
            "social": "A social post or a carousel on a square, portrait, story, or landscape canvas, for Instagram, LinkedIn, X, or Facebook. Written as a social with `frames` and `format`, and no `viewport`. Saved under /socials.",
            "print": "A print piece on an A5, A4, A3, Letter, or Tabloid canvas, portrait or landscape: a poster, a flyer, a menu, a program, a certificate, or a sign. Written as a print with `sheets`, `size`, and `orientation`, and no `viewport`. Saved under /prints.",
            "mailing": "An email or an email sequence on a 600 px wide canvas: a newsletter, an announcement, a promotion, a welcome email, a digest, or an invitation. Written as a mailing with `emails` and `format`, and no `viewport`. Saved under /mailings.",
        },
        "steps": steps(),
        "demo_rules": DEMO_RULES,
        "deck_rules": DECK_RULES,
        "document_rules": DOCUMENT_RULES,
        "social_rules": SOCIAL_RULES,
        "print_rules": PRINT_RULES,
        "mailing_rules": MAILING_RULES,
        "charts": {
            "rules": CHART_RULES,
            "example": CHART_EXAMPLE,
        },
        "conventions": {
            "canvas": "a design's `viewport` in px (default 1440 by 900); a deck's fixed 1920 by 1080; a document's `paper`: 794 by 1123 for a4, 816 by 1056 for letter; a social's `format`: 1080 by 1080 for square, 1080 by 1350 for portrait, 1080 by 1920 for story, 1200 by 630 for landscape; a print's `size`: 559 by 794 for a5, 794 by 1123 for a4, 1123 by 1587 for a3, 816 by 1056 for letter, 1056 by 1632 for tabloid, with `orientation: 'landscape'` swapping width and height; a mailing's `format`: 600 by 800 for short, 600 by 1200 for standard, 600 by 1800 for long. Use px units. The server scales the canvas to any frame.",
            "css_variables": ["--background", "--text", "--accent", "--muted", "--heading-font", "--body-font", "--mono-font"],
            "base_styles": "32px body text in the body font and text color; headings in the heading font with margin 0; paragraphs and lists margin 0; images block and max-width 100%",
            "node_reference": "[screen N, node a/b/c <tag.class>: text] for a design; [slide N, node a/b/c <tag.class>: text] for a deck; [page N, node a/b/c <tag.class>: text] for a document; [frame N, node a/b/c <tag.class>: text] for a social; [sheet N, node a/b/c <tag.class>: text] for a print; [email N, node a/b/c <tag.class>: text] for a mailing",
            "upload_reference": "[upload name]",
            "comment_lines": "A message may carry several comments, one per line, each `<node or page reference>: <note>`. Apply every line in one edit.",
        },
        "payloads": {
            "PUT /sessions/{id}/question-set": "a BriefQuestionSet",
            "PUT /designs/{id}": "the design JSON",
            "POST /designs/render": "the design JSON; returns the rendered HTML, or every validation error",
            "PUT /decks/{id}": "the deck JSON",
            "POST /decks/render": "the deck JSON; returns the rendered HTML, or every validation error",
            "PUT /documents/{id}": "the document JSON",
            "POST /documents/render": "the document JSON; returns the rendered HTML, or every validation error",
            "PUT /socials/{id}": "the social JSON",
            "POST /socials/render": "the social JSON; returns the rendered HTML, or every validation error",
            "PUT /prints/{id}": "the print JSON",
            "POST /prints/render": "the print JSON; returns the rendered HTML, or every validation error",
            "PUT /mailings/{id}": "the mailing JSON",
            "POST /mailings/render": "the mailing JSON; returns the rendered HTML, or every validation error",
        },
        "routes": routes_map(),
        "example_design": example_design,
        "example_deck": example_deck,
        "example_document": example_document,
        "example_social": example_social,
        "example_print": example_print,
        "example_mailing": example_mailing,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Deck, Design, Document, Mailing, Print, Social};

    use crate::instructions::{
        CHART_EXAMPLE, DECK_RULES, DEMO_RULES, DOCUMENT_RULES, MAILING_RULES, PRINT_RULES,
        SOCIAL_RULES, instructions,
    };

    #[test]
    fn instructions_carry_social_rules_routes_and_the_example() {
        let payload = instructions();
        assert_eq!(
            payload["social_rules"].as_array().unwrap().len(),
            SOCIAL_RULES.len()
        );
        let social = payload["social_rules"].to_string();
        assert!(social.contains("1080 by 1350 px"));
        assert!(social.contains("`format`"));
        assert!(social.contains("titles 72 to 120px"));
        assert!(social.contains("The first frame is the hook"));
        assert!(social.contains("Do not write links between frames"));
        assert!(!social.contains("#screen-"));
        assert!(!social.contains("/decks/"));
        assert_eq!(payload["routes"]["schema_social"], "GET /schemas/social");
        assert_eq!(payload["routes"]["save_social"], "PUT /socials/{id}");
        assert_eq!(
            payload["routes"]["frame_image"],
            "GET /socials/{id}/frames/{n}.png"
        );
        assert_eq!(
            payload["routes"]["export_social_zip"],
            "GET /socials/{id}/export.zip"
        );
        assert_eq!(payload["routes"]["fork_social"], "POST /socials/{id}/fork");
        assert!(
            payload["kinds"]["social"]
                .as_str()
                .unwrap()
                .contains("`frames`")
        );
        let text = payload.to_string();
        assert!(text.contains("PUT it to /socials/{id}-candidate-1"));
        assert!(text.contains("demo, deck, document, social, print, or mailing"));
        assert!(text.contains("`[frame 2]`"));
        let example: Social = serde_json::from_value(payload["example_social"].clone()).unwrap();
        assert_eq!(example.validate(), Vec::new());
        assert!(payload["example_social"].get("viewport").is_none());
    }

    #[test]
    fn instructions_carry_print_rules_routes_and_the_example() {
        let payload = instructions();
        assert_eq!(
            payload["print_rules"].as_array().unwrap().len(),
            PRINT_RULES.len()
        );
        let print = payload["print_rules"].to_string();
        assert!(print.contains("794 by 1123 px"));
        assert!(print.contains("`size`"));
        assert!(print.contains("swaps the width and the height"));
        assert!(print.contains("safe margin"));
        assert!(print.contains("The first sheet is the face of the piece"));
        assert!(print.contains("Do not write links between sheets"));
        assert!(!print.contains("#screen-"));
        assert!(!print.contains("/decks/"));
        assert_eq!(payload["routes"]["schema_print"], "GET /schemas/print");
        assert_eq!(payload["routes"]["save_print"], "PUT /prints/{id}");
        assert_eq!(
            payload["routes"]["sheet_image"],
            "GET /prints/{id}/sheets/{n}.png"
        );
        assert_eq!(
            payload["routes"]["export_print_zip"],
            "GET /prints/{id}/export.zip"
        );
        assert_eq!(payload["routes"]["fork_print"], "POST /prints/{id}/fork");
        assert!(
            payload["kinds"]["print"]
                .as_str()
                .unwrap()
                .contains("`sheets`")
        );
        let text = payload.to_string();
        assert!(text.contains("PUT it to /prints/{id}-candidate-1"));
        assert!(text.contains("demo, deck, document, social, print, or mailing"));
        assert!(text.contains("`[sheet 2]`"));
        let example: Print = serde_json::from_value(payload["example_print"].clone()).unwrap();
        assert_eq!(example.validate(), Vec::new());
        assert!(payload["example_print"].get("viewport").is_none());
    }

    #[test]
    fn instructions_carry_mailing_rules_routes_and_the_example() {
        let payload = instructions();
        assert_eq!(
            payload["mailing_rules"].as_array().unwrap().len(),
            MAILING_RULES.len()
        );
        let mailing = payload["mailing_rules"].to_string();
        assert!(mailing.contains("600 by 1200 px"));
        assert!(mailing.contains("`format`"));
        assert!(mailing.contains("one column"));
        assert!(mailing.contains("Do not use flex"));
        assert!(mailing.contains("inline-block"));
        assert!(mailing.contains("Subject:"));
        assert!(mailing.contains("unsubscribe"));
        assert!(mailing.contains("Do not write links between emails"));
        assert!(!mailing.contains("#screen-"));
        assert!(!mailing.contains("/decks/"));
        assert_eq!(payload["routes"]["schema_mailing"], "GET /schemas/mailing");
        assert_eq!(payload["routes"]["save_mailing"], "PUT /mailings/{id}");
        assert_eq!(
            payload["routes"]["email_image"],
            "GET /mailings/{id}/emails/{n}.png"
        );
        assert_eq!(
            payload["routes"]["export_mailing_zip"],
            "GET /mailings/{id}/export.zip"
        );
        assert_eq!(
            payload["routes"]["export_mailing_email_zip"],
            "GET /mailings/{id}/export.email.zip"
        );
        assert_eq!(
            payload["routes"]["email_client_html"],
            "GET /mailings/{id}/emails/{n}.html"
        );
        assert_eq!(
            payload["routes"]["fork_mailing"],
            "POST /mailings/{id}/fork"
        );
        assert!(
            payload["kinds"]["mailing"]
                .as_str()
                .unwrap()
                .contains("`emails`")
        );
        let text = payload.to_string();
        assert!(text.contains("PUT it to /mailings/{id}-candidate-1"));
        assert!(text.contains("demo, deck, document, social, print, or mailing"));
        assert!(text.contains("`[email 2]`"));
        let example: Mailing = serde_json::from_value(payload["example_mailing"].clone()).unwrap();
        assert_eq!(example.validate(), Vec::new());
        assert!(payload["example_mailing"].get("viewport").is_none());
    }

    #[test]
    fn instructions_carry_document_rules_routes_and_the_example() {
        let payload = instructions();
        assert_eq!(
            payload["document_rules"].as_array().unwrap().len(),
            DOCUMENT_RULES.len()
        );
        let document = payload["document_rules"].to_string();
        assert!(document.contains("794 by 1123 px"));
        assert!(document.contains("`paper`"));
        assert!(document.contains("titles 28 to 40px"));
        assert!(document.contains("A document is read, not clicked."));
        assert!(!document.contains("#screen-"));
        assert!(!document.contains("/decks/"));
        assert_eq!(
            payload["routes"]["schema_document"],
            "GET /schemas/document"
        );
        assert_eq!(payload["routes"]["save_document"], "PUT /documents/{id}");
        assert_eq!(
            payload["routes"]["page_image"],
            "GET /documents/{id}/pages/{n}.png"
        );
        assert_eq!(
            payload["routes"]["export_docx"],
            "GET /documents/{id}/export.docx"
        );
        assert_eq!(
            payload["routes"]["fork_document"],
            "POST /documents/{id}/fork"
        );
        assert!(
            payload["kinds"]["document"]
                .as_str()
                .unwrap()
                .contains("`pages`")
        );
        let text = payload.to_string();
        assert!(text.contains("PUT it to /documents/{id}-candidate-1"));
        assert!(text.contains("demo, deck, document, social, print, or mailing"));
        assert!(text.contains("`[page 2]`"));
        let example: Document =
            serde_json::from_value(payload["example_document"].clone()).unwrap();
        assert_eq!(example.validate(), Vec::new());
        assert!(payload["example_document"].get("viewport").is_none());
    }

    #[test]
    fn instructions_cover_steps_rules_routes_and_an_example() {
        let payload = instructions();
        assert!(!payload["steps"].as_array().unwrap().is_empty());
        assert!(!payload["demo_rules"].as_array().unwrap().is_empty());
        assert_eq!(payload["routes"]["schema_design"], "GET /schemas/design");
        assert_eq!(payload["routes"]["session"], "GET /sessions/{id}");
        assert_eq!(payload["example_design"]["title"], "Swift Design Overview");
    }

    #[test]
    fn instructions_name_the_modes_and_the_gating() {
        let text = instructions().to_string();
        assert!(text.contains("Plan the turn"));
        assert!(text.contains("There is no brief"));
        assert!(text.contains("at most 3 questions"));
        assert!(text.contains("When you know enough, write."));
        assert!(!text.contains("briefing mode"));
    }

    #[test]
    fn instructions_serve_every_schema() {
        let payload = instructions();
        assert_eq!(payload["routes"]["schema_deck"], "GET /schemas/deck");
        assert_eq!(
            payload["routes"]["schema_question_set"],
            "GET /schemas/question-set"
        );
        assert_eq!(payload["routes"]["templates"], "GET /templates");
        assert_eq!(payload["routes"]["fork_design"], "POST /designs/{id}/fork");
        assert_eq!(payload["routes"]["fork_deck"], "POST /decks/{id}/fork");
        let text = payload["steps"].to_string();
        assert!(text.contains("is a merge: write one new candidate under the next free number"));
        assert!(text.contains("is_regenerate true"));
        assert!(text.contains("Number a later run after the candidates the session has"));
    }

    #[test]
    fn instructions_carry_rules_for_both_kinds() {
        let payload = instructions();
        assert_eq!(
            payload["demo_rules"].as_array().unwrap().len(),
            DEMO_RULES.len()
        );
        assert_eq!(
            payload["deck_rules"].as_array().unwrap().len(),
            DECK_RULES.len()
        );
        let demo = payload["demo_rules"].to_string();
        let deck = payload["deck_rules"].to_string();
        assert!(demo.contains("`viewport`"));
        assert!(demo.contains("On a 390 px phone canvas"));
        assert!(!demo.contains("titles 80 to 120px"));
        assert!(deck.contains("1920 by 1080 px"));
        assert!(deck.contains("titles 80 to 120px"));
        assert!(deck.contains("/decks/{id}/present"));
        assert!(!deck.contains("`viewport`."));
        assert!(
            payload["kinds"]["deck"]
                .as_str()
                .unwrap()
                .contains("`slides`")
        );
    }

    #[test]
    fn instructions_embed_the_deck_example() {
        let payload = instructions();
        assert_eq!(
            payload["example_deck"]["title"],
            "Swift Design Deck Overview"
        );
        assert!(payload["example_deck"]["slides"].is_array());
        assert!(payload["example_deck"].get("viewport").is_none());
        let deck: Deck = serde_json::from_value(payload["example_deck"].clone()).unwrap();
        assert_eq!(deck.validate(), Vec::new());
    }

    #[test]
    fn instructions_name_the_deck_routes() {
        let payload = instructions();
        assert_eq!(payload["routes"]["save_deck"], "PUT /decks/{id}");
        assert_eq!(payload["routes"]["check_deck"], "POST /decks/render");
        assert_eq!(payload["routes"]["present"], "GET /decks/{id}/present");
        assert_eq!(
            payload["routes"]["render_audience"],
            "GET /decks/{id}/render?audience=true"
        );
        assert_eq!(
            payload["routes"]["slide_image"],
            "GET /decks/{id}/slides/{n}.png"
        );
        let text = payload.to_string();
        assert!(text.contains("PUT it to /decks/{id}-candidate-1"));
        assert!(text.contains("SWIFT_DESIGN_ARTIFACT_KIND"));
    }

    #[test]
    fn the_demo_rules_explain_screen_links_and_css_widgets() {
        let demo = DEMO_RULES.join("\n");
        assert!(demo.contains("`<a href='#screen-3'>`"));
        assert!(demo.contains("`<details>` with a `<summary>`"));
        assert!(demo.contains("`<input type='checkbox'>`"));
        assert!(demo.contains("Do not write `<button>`."));
        assert!(demo.contains("Every control acts."));
        assert!(demo.contains("`static_control`"));
        let deck = DECK_RULES.join("\n");
        assert!(!deck.contains("#screen-"));
        assert!(deck.contains("`<input>` with type `checkbox` or `radio`"));
    }

    #[test]
    fn instructions_describe_charts() {
        let payload = instructions();
        let text = payload.to_string();
        assert!(
            text.contains("Draw a chart as inline SVG in the screen, slide, page, or frame html.")
        );
        assert!(text.contains("Do not load a chart library."));
        assert!(text.contains("Do not write a `<script>` element."));
        assert!(text.contains("Give the SVG a `viewBox` attribute."));
        assert!(text.contains("Take every chart color from the theme palette"));
        assert!(text.contains("Label each axis."));
        assert!(text.contains("Do not compute a value at run time."));
        assert!(text.contains("`role='img'` attribute and a `<title>` element"));
        assert_eq!(payload["charts"]["rules"].as_array().unwrap().len(), 8);
        assert_eq!(payload["charts"]["example"], CHART_EXAMPLE);
    }

    #[test]
    fn the_chart_example_is_short_and_validates_inside_a_design() {
        assert!(CHART_EXAMPLE.lines().count() < 20);
        assert!(CHART_EXAMPLE.contains("viewBox='0 0 800 500'"));
        assert!(CHART_EXAMPLE.contains("<title>"));
        assert!(CHART_EXAMPLE.contains("var(--accent)"));
        assert!(!CHART_EXAMPLE.contains("width='800'"));
        let mut design: Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        design.screens[0].html = format!("<div class='s4-page'>{CHART_EXAMPLE}</div>");
        assert_eq!(design.validate(), Vec::new());
    }

    #[test]
    fn instructions_name_every_export() {
        let payload = instructions();
        assert_eq!(payload["routes"]["export_html"], "GET /designs/{id}/export");
        assert!(payload["routes"].get("export_pdf").is_none());
        assert!(payload.to_string().contains("A design has no PDF export."));
        assert_eq!(
            payload["routes"]["export_pptx"],
            "GET /decks/{id}/export.pptx"
        );
        let text = payload.to_string();
        assert!(text.contains("one page per slide"));
        assert!(text.contains("PowerPoint"));
    }

    #[test]
    fn instructions_describe_upload_rows_and_references() {
        let payload = instructions();
        assert_eq!(payload["routes"]["upload"], "GET /uploads/{name}");
        assert_eq!(payload["conventions"]["upload_reference"], "[upload name]");
        let text = payload.to_string();
        assert!(text.contains("is_image"));
        assert!(text.contains("<img src='/uploads/"));
    }
}
