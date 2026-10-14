use std::collections::HashMap;

use design_model::{
    AD_COUNT_LIMIT, ArtifactKind, COVER_COUNT_LIMIT, DECK_SCENARIOS, EMAIL_COUNT_LIMIT,
    FRAME_COUNT_LIMIT, PAGE_COUNT_LIMIT, SHEET_COUNT_LIMIT,
};

use crate::api;

use super::*;

#[test]
fn the_record_reads_a_preset_a_typed_answer_and_a_blank() {
    let options = api::SessionOptions {
        tone: Some("technical".to_owned()),
        scenario: Some("Board offsite".to_owned()),
        slide_count: Some(10),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Deck, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let tone = row("What tone should it have?").expect("tone row");
    assert_eq!(tone.answer, "Technical and precise");
    assert!(!tone.is_assumed);
    let scenario = row("What scenario is the deck for?").expect("scenario row");
    assert_eq!(scenario.answer, "Board offsite");
    let length = row("How long should the deck be?").expect("length row");
    assert_eq!(length.answer, "10 slides");
    let audience = row("Who is it for?").expect("audience row");
    assert!(audience.is_assumed);
    assert_eq!(audience.answer, "");
}

#[test]
fn a_document_record_names_its_kind_paper_and_length() {
    let options = api::SessionOptions {
        document_kind: Some("memo".to_owned()),
        paper: Some("letter".to_owned()),
        page_count: Some(2),
        audience: Some("decision_makers".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Document, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let kind = row("What kind of document is it?").expect("kind row");
    assert_eq!(kind.answer, "Memo or brief");
    let paper = row("What paper is it for?").expect("paper row");
    assert_eq!(paper.answer, "US Letter");
    let length = row("How long should the document be?").expect("length row");
    assert_eq!(length.answer, "2 pages");
    let audience = row("Who is it for?").expect("audience row");
    assert_eq!(audience.answer, "Decision makers");
    assert!(row("What scenario is the deck for?").is_none());
    assert!(row("Canvas").is_none());
    let next = with_axis(&options, "paper", "a4".to_owned());
    assert_eq!(axis_value(&next, "paper").as_deref(), Some("a4"));
}

#[test]
fn the_page_count_options_stay_under_the_limit() {
    let options = page_count_options();
    assert_eq!(options[0].1, "The agent decides");
    assert_eq!(options[1], ("1".to_owned(), "1 page".to_owned()));
    for (value, _) in options.iter().skip(1) {
        let count = value.parse::<u32>().ok();
        assert!(
            count.is_some_and(|count| count <= PAGE_COUNT_LIMIT),
            "{value}"
        );
    }
}

#[test]
fn a_social_record_names_its_platform_format_and_length() {
    let options = api::SessionOptions {
        platform: Some("linkedin".to_owned()),
        format: Some("square".to_owned()),
        post_goal: Some("recruit".to_owned()),
        frame_count: Some(5),
        tone: Some("playful".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Social, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let platform = row("What platform is it for?").expect("platform row");
    assert_eq!(platform.answer, "LinkedIn");
    let format = row("What format is it?").expect("format row");
    assert_eq!(format.answer, "Square, 1:1");
    let goal = row("What is the post for?").expect("goal row");
    assert_eq!(goal.answer, "Recruit or invite");
    let length = row("How many frames should it have?").expect("length row");
    assert_eq!(length.answer, "5 frames");
    let tone = row("What tone should it have?").expect("tone row");
    assert!(!tone.is_assumed);
    assert!(row("What paper is it for?").is_none());
    assert!(row("What scenario is the deck for?").is_none());
    assert!(row("Canvas").is_none());
    let next = with_axis(&options, "format", "story".to_owned());
    assert_eq!(axis_value(&next, "format").as_deref(), Some("story"));
    assert_eq!(next.format.as_deref(), Some("story"));
    let cleared = with_axis(&next, "platform", String::new());
    assert_eq!(axis_value(&cleared, "platform"), None);
    assert_eq!(
        axis_value(
            &with_axis(&options, "post_goal", "educate".to_owned()),
            "post_goal"
        )
        .as_deref(),
        Some("educate")
    );
}

#[test]
fn a_print_record_names_its_kind_size_and_length() {
    let options = api::SessionOptions {
        print_kind: Some("poster".to_owned()),
        print_size: Some("a3".to_owned()),
        orientation: Some("landscape".to_owned()),
        sheet_count: Some(2),
        tone: Some("confident".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Print, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let kind = row("What kind of print piece is it?").expect("kind row");
    assert_eq!(kind.answer, "Poster");
    let size = row("What paper size is it for?").expect("size row");
    assert_eq!(size.answer, "A3");
    let orientation = row("How is it turned?").expect("orientation row");
    assert_eq!(orientation.answer, "Landscape");
    let length = row("How many sheets should it have?").expect("length row");
    assert_eq!(length.answer, "2 sheets");
    let tone = row("What tone should it have?").expect("tone row");
    assert!(!tone.is_assumed);
    assert!(row("What paper is it for?").is_none());
    assert!(row("What platform is it for?").is_none());
    let next = with_axis(&options, "print_size", "a5".to_owned());
    assert_eq!(axis_value(&next, "print_size").as_deref(), Some("a5"));
    assert_eq!(next.print_size.as_deref(), Some("a5"));
    let cleared = with_axis(&next, "orientation", String::new());
    assert_eq!(axis_value(&cleared, "orientation"), None);
}

#[test]
fn the_sheet_count_options_stay_under_the_limit() {
    let options = sheet_count_options();
    assert_eq!(options[0].1, "The agent decides");
    assert_eq!(options[1], ("1".to_owned(), "1 sheet".to_owned()));
    assert!(options.iter().any(|(_, label)| label == "4 sheets"));
    for (value, _) in options.iter().skip(1) {
        let count = value.parse::<u32>().ok();
        assert!(
            count.is_some_and(|count| (1..=SHEET_COUNT_LIMIT).contains(&count)),
            "{value}"
        );
    }
}

#[test]
fn a_mailing_record_names_its_kind_format_and_length() {
    let options = api::SessionOptions {
        email_kind: Some("newsletter".to_owned()),
        email_format: Some("long".to_owned()),
        email_count: Some(3),
        tone: Some("confident".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Mailing, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let kind = row("What kind of email is it?").expect("kind row");
    assert_eq!(kind.answer, "Newsletter");
    let format = row("What canvas is it laid out on?").expect("format row");
    assert_eq!(format.answer, "Long, a full read");
    let length = row("How many emails are in the sequence?").expect("length row");
    assert_eq!(length.answer, "3 emails");
    let tone = row("What tone should it have?").expect("tone row");
    assert!(!tone.is_assumed);
    assert!(row("What paper size is it for?").is_none());
    assert!(row("What platform is it for?").is_none());
    let next = with_axis(&options, "email_format", "short".to_owned());
    assert_eq!(axis_value(&next, "email_format").as_deref(), Some("short"));
    assert_eq!(next.email_format.as_deref(), Some("short"));
    let cleared = with_axis(&next, "email_kind", String::new());
    assert_eq!(axis_value(&cleared, "email_kind"), None);
}

#[test]
fn a_campaign_record_names_its_kind_size_and_length() {
    let options = api::SessionOptions {
        ad_kind: Some("retargeting".to_owned()),
        ad_size: Some("leaderboard".to_owned()),
        ad_count: Some(2),
        tone: Some("confident".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Campaign, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let kind = row("What kind of ad is it?").expect("kind row");
    assert_eq!(kind.answer, "Retargeting");
    let size = row("What size is it laid out on?").expect("size row");
    assert_eq!(size.answer, "Leaderboard, 728 by 90");
    let length = row("How many ads are in the set?").expect("length row");
    assert_eq!(length.answer, "2 ad variants");
    let tone = row("What tone should it have?").expect("tone row");
    assert!(!tone.is_assumed);
    assert!(row("What kind of email is it?").is_none());
    assert!(row("What platform is it for?").is_none());
    let next = with_axis(&options, "ad_size", "half_page".to_owned());
    assert_eq!(axis_value(&next, "ad_size").as_deref(), Some("half_page"));
    assert_eq!(next.ad_size.as_deref(), Some("half_page"));
    let cleared = with_axis(&next, "ad_kind", String::new());
    assert_eq!(axis_value(&cleared, "ad_kind"), None);
}

#[test]
fn an_artwork_record_names_its_kind_size_and_length() {
    let options = api::SessionOptions {
        cover_kind: Some("podcast_cover".to_owned()),
        cover_size: Some("album".to_owned()),
        cover_count: Some(2),
        tone: Some("confident".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Artwork, &options);
    let row = |question: &str| {
        entries
            .iter()
            .find(|entry| entry.question == question)
            .cloned()
    };
    let kind = row("What kind of cover is it?").expect("kind row");
    assert_eq!(kind.answer, "Podcast cover");
    let size = row("What size is it laid out on?").expect("size row");
    assert_eq!(size.answer, "Album cover, 3000 by 3000");
    let length = row("How many covers are in the set?").expect("length row");
    assert_eq!(length.answer, "2 cover variants");
    let tone = row("What tone should it have?").expect("tone row");
    assert!(!tone.is_assumed);
    assert!(row("What kind of ad is it?").is_none());
    assert!(row("What platform is it for?").is_none());
    let next = with_axis(&options, "cover_size", "book".to_owned());
    assert_eq!(axis_value(&next, "cover_size").as_deref(), Some("book"));
    assert_eq!(next.cover_size.as_deref(), Some("book"));
    let cleared = with_axis(&next, "cover_kind", String::new());
    assert_eq!(axis_value(&cleared, "cover_kind"), None);
}

#[test]
fn the_cover_count_options_stay_under_the_limit() {
    let options = cover_count_options();
    assert_eq!(options[0].1, "The agent decides");
    assert_eq!(options[1], ("1".to_owned(), "A single cover".to_owned()));
    assert!(options.iter().any(|(_, label)| label == "4 cover variants"));
    for (value, _) in options.iter().skip(1) {
        let count = value.parse::<u32>().ok();
        assert!(
            count.is_some_and(|count| (1..=COVER_COUNT_LIMIT).contains(&count)),
            "{value}"
        );
    }
}

#[test]
fn the_ad_count_options_stay_under_the_limit() {
    let options = ad_count_options();
    assert_eq!(options[0].1, "The agent decides");
    assert_eq!(options[1], ("1".to_owned(), "A single ad".to_owned()));
    assert!(options.iter().any(|(_, label)| label == "4 ad variants"));
    for (value, _) in options.iter().skip(1) {
        let count = value.parse::<u32>().ok();
        assert!(
            count.is_some_and(|count| (1..=AD_COUNT_LIMIT).contains(&count)),
            "{value}"
        );
    }
}

#[test]
fn the_email_count_options_stay_under_the_limit() {
    let options = email_count_options();
    assert_eq!(options[0].1, "The agent decides");
    assert_eq!(options[1], ("1".to_owned(), "A single email".to_owned()));
    assert!(options.iter().any(|(_, label)| label == "5 emails"));
    for (value, _) in options.iter().skip(1) {
        let count = value.parse::<u32>().ok();
        assert!(
            count.is_some_and(|count| (1..=EMAIL_COUNT_LIMIT).contains(&count)),
            "{value}"
        );
    }
}

#[test]
fn the_frame_count_options_stay_under_the_limit() {
    let options = frame_count_options();
    assert_eq!(options[0].1, "The agent decides");
    assert_eq!(options[1], ("1".to_owned(), "1 frame".to_owned()));
    assert!(options.iter().any(|(_, label)| label == "10 frames"));
    for (value, _) in options.iter().skip(1) {
        let count = value.parse::<u32>().ok();
        assert!(
            count.is_some_and(|count| (1..=FRAME_COUNT_LIMIT).contains(&count)),
            "{value}"
        );
    }
}

#[test]
fn a_demo_record_has_no_audience_and_no_tone() {
    let options = api::SessionOptions {
        audience: Some("newcomers".to_owned()),
        product_kind: Some("developer_tool".to_owned()),
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Demo, &options);
    assert!(
        entries
            .iter()
            .all(|entry| entry.question != "Who is it for?")
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.question != "What tone should it have?")
    );
    let kind = entries
        .iter()
        .find(|entry| entry.question == "What kind of product is it?")
        .expect("product row");
    assert_eq!(kind.answer, "Developer tool");
}

#[test]
fn a_pick_by_the_user_ends_the_suggestion() {
    let options = api::SessionOptions {
        product_kind: Some("developer_tool".to_owned()),
        color_mode: Some("dark".to_owned()),
        suggested: vec!["product_kind".to_owned(), "color_mode".to_owned()],
        ..Default::default()
    };
    assert!(is_suggested(&options, "product_kind"));
    let next = with_axis(&options, "product_kind", "dashboard".to_owned());
    assert_eq!(next.product_kind.as_deref(), Some("dashboard"));
    assert!(!is_suggested(&next, "product_kind"));
    assert!(is_suggested(&next, "color_mode"));
}

#[test]
fn a_demo_record_names_its_canvases() {
    let options = api::SessionOptions {
        platforms: vec!["desktop web".to_owned()],
        ..Default::default()
    };
    let entries = app_answers(ArtifactKind::Demo, &options);
    let canvas = entries.last().expect("canvas row");
    assert_eq!(canvas.question, "Canvas");
    assert_eq!(canvas.answer, "Desktop");
}

#[test]
fn a_fixed_axis_offers_the_judgment_choice_first() {
    let choices = fixed_choices(&AUDIENCES);
    assert_eq!(choices.len(), AUDIENCES.len() + 1);
    assert_eq!(choices[0], (String::new(), "The agent decides".to_owned()));
    assert!(is_judgment_choice(&choices[0].0));
    assert_eq!(choices[1].0, "newcomers".to_owned());
    assert_eq!(choices[1].1, "Newcomers to the subject".to_owned());
}

#[test]
fn a_card_takes_the_full_row_only_when_its_chips_need_it() {
    // Few short chips share a row.
    assert!(!is_wide_card(&fixed_choices(&EVIDENCE_STYLES)));
    assert!(!is_wide_card(&fixed_choices(&SLIDE_DENSITIES)));
    assert!(!is_wide_card(&fixed_choices(&DATA_STATES)));
    // Many chips, or long ones, take the row.
    assert!(is_wide_card(&fixed_choices(&COLOR_MODES)));
    assert!(is_wide_card(&fixed_choices(&PRODUCT_KINDS)));
    assert!(is_wide_card(&fixed_choices(&AUDIENCES)));
    assert!(is_wide_card(&scenario_choices()));
    // The judgment choice is drawn as a chip too, but it is short
    // and always present, so it does not count.
    assert!(!is_wide_card(&[(
        String::new(),
        "The agent decides".to_owned()
    )]));
}

#[test]
fn a_value_outside_the_chips_reads_as_a_typed_answer() {
    let choices = fixed_choices(&TONES);
    assert_eq!(
        typed_answer(Some("wry, like a changelog"), &choices),
        Some("wry, like a changelog".to_owned())
    );
    // A chip value is a chip, not a typed answer.
    assert_eq!(typed_answer(Some("playful"), &choices), None);
    // An empty value is the judgment choice.
    assert_eq!(typed_answer(Some(""), &choices), None);
    // Nothing picked yet.
    assert_eq!(typed_answer(None, &choices), None);
}

#[test]
fn the_fixed_axes_never_change_between_runs() {
    // The whole point of hardcoding them: two calls are identical.
    for axis in [&AUDIENCES[..], &TONES[..], &DEMO_SCOPES[..]] {
        assert_eq!(fixed_choices(axis), fixed_choices(axis));
    }
}

#[test]
fn every_pick_maps_to_one_canvas() {
    // Free text from the model still lands on a choice.
    assert_eq!(
        picked_platforms(&["Desktop web app".to_owned()]),
        vec!["desktop web"]
    );
    assert_eq!(picked_platforms(&["iOS app".to_owned()]), vec!["phone"]);
    assert_eq!(picked_platforms(&["iPad".to_owned()]), vec!["tablet"]);
    // No pick is the desktop canvas, which is what the renderer
    // falls back to.
    assert_eq!(picked_platforms(&[]), vec!["desktop web"]);
    // The same canvas twice is one pick.
    assert_eq!(
        picked_platforms(&["phone".to_owned(), "iPhone".to_owned()]),
        vec!["phone"]
    );
    assert_eq!(platform_choices().len(), 3);
}

#[test]
fn a_pick_toggles_and_the_last_one_stays() {
    let picked = vec!["desktop web".to_owned()];
    // Adding keeps the offered order, so the tabs never jump about.
    assert_eq!(
        toggled_platforms(&picked, "phone"),
        vec!["desktop web", "phone"]
    );
    let two = vec!["desktop web".to_owned(), "phone".to_owned()];
    assert_eq!(toggled_platforms(&two, "desktop web"), vec!["phone"]);
    // A run needs a canvas, so the last pick cannot be removed.
    assert_eq!(toggled_platforms(&picked, "desktop web"), picked);
    assert_eq!(
        toggled_platforms(&two, "tablet"),
        vec!["desktop web", "phone", "tablet"]
    );
}

#[test]
fn the_first_slide_count_option_leaves_the_length_to_the_agent() {
    let options = slide_count_options();
    assert_eq!(options[0].0, "");
    assert_eq!(options[0].1, "The agent decides");
    assert!(options.iter().any(|(value, _)| value == "12"));
    // Every other value parses back to a slide count.
    for (value, _) in options.iter().skip(1) {
        assert!(value.parse::<u32>().is_ok());
    }
}

#[test]
fn the_empty_choice_is_the_judgment_one() {
    assert!(is_judgment_choice(""));
    assert!(!is_judgment_choice("5"));
}

#[test]
fn a_judgment_pick_clears_the_field_or_sets_the_default() {
    let options = api::SessionOptions {
        variety: "high".to_owned(),
        slide_count: Some(12),
        scenario: Some("pitch".to_owned()),
        ..Default::default()
    };
    // The variety has no blank state, so judgment is the default.
    assert_eq!(with_pick(&options, "variety", "").variety, "medium");
    assert_eq!(with_pick(&options, "variety", "low").variety, "low");
    assert_eq!(with_pick(&options, "slides", "").slide_count, None);
    assert_eq!(with_pick(&options, "slides", "8").slide_count, Some(8));
    assert_eq!(with_pick(&options, "pages", "3").page_count, Some(3));
    assert_eq!(with_pick(&options, "frames", "7").frame_count, Some(7));
    assert_eq!(with_pick(&options, "frames", "").frame_count, None);
    assert_eq!(with_pick(&options, "candidates", "2").variations, Some(2));
    assert_eq!(with_pick(&options, "scenario", "").scenario, None);
    // Any other key is an axis.
    assert_eq!(
        with_pick(&options, "color_mode", "dark").color_mode,
        Some("dark".to_owned())
    );
}

#[test]
fn every_pick_on_the_page_lands_in_one_save() {
    let options = api::SessionOptions {
        suggested: vec!["color_mode".to_owned()],
        color_mode: Some("light".to_owned()),
        ..Default::default()
    };
    let picks = HashMap::from([
        ("color_mode".to_owned(), "dark".to_owned()),
        ("variety".to_owned(), String::new()),
        ("slides".to_owned(), "10".to_owned()),
    ]);
    let next = with_picks(&options, &picks);
    assert_eq!(next.color_mode, Some("dark".to_owned()));
    assert_eq!(next.variety, "medium");
    assert_eq!(next.slide_count, Some(10));
    // A pick by the user is no longer a suggestion.
    assert!(next.suggested.is_empty());
    // No picks is no change.
    assert_eq!(with_picks(&options, &HashMap::new()), options);
}

#[test]
fn the_deck_choice_lists_start_with_the_agent_and_use_the_option_values() {
    let scenarios = scenario_choices();
    assert_eq!(scenarios[0].1, "The agent decides");
    assert_eq!(scenarios.len(), DECK_SCENARIOS.len() + 1);
    let candidates = candidate_choices();
    assert_eq!(candidates[0], ("1".to_owned(), "1 candidate".to_owned()));
    assert_eq!(candidates[candidates.len() - 1].1, "5 candidates");
    let variety = variety_choices();
    assert_eq!(variety[1].0, "medium");
}

#[test]
fn state_labels_are_readable() {
    assert_eq!(state_label(WorkflowState::Clarifying), "Questions");
    assert_eq!(state_label(WorkflowState::Error), "Error");
    assert_eq!(state_label(WorkflowState::Stopped), "Stopped");
}
