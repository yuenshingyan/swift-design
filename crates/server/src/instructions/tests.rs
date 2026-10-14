#![allow(clippy::unwrap_used)]

use design_model::{Artwork, Campaign, Deck, Design, Document, Mailing, Print, Social};

use crate::instructions::{
    ARTWORK_RULES, CAMPAIGN_RULES, CHART_EXAMPLE, DECK_RULES, DEMO_RULES, DOCUMENT_RULES,
    MAILING_RULES, PRINT_RULES, SOCIAL_RULES, instructions,
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
    assert!(text.contains("demo, deck, document, social, print, mailing, campaign, or artwork"));
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
    assert!(text.contains("demo, deck, document, social, print, mailing, campaign, or artwork"));
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
    assert!(text.contains("demo, deck, document, social, print, mailing, campaign, or artwork"));
    assert!(text.contains("`[email 2]`"));
    let example: Mailing = serde_json::from_value(payload["example_mailing"].clone()).unwrap();
    assert_eq!(example.validate(), Vec::new());
    assert!(payload["example_mailing"].get("viewport").is_none());
}

#[test]
fn instructions_carry_campaign_rules_routes_and_the_example() {
    let payload = instructions();
    assert_eq!(
        payload["campaign_rules"].as_array().unwrap().len(),
        CAMPAIGN_RULES.len()
    );
    let campaign = payload["campaign_rules"].to_string();
    assert!(campaign.contains("300 by 250 px"));
    assert!(campaign.contains("`size`"));
    assert!(campaign.contains("one message on each ad"));
    assert!(campaign.contains("safe margin"));
    assert!(campaign.contains("Link:"));
    assert!(campaign.contains("Do not write links between ads"));
    assert!(!campaign.contains("#screen-"));
    assert!(!campaign.contains("/decks/"));
    assert_eq!(
        payload["routes"]["schema_campaign"],
        "GET /schemas/campaign"
    );
    assert_eq!(payload["routes"]["save_campaign"], "PUT /campaigns/{id}");
    assert_eq!(
        payload["routes"]["ad_image"],
        "GET /campaigns/{id}/ads/{n}.png"
    );
    assert_eq!(
        payload["routes"]["export_campaign_zip"],
        "GET /campaigns/{id}/export.zip"
    );
    assert_eq!(
        payload["routes"]["fork_campaign"],
        "POST /campaigns/{id}/fork"
    );
    assert!(
        payload["kinds"]["campaign"]
            .as_str()
            .unwrap()
            .contains("`ads`")
    );
    let text = payload.to_string();
    assert!(text.contains("PUT it to /campaigns/{id}-candidate-1"));
    assert!(text.contains("`[ad 2]`"));
    let example: Campaign = serde_json::from_value(payload["example_campaign"].clone()).unwrap();
    assert_eq!(example.validate(), Vec::new());
    assert!(payload["example_campaign"].get("viewport").is_none());
}

