//! The agent contract, served by the app.
//!
//! Agents get everything over HTTP: `GET /instructions` returns the
//! full build procedure as JSON, and `GET /schemas/design` returns the
//! design JSON Schema generated from the Rust types at runtime, so it
//! can never go stale. No repo file is part of the agent interface.
//! Instruction strings follow Simplified Technical English.

use axum::routing::get;
use axum::{Json, Router};
use design_model::{Design, QUESTIONS_PER_TURN_LIMIT};

/// Design content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const CONTENT_RULES: &[&str] = &[
    "A screen is one HTML fragment in `html` and one CSS block in `css`.",
    "Design each screen for the px canvas in the design's `viewport`. The default is 1440 by 900 px (desktop web). Use 390 by 844 for a phone and 1024 by 768 for a tablet. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The screen root is position: relative, the viewport size, overflow: hidden.",
    "The server scopes your CSS to the screen. Write plain selectors such as `.title` or `h1`. Do not write `html`, `body`, or `:root` selectors. Do not use `@import`. `@media`, `@keyframes`, and `@font-face` are allowed.",
    "Use the theme through CSS variables: `--background`, `--text`, `--accent`, `--muted`, `--heading-font`, `--body-font`, `--mono-font`. Write other colors as #rrggbb.",
    "The server loads the theme fonts from Google Fonts. Base styles: text is 32px in the body font and text color, headings use the heading font with margin 0, paragraphs and lists have margin 0, images are block and max-width 100%.",
    "Allowed HTML: headings, text, lists, tables, `<img>`, inline `<svg>`, `<pre><code>`, `<blockquote>`, `<a>`. Close every tag. Do not write `<script>`, `<style>`, `<iframe>`, `<object>`, `<embed>`, `<link>`, `<meta>`, forms, media, comments, on* attributes, javascript: URLs, or data: URLs.",
    "Images: `<img src='/uploads/{name}'>` for files in GET /uploads. Use no other image source. Draw charts, icons, and shapes as inline SVG or CSS.",
    "Give every id and @keyframes name a prefix unique to the screen, such as `s3-`.",
    "Font sizes: titles 80 to 120px, body 32 to 44px, captions 24 to 30px. Keep all text inside the screen with margins of at least 80px. Give boxes enough height for every line.",
    "Keep html under 20,000 characters and css under 10,000 characters. Use single quotes for HTML attribute values inside the JSON string.",
    "Put one idea on each screen. Put speaker text in notes. The renderer does not show notes on the screen.",
    "`transition` is optional. Leave it out and the design scrolls. Set it to give the design a page transition: `effect` is `none`, `fade`, `push`, `cover`, or `zoom`; `axis` is `vertical` or `horizontal`; `duration_ms` is 0 to 3000. `axis` moves `push` and `cover` only. Set it only when the user asks for one.",
];

/// Chart rules for data screens. Simplified Technical English.
pub const CHART_RULES: &[&str] = &[
    "Draw a chart as inline SVG in the screen html.",
    "Do not load a chart library.",
    "Do not write a `<script>` element.",
    "Give the SVG a `viewBox` attribute. Do not set a fixed pixel width on the SVG.",
    "Take every chart color from the design theme palette: `var(--accent)`, `var(--text)`, `var(--muted)`, and `var(--background)`.",
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
        .route("/schemas/brief", get(get_brief_schema))
        .route("/schemas/question-set", get(get_question_set_schema))
}

/// Returns the design JSON Schema, generated from the design types.
async fn get_design_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Design))
}

/// Returns the brief JSON Schema, generated from the brief types.
async fn get_brief_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(design_model::DesignBrief))
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

