//! The agent contract, served by the app.
//!
//! Agents get everything over HTTP: `GET /instructions` returns the
//! full build procedure as JSON, and `GET /schemas/{design,deck,brief,
//! question-set}` return the JSON Schemas generated from the Rust types
//! at runtime, so they can never go stale. No repo file is part of the
//! agent interface. Instruction strings follow Simplified Technical
//! English.
//!
//! There are two artifact kinds. A demo session writes designs
//! (screens on a device viewport). A deck session writes decks (slides
//! on a 1920 by 1080 px canvas). Each kind has its own rule list.

use axum::routing::get;
use axum::{Json, Router};
use design_model::{Deck, Design, QUESTIONS_PER_TURN_LIMIT};

/// Rules every screen and every slide follow. Shared by both kinds.
const SHARED_RULES: [&str; 6] = [
    "The server scopes your CSS to the screen or slide. Write plain selectors such as `.title` or `h1`. Do not write `html`, `body`, or `:root` selectors. Do not use `@import`. `@media`, `@keyframes`, and `@font-face` are allowed.",
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

/// Chart rules for data screens and slides. Simplified Technical English.
pub const CHART_RULES: &[&str] = &[
    "Draw a chart as inline SVG in the screen or slide html.",
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
        .route("/schemas/question-set", get(get_question_set_schema))
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
        "GET /schemas/design, GET /schemas/deck, and GET /schemas/question-set. Read the JSON Schemas.".to_owned(),
        "GET /sessions/{id}. Read the request, the artifact_kind, the options, the state, the messages, the question sets, and the answers. The artifact_kind is `demo` or `deck`. A demo session writes designs. A deck session writes decks. There is no brief: the request and the answers are the input.".to_owned(),
        format!("Plan the turn. When you need a choice from the user, PUT /sessions/{{id}}/question-set with a BriefQuestionSet: at most {limit} questions, each with 2 to 4 short options and allow_other true, none required, can_proceed_with_assumptions true. Use single_select when the options rule each other out. Use multi_select when the user can pick more than one at once. The app adds a skip choice. Write every option from the request and the source files, in their words. An option that would fit any other project is wrong. Name the real thing the option builds. The session moves to clarifying. Then stop. After {answered} answered questions about one request, do not ask more about it. A later request for a change starts fresh: ask when the change is unclear.", limit = QUESTIONS_PER_TURN_LIMIT, answered = crate::planner::ANSWERED_QUESTION_LIMIT),
        "The app asks the user for these, not you: the artifact kind, how the colors read, the number of variations; for a demo how much to build, what kind of product it is, what state the screens show, and the canvases; for a deck the audience, the tone, the scenario, the number of slides, how much goes on a slide, how much it leans on data, and how different the candidates are. Never ask about them, and never ask about them in other words. Ask only what this request raises and that list does not cover, such as the features to show or the data on a screen. Ask nothing when the request and the source files already say enough. Read them from the session options and follow them. A demo with several platforms wants one design per canvas: write one file per canvas, each with that canvas in `viewport`.".to_owned(),
        "When you know enough, write. First POST /sessions/{id}/generate: the session moves to generating and accepts artifact writes; in any other state the server answers 409. Demo session: write the design as JSON that conforms to GET /schemas/design. PUT it to /designs/{id}-candidate-1. Use the session id as the base. Number a later run after the candidates the session has: with candidates 1 to 3 present, write candidate 4. The browser shows the candidates.".to_owned(),
        "Deck session: write the deck as JSON that conforms to GET /schemas/deck. The deck has `slides`, not `screens`, and no `viewport`. PUT it to /decks/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.".to_owned(),
        "A 422 response lists every problem in error.details. Fix each one. PUT again.".to_owned(),
        "When the user asks for a change in the chat while an artifact is open, edit that artifact and PUT it again. When no artifact is open, write new candidates. A message that pins two or more candidates and asks to combine parts of them is a merge: write one new candidate under the next free number, and take each part from the candidate the user names for it. A message with is_regenerate true names units like `[screen 2]` or `[slide 2]`: write those units anew, without their old markup, and keep the rest. POST /designs/{id}/fork or POST /decks/{id}/fork copies a candidate to the next free number; the user presses Fork for that, you do not.".to_owned(),
        "GET /uploads?session={id} lists the source files of that session. Each row has name, size_bytes, content_type, and is_image. GET /uploads/{name} returns the file. Use an image row as `<img src='/uploads/{name}'>`. A file belongs to one session: never read another session's files.".to_owned(),
        "After you save a design, look at it: GET /designs/{id}/screens/{n}.png returns a PNG of screen n, 1-based. It needs Chrome or Chromium and answers 503 without one. Review every screen for overlap, overflow, empty space, and weak contrast. Fix what you see and PUT the design again. GET /designs/{id}/export returns the design as one HTML file. A design has no PDF export.".to_owned(),
        "After you save a deck, look at it: GET /decks/{id}/slides/{n}.png returns a PNG of slide n, 1-based. Review every slide the same way and PUT the deck again. GET /decks/{id}/export returns the deck as one HTML file. GET /decks/{id}/export.pdf returns it as a PDF, one page per slide. GET /decks/{id}/export.pptx returns it as a PowerPoint file. GET /decks/{id}/present is the presenter view with the notes.".to_owned(),
        "When the design or the deck is written, POST /sessions/{id}/complete, or exit with code 0. The session moves to reviewing.".to_owned(),
        "GET /events returns {\"revision\": n}. The revision increases when data changes. To wait for a change in one run, call GET /events?after={revision}&wait=25 in a loop. Each call returns within the wait time, so loop; do not treat a timeout as an error.".to_owned(),
    ]
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
    serde_json::json!({
        "purpose": "Turn a request into HTML designs or decks: ask a few questions in the chat, write candidates, then edit them from the chat. Swift Design keeps the workflow state, validates, renders, and lets the user edit. Swift Design makes no LLM API calls.",
        "session": "The run works on one session. Its id is in the SWIFT_DESIGN_SESSION_ID environment variable. The mode is in SWIFT_DESIGN_RUN_MODE: generation. The artifact kind is in SWIFT_DESIGN_ARTIFACT_KIND: demo or deck. GET /sessions/{id} returns the state, the artifact_kind, the options, the question sets, the answers, and the messages.",
        "kinds": {
            "demo": "A software demo: a landing page, app screens, or a similar layout on a device viewport. Written as a design with `screens` and a `viewport`. Saved under /designs.",
            "deck": "A slide presentation on a 1920 by 1080 px canvas. Written as a deck with `slides` and no `viewport`. Saved under /decks.",
        },
        "steps": steps(),
        "demo_rules": DEMO_RULES,
        "deck_rules": DECK_RULES,
        "charts": {
            "rules": CHART_RULES,
            "example": CHART_EXAMPLE,
        },
        "conventions": {
            "canvas": "a design's `viewport` in px (default 1440 by 900); a deck's fixed 1920 by 1080. Use px units. The server scales the canvas to any frame.",
            "css_variables": ["--background", "--text", "--accent", "--muted", "--heading-font", "--body-font", "--mono-font"],
            "base_styles": "32px body text in the body font and text color; headings in the heading font with margin 0; paragraphs and lists margin 0; images block and max-width 100%",
            "node_reference": "[screen N, node a/b/c <tag.class>: text] for a design; [slide N, node a/b/c <tag.class>: text] for a deck",
            "upload_reference": "[upload name]",
        },
        "payloads": {
            "PUT /sessions/{id}/question-set": "a BriefQuestionSet",
            "PUT /designs/{id}": "the design JSON",
            "POST /designs/render": "the design JSON; returns the rendered HTML, or every validation error",
            "PUT /decks/{id}": "the deck JSON",
            "POST /decks/render": "the deck JSON; returns the rendered HTML, or every validation error",
        },
        "routes": {
            "schema_design": "GET /schemas/design",
            "schema_deck": "GET /schemas/deck",
            "schema_question_set": "GET /schemas/question-set",
            "session": "GET /sessions/{id}",
            "question_set": "PUT /sessions/{id}/question-set",
            "answers": "GET /sessions/{id} (answers field)",
            "complete": "POST /sessions/{id}/complete",
            "events": "GET /events?after={revision}&wait={seconds}",
            "uploads": "GET /uploads?session={id}",
            "upload": "GET /uploads/{name}",
            "save_design": "PUT /designs/{id}",
            "check_design": "POST /designs/render",
            "render_design": "GET /designs/{id}/render",
            "screen_image": "GET /designs/{id}/screens/{n}.png",
            "export_html": "GET /designs/{id}/export",
            "save_deck": "PUT /decks/{id}",
            "check_deck": "POST /decks/render",
            "render_deck": "GET /decks/{id}/render",
            "render_audience": "GET /decks/{id}/render?audience=true",
            "slide_image": "GET /decks/{id}/slides/{n}.png",
            "present": "GET /decks/{id}/present",
            "export_deck_html": "GET /decks/{id}/export",
            "export_deck_pdf": "GET /decks/{id}/export.pdf",
            "export_pptx": "GET /decks/{id}/export.pptx",
            "fork_design": "POST /designs/{id}/fork",
            "fork_deck": "POST /decks/{id}/fork",
            "chooser": "GET /candidates/{base}",
            "templates": "GET /templates",
            "template": "GET /templates/{id}",
        },
        "example_design": example_design,
        "example_deck": example_deck,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Deck, Design};

    use crate::instructions::{CHART_EXAMPLE, DECK_RULES, DEMO_RULES, instructions};

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
        assert!(text.contains("Draw a chart as inline SVG in the screen or slide html."));
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