#[test]
fn instructions_carry_artwork_rules_routes_and_the_example() {
    let payload = instructions();
    assert_eq!(
        payload["artwork_rules"].as_array().unwrap().len(),
        ARTWORK_RULES.len()
    );
    let artwork = payload["artwork_rules"].to_string();
    assert!(artwork.contains("1280 by 720 px"));
    assert!(artwork.contains("`size`"));
    assert!(artwork.contains("one message on each cover"));
    assert!(artwork.contains("safe margin"));
    assert!(artwork.contains("Title:"));
    assert!(artwork.contains("Do not write a call to action button"));
    assert!(!artwork.contains("#screen-"));
    assert!(!artwork.contains("/decks/"));
    assert_eq!(payload["routes"]["schema_artwork"], "GET /schemas/artwork");
    assert_eq!(payload["routes"]["save_artwork"], "PUT /artworks/{id}");
    assert_eq!(
        payload["routes"]["cover_image"],
        "GET /artworks/{id}/covers/{n}.png"
    );
    assert_eq!(
        payload["routes"]["export_artwork_zip"],
        "GET /artworks/{id}/export.zip"
    );
    assert_eq!(
        payload["routes"]["fork_artwork"],
        "POST /artworks/{id}/fork"
    );
    assert!(
        payload["kinds"]["artwork"]
            .as_str()
            .unwrap()
            .contains("`covers`")
    );
    let text = payload.to_string();
    assert!(text.contains("PUT it to /artworks/{id}-candidate-1"));
    assert!(text.contains("`[cover 2]`"));
    let example: Artwork = serde_json::from_value(payload["example_artwork"].clone()).unwrap();
    assert_eq!(example.validate(), Vec::new());
    assert!(payload["example_artwork"].get("viewport").is_none());
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
    assert!(text.contains("demo, deck, document, social, print, mailing, campaign, or artwork"));
    assert!(text.contains("`[page 2]`"));
    let example: Document = serde_json::from_value(payload["example_document"].clone()).unwrap();
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
fn demo_rules_allow_translucent_surfaces_over_a_simulated_desktop() {
    let rules = DEMO_RULES.join("\n");
    assert!(rules.contains("Use rgba() or an eight-digit hex color"));
    assert!(rules.contains("simulated desktop backdrop inside each screen"));
    assert!(rules.contains("backdrop-filter on the app shell"));
    assert!(rules.contains("Do not cover the backdrop with opaque page backgrounds"));
    assert!(!rules.contains("Write other colors as #rrggbb"));
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
fn instructions_describe_queued_prompts_and_pause_conditions() {
    let payload = instructions();
    let note = payload["conventions"]["queued_prompts"].as_str().unwrap();
    assert!(note.contains("is_queued: true"));
    assert!(note.contains("Ignore queued messages"));
    assert!(note.contains("one prompt"));
    assert!(note.contains("latest saved artifact"));
    assert!(note.contains("Stop and failure pause"));
    assert!(note.contains("Finish requests"));
}

#[test]
fn instructions_describe_parallel_editing_before_serial_coordination() {
    let payload = instructions();
    let note = payload["conventions"]["parallel_edits"].as_str().unwrap();
    assert!(note.contains("at most four"));
    assert!(note.contains("same original artifact"));
    assert!(note.contains("only its assigned unit"));
    assert!(note.contains("One serial coordination pass"));
    assert!(note.contains("before saving"));
    assert!(note.contains("stored artifact unchanged"));
}

#[test]
fn instructions_describe_saved_edit_reports_without_claiming_backend_verification() {
    let payload = instructions();
    let note = payload["conventions"]["edit_reports"].as_str().unwrap();
    assert!(note.contains("final edited artifact after fixes"));
    assert!(note.contains("one-based number"));
    assert!(note.contains("shared theme"));
    assert!(note.contains("Use the latest saved report"));
    assert!(note.contains("does not prove that an interaction or a backend works"));
}

#[test]
fn instructions_describe_charts() {
    let payload = instructions();
    let text = payload.to_string();
    assert!(text.contains("Draw a chart as inline SVG in the screen, slide, page, or frame html."));
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
        serde_json::from_str(include_str!("../../../../fixtures/sample-design.json")).unwrap();
    design.screens[0].html = format!("<div class='s4-page'>{CHART_EXAMPLE}</div>");
    assert_eq!(design.validate(), Vec::new());
}

#[test]
fn demo_rules_require_verified_tweaking_and_declared_alignment() {
    let rules = DEMO_RULES.join("\n");
    assert!(rules.contains("rechecks the exact final version"));
    assert!(rules.contains("Plain references such as Page 3"));
    assert!(rules.contains("saved report lists the verified screens"));
    assert!(rules.contains("Unused padding in scroll measurements does not block a save"));
    assert!(rules.contains("Backoff waits are 5, 10, and 20 seconds"));
    assert!(rules.contains("report the provider error and the remaining element findings"));
    assert!(rules.contains("data-layout-align='left'"));
    assert!(rules.contains("data-layout-gap='x'"));
    assert!(rules.contains("an error, not a pass"));
}

#[test]
fn demo_rules_preserve_destinations_after_screen_reordering() {
    let rules = DEMO_RULES.join("\n");
    assert!(rules.contains("The studio remaps local screen links"));
    assert!(rules.contains("Do not restore old numbers from the conversation"));
}

#[test]
fn demo_rules_define_window_bounds_per_candidate_not_across_candidates() {
    let rules = DEMO_RULES.join("\n");
    assert!(rules.contains("Choose the bounds once per candidate"));
    assert!(rules.contains("Different candidates may use different bounds"));
    assert!(rules.contains("exactly one top-level non-void shell"));
    assert!(rules.contains("Preserve `app_window` during edits, regeneration, and continuation"));
}

#[test]
fn instructions_keep_historical_questions_separate_from_current_clarification() {
    let payload = instructions();
    assert!(payload["steps"].as_array().unwrap().iter().any(|step| {
            step.as_str().unwrap().contains(
                "A question set is open only while the state is clarifying. Historical unanswered question sets do not reopen the setup card.",
            )
        }));
}

#[test]
fn instructions_describe_image_zip_handoffs_for_demos_decks_and_documents() {
    let payload = instructions();
    for (key, kind, unit) in [
        ("export_design_zip", "designs", "screen"),
        ("export_deck_zip", "decks", "slide"),
        ("export_document_zip", "documents", "page"),
    ] {
        let route = format!("GET /{kind}/{{id}}/export.zip");
        assert_eq!(payload["routes"][key], route);
        let steps = payload["steps"].as_array().unwrap();
        assert!(steps.iter().any(|step| {
            let text = step.as_str().unwrap();
            text.contains(&route) && text.contains(&format!("{{id}}-{unit}-{{n}}.png"))
        }));
    }
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