/// The instructions payload. Kept in one function so tests can check it
/// without a request.
fn instructions() -> serde_json::Value {
    let example_design: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json"))
            .unwrap_or_default();
    serde_json::json!({
        "purpose": "Turn a request into an approved design brief, then build HTML designs from it. Swift Design keeps the workflow state, validates, renders, and lets the user edit. Swift Design makes no LLM API calls.",
        "session": "The run works on one session. Its id is in the SWIFT_DESIGN_SESSION_ID environment variable. The mode is in SWIFT_DESIGN_RUN_MODE: briefing or generation. GET /sessions/{id} returns the state, the brief, the question sets, the answers, and the messages.",
        "steps": [
            "GET /schemas/design, GET /schemas/brief, and GET /schemas/question-set. Read the JSON Schemas.",
            "GET /sessions/{id}. Read the request, the state, the messages, the question sets, and the answers.",
            format!("Briefing mode: ask only questions that change the design. Ask in this order: artifact type and platform; audience and primary goal; primary action; required content and constraints; brand and visual direction and accessibility; technical constraints. Ask at most {limit} questions per turn. PUT /sessions/{{id}}/question-set with a BriefQuestionSet. Set required to false for a question the user may skip. The app adds a skip choice. Never invent a brand, an audience, or a conversion goal.", limit = QUESTIONS_PER_TURN_LIMIT),
            "Briefing mode: wait with the /events loop, then GET /sessions/{id} and read the answers. When the brief is ready, PUT /sessions/{id}/brief with {\"brief\": <DesignBrief>, \"source\": \"agent\"}. Keep confirmed_facts for what the user stated, assumptions for what you decided, and open_questions for what is still unknown. Do not write designs in briefing mode. The server answers 409.",
            "Generation mode: GET /sessions/{id}/brief. The brief is authoritative. Do not override a confirmed fact. Use an assumption only where no confirmed fact covers the need.",
            "Generation mode: write the design as JSON that conforms to the schema. PUT it to /designs/{id}-candidate-1. Use the session id as the base. The browser shows the candidates.",
            "A 422 response lists every problem in error.details. Fix each one. PUT again.",
            "Generation mode: if the brief lacks a detail you cannot design without, do not guess. PUT /sessions/{id}/question-set with the blocking questions. The session returns to clarifying. Then stop.",
            "GET /uploads lists the user's source files. Each row has name, size_bytes, content_type, and is_image. GET /uploads/{name} returns the file. Use an image row as `<img src='/uploads/{name}'>`.",
            "After you save a design, look at it: GET /designs/{id}/screens/{n}.png returns a PNG of screen n, 1-based. It needs Chrome or Chromium and answers 503 without one. Review every screen for overlap, overflow, empty space, and weak contrast. Fix what you see and PUT the design again. GET /designs/{id}/export returns the design as one HTML file. GET /designs/{id}/export.pdf returns it as a PDF, one page per screen.",
            "When the design is written, POST /sessions/{id}/complete, or exit with code 0. The session moves to reviewing.",
            "GET /events returns {\"revision\": n}. The revision increases when data changes. To wait for a change in one run, call GET /events?after={revision}&wait=25 in a loop. Each call returns within the wait time, so loop; do not treat a timeout as an error.",
        ],
        "rules": CONTENT_RULES,
        "charts": {
            "rules": CHART_RULES,
            "example": CHART_EXAMPLE,
        },
        "conventions": {
            "canvas": "the design's `viewport` in px (default 1440 by 900), px units, scaled by the server to any frame",
            "css_variables": ["--background", "--text", "--accent", "--muted", "--heading-font", "--body-font", "--mono-font"],
            "base_styles": "32px body text in the body font and text color; headings in the heading font with margin 0; paragraphs and lists margin 0; images block and max-width 100%",
            "node_reference": "[screen N, node a/b/c <tag.class>: text]",
            "upload_reference": "[upload name]",
        },
        "payloads": {
            "PUT /sessions/{id}/question-set": "a BriefQuestionSet",
            "PUT /sessions/{id}/brief": {"brief": "a DesignBrief", "source": "agent"},
            "PUT /designs/{id}": "the design JSON",
            "POST /designs/render": "the design JSON; returns the rendered HTML, or every validation error",
        },
        "routes": {
            "schema_design": "GET /schemas/design",
            "schema_brief": "GET /schemas/brief",
            "schema_question_set": "GET /schemas/question-set",
            "session": "GET /sessions/{id}",
            "brief": "GET /sessions/{id}/brief",
            "save_brief": "PUT /sessions/{id}/brief",
            "question_set": "PUT /sessions/{id}/question-set",
            "answers": "GET /sessions/{id} (answers field)",
            "complete": "POST /sessions/{id}/complete",
            "events": "GET /events?after={revision}&wait={seconds}",
            "uploads": "GET /uploads",
            "upload": "GET /uploads/{name}",
            "save_design": "PUT /designs/{id}",
            "check_design": "POST /designs/render",
            "render_design": "GET /designs/{id}/render",
            "screen_image": "GET /designs/{id}/screens/{n}.png",
            "export_html": "GET /designs/{id}/export",
            "export_pdf": "GET /designs/{id}/export.pdf",
            "chooser": "GET /candidates/{base}",
            "templates": "GET /templates",
            "template": "GET /templates/{id}",
        },
        "example_design": example_design,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::Design;

    use crate::instructions::{CHART_EXAMPLE, instructions};

    #[test]
    fn instructions_cover_steps_rules_routes_and_an_example() {
        let payload = instructions();
        assert!(!payload["steps"].as_array().unwrap().is_empty());
        assert!(!payload["rules"].as_array().unwrap().is_empty());
        assert_eq!(payload["routes"]["schema_design"], "GET /schemas/design");
        assert_eq!(payload["routes"]["session"], "GET /sessions/{id}");
        assert_eq!(payload["example_design"]["title"], "Swift Design Overview");
    }

    #[test]
    fn instructions_name_the_modes_and_the_gating() {
        let text = instructions().to_string();
        assert!(text.contains("Briefing mode"));
        assert!(text.contains("Generation mode"));
        assert!(text.contains("Do not write designs in briefing mode"));
        assert!(text.contains("at most 3 questions"));
        assert!(text.contains("The brief is authoritative"));
    }

    #[test]
    fn instructions_serve_every_schema() {
        let payload = instructions();
        assert_eq!(payload["routes"]["schema_brief"], "GET /schemas/brief");
        assert_eq!(
            payload["routes"]["schema_question_set"],
            "GET /schemas/question-set"
        );
        assert_eq!(payload["routes"]["templates"], "GET /templates");
    }

    #[test]
    fn instructions_describe_charts() {
        let payload = instructions();
        let text = payload.to_string();
        assert!(text.contains("Draw a chart as inline SVG in the screen html."));
        assert!(text.contains("Do not load a chart library."));
        assert!(text.contains("Do not write a `<script>` element."));
        assert!(text.contains("Give the SVG a `viewBox` attribute."));
        assert!(text.contains("Take every chart color from the design theme palette"));
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
    fn instructions_name_both_exports() {
        let payload = instructions();
        assert_eq!(payload["routes"]["export_html"], "GET /designs/{id}/export");
        assert_eq!(
            payload["routes"]["export_pdf"],
            "GET /designs/{id}/export.pdf"
        );
        assert!(payload.to_string().contains("one page per screen"));
        assert!(!payload.to_string().contains("PowerPoint"));
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
