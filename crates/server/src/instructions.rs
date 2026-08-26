//! The agent contract, served by the app.
//!
//! Agents get everything over HTTP: `GET /instructions` returns the
//! full build procedure as JSON, and `GET /schemas/design` returns the
//! design JSON Schema generated from the Rust types at runtime, so it
//! can never go stale. No repo file is part of the agent interface.
//! Instruction strings follow Simplified Technical English.

use axum::routing::get;
use axum::{Json, Router};
use design_model::Design;

use crate::briefs::PREVIEW_SCREEN_COUNT;
use crate::candidates::CANDIDATE_LIMIT;
use crate::questions::QUESTION_LIMIT;

/// Design content rules, shared by the agent instructions and the
/// built-in generation engine. Simplified Technical English.
pub const CONTENT_RULES: &[&str] = &[
    "A screen is one HTML fragment in `html` and one CSS block in `css`.",
    "Design each screen for a canvas of 1920 by 1080 px. Use px units. Do not use vw, vh, vmin, vmax, or container units.",
    "Lay out with flex, grid, or absolute positioning. The screen root is position: relative, 1920 by 1080 px, overflow: hidden.",
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
}

/// Returns the design JSON Schema, generated from the design types.
async fn get_design_schema() -> Json<schemars::Schema> {
    Json(schemars::schema_for!(Design))
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
        "purpose": "Build an HTML presentation as one JSON design. Swift Design validates, renders, and lets the user edit it. Swift Design makes no LLM API calls.",
        "steps": [
            "GET /schemas/design. Read the JSON Schema for designs.",
            "GET /briefs. Read the user's prompt, the scenario, the length, the variation count, the project, the effort, the preview flag, the variety, the answers, and the messages. Length is the target screen count as `min-max`, like `10-15`, or `any`. Stay in the range. One screen outside it is acceptable when the content needs it. Variety says how different the candidates must be: low changes only colors and fonts, medium changes themes and arrangements, high changes themes, structure, and angle. When the scenario is absent, ask the question `What scenario is the design for?` with PUT /questions and options such as `Technology`, `Academia`, `Business`, `Finance`, `Medical`, `Education`, `Marketing`, `Startup pitch`. When the length is absent, ask the question `How long should the design be?` with the options `Short: 5 to 8 screens`, `Standard: 10 to 15 screens`, `Long: 20 to 30 screens`. When the variation count is absent, ask the question `How many candidates should I write?` with PUT /questions and the options `1 candidate` to `{CANDIDATE_LIMIT} candidates`. When variety is absent and the variation count is more than 1, ask the question `How different should the candidates be?` with the options `Low: same structure, new colors and fonts`, `Medium: new themes and arrangements, same outline`, `High: new themes, structure, and angle`. The app stores the answers as the brief's scenario, length, variations, and variety. The messages are the conversation, oldest first. The brief's `templates` field lists template ids. When it is not empty, GET /templates/{id} for each id. Give candidate 1 the first template. Give candidate 2 the second template. Start again at the first template when the candidates outnumber the templates. Use that template's theme for that candidate. Match its screens in CSS style. Write new content; do not reuse its text. A brief saved before this field can carry one `template` id instead; read it the same way. Use the project as {base} for design ids. If the response is 404, ask the user for the subject, the theme, the fonts, and the visual style.",
            "To talk to the user, POST /briefs/messages with {\"role\":\"assistant\",\"content\":\"text\"}. The user replies in the Swift Design browser UI. A new user message raises the /events revision; then GET /briefs again and read the last message. A user message may carry design: the id of the design open in the editor. Then apply the request to that design: GET /designs/{id}, change only what the request asks, and PUT /designs/{id}. A reference like [screen 3, node 0/1 <h2.title>: What Swift Design does] names a screen (1-based) and one element in its html by the index path from the screen root (zero-based child indexes), its tag and class, and the start of its text.",
            "GET /events returns {\"revision\": n}. The revision increases when data changes. To wait for a change in one run, call GET /events?after={revision}&wait=25 in a loop until the revision increases. Each call returns within the wait time, so loop; do not treat a timeout as an error.",
            format!("If the brief leaves a choice open, PUT the questions to /questions. Send at most {QUESTION_LIMIT}. The user answers in the Swift Design browser UI. Wait with the /events loop, then GET /briefs and read the answers. Treat the answer `You decide` as your choice to make."),
            "GET /uploads lists the user's source files. Each row has name, size_bytes, content_type, and is_image. GET /uploads/{name} returns the file. Read the source files to write screen content. Use an image row as `<img src='/uploads/{name}'>`. A reference like [upload chart.png] in a user message names one of these files.",
            "Write the design as JSON that conforms to the schema.",
            "PUT the design to /designs/{id}. Use a kebab-case id: lowercase letters, digits, and hyphens. Use at most 64 characters. The id `render` is reserved.",
            "A 422 response lists every problem in error.details. Fix each one. PUT again.",
            format!("When the brief asks for more than one variation, first decide one concept per candidate: angle, outline, palette, fonts, and visual idea. Make the concepts differ by the variety level. Then PUT designs named {{base}}-candidate-1 to {{base}}-candidate-N, one per concept. Write at most {CANDIDATE_LIMIT}. The browser shows them to the user at once. To learn the choice in the same run, wait with the /events loop until GET /designs/{{base}} returns 200."),
            format!("When the brief's preview flag is true, write each candidate as a preview: only the first {PREVIEW_SCREEN_COUNT} screens, starting with the title screen, plus `outline`, the screen titles of the complete design in order. A design with more outline titles than screens is a preview. The user picks a preview to continue. A user message with action `continue` and a design id asks for the rest: GET /designs/{{id}}, keep the theme and the existing screens unchanged, add one screen per remaining outline title in order, set outline to an empty list, and PUT /designs/{{id}}. When the preview flag is false, write complete candidates with an empty outline."),
            "Save the design before you tell the user it is ready. The user edits it in the browser at /.",
            "After you save a design, look at it: GET /designs/{id}/screens/{n}.png returns a PNG of screen n, 1-based. It needs Chrome or Chromium on the server machine and answers 503 without one. Review every screen for overlap, overflow, empty space, and weak contrast. Fix what you see and PUT the design again. GET /designs/{id}/export returns the design as one HTML file. GET /designs/{id}/export.pdf returns it as a PDF, one page per screen. The PDF needs Chrome too.",
        ],
        "rules": CONTENT_RULES,
        "charts": {
            "rules": CHART_RULES,
            "example": CHART_EXAMPLE,
        },
        "conventions": {
            "canvas": "1920 by 1080 px, px units, scaled by the server to any screen",
            "css_variables": ["--background", "--text", "--accent", "--muted", "--heading-font", "--body-font", "--mono-font"],
            "base_styles": "32px body text in the body font and text color; headings in the heading font with margin 0; paragraphs and lists margin 0; images block and max-width 100%",
            "node_reference": "[screen N, node a/b/c <tag.class>: text]",
            "upload_reference": "[upload name]",
        },
        "payloads": {
            "PUT /questions": {"questions": [{"question": "text", "options": ["option"]}]},
            "POST /briefs/messages": {"role": "assistant", "content": "text"},
            "PUT /designs/{id}": "the design JSON",
            "POST /designs/render": "the design JSON; returns the rendered HTML, or every validation error",
        },
        "routes": {
            "schema": "GET /schemas/design",
            "brief": "GET /briefs",
            "events": "GET /events?after={revision}&wait={seconds}",
            "questions": "PUT /questions",
            "messages": "POST /briefs/messages",
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
        assert_eq!(payload["routes"]["schema"], "GET /schemas/design");
        assert_eq!(payload["example_design"]["title"], "Swift Design Overview");
    }

    #[test]
    fn instructions_name_the_candidate_and_question_limits() {
        let text = instructions().to_string();
        assert!(text.contains("at most 5"));
        assert!(text.contains("You decide"));
        assert!(text.contains("only the first 3 screens"));
        assert!(text.contains("action `continue`"));
    }

    #[test]
    fn instructions_describe_transitions_and_templates() {
        let text = instructions().to_string();
        assert!(text.contains("`fade`, `push`, `cover`, or `zoom`"));
        assert!(text.contains("0 to 3000"));
        assert!(text.contains("The brief's `templates` field lists template ids."));
        assert!(text.contains("Start again at the first template"));
        assert_eq!(instructions()["routes"]["templates"], "GET /templates");
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
        assert!(text.contains("[upload chart.png]"));
    }
}
