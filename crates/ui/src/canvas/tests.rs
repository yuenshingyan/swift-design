use design_model::{LANDSCAPE_VIEWPORT, PORTRAIT_VIEWPORT, SQUARE_VIEWPORT, STORY_VIEWPORT};

use crate::api::{DeckSummary, DesignSummary, SocialSummary};

use super::*;

#[test]
fn candidate_selection_toggles_without_clearing_other_cards() {
    let mut selected = HashSet::new();
    toggle_selection(&mut selected, "first".to_owned());
    toggle_selection(&mut selected, "second".to_owned());
    assert_eq!(selected.len(), 2);
    toggle_selection(&mut selected, "first".to_owned());
    assert!(!selected.contains("first"));
    assert!(selected.contains("second"));
}

fn message(role: &str, design: Option<&str>, is_continue: bool) -> api::ChatMessage {
    api::ChatMessage {
        role: role.to_owned(),
        content: String::new(),
        design: design.map(str::to_owned),
        question_set: None,
        is_continue,
        is_queued: false,
        at: None,
        artifacts: Vec::new(),
        snapshot_stamps: None,
    }
}

#[test]
fn pending_prompts_do_not_hide_finishes_joining_the_active_turn() {
    let mut pending = message("user", None, false);
    pending.is_queued = true;
    let messages = vec![message("user", Some("a"), true), pending];
    assert!(queued_finishes(&messages, true).contains("a"));
}

#[test]
fn queued_finishes_are_the_trailing_continues_of_a_running_turn() {
    let messages = vec![
        message("user", Some("a"), true),
        message("assistant", None, false),
        message("user", None, false),
        message("user", Some("b"), true),
        message("user", Some("c"), true),
    ];
    let queued = queued_finishes(&messages, true);
    assert!(queued.contains("b") && queued.contains("c"));
    assert!(!queued.contains("a"));
    assert!(queued_finishes(&messages, false).is_empty());
}

#[test]
fn arrow_keys_step_and_other_keys_do_not() {
    assert_eq!(key_step(&Key::ArrowLeft), Some(-1));
    assert_eq!(key_step(&Key::ArrowRight), Some(1));
    assert_eq!(key_step(&Key::Enter), None);
    assert_eq!(key_step(&Key::ArrowUp), None);
}
fn phone() -> Viewport {
    Viewport {
        width: 390,
        height: 844,
    }
}

fn tablet() -> Viewport {
    Viewport {
        width: 1024,
        height: 768,
    }
}

fn card(id: &str, viewport: Viewport) -> CanvasCard {
    CanvasCard {
        id: id.to_owned(),
        title: "Today board".to_owned(),
        kind: ArtifactKind::Demo,
        count: 3,
        outline_count: 0,
        ratio: viewport.aspect_ratio_css(),
        viewport,
    }
}

fn summary(id: &str) -> DesignSummary {
    DesignSummary {
        id: id.to_owned(),
        title: "T".to_owned(),
        theme: "slate".to_owned(),
        viewport: Default::default(),
        screen_count: 3,
        outline_count: 0,
        pending_count: 0,
    }
}

fn deck_summary(id: &str) -> DeckSummary {
    DeckSummary {
        id: id.to_owned(),
        title: "T".to_owned(),
        theme: "slate".to_owned(),
        slide_count: 5,
        outline_count: 0,
        pending_count: 0,
    }
}

#[test]
fn the_tabs_are_the_canvases_in_first_seen_order() {
    let cards = vec![
        card("a-candidate-1", Viewport::default()),
        card("a-candidate-2", Viewport::default()),
        card("a-candidate-3", phone()),
    ];
    assert_eq!(canvas_tabs(&cards), vec![Viewport::default(), phone()]);
    assert_eq!(canvas_tabs(&[]), Vec::new());
    assert_eq!(cards_on_canvas(&cards, Viewport::default()), 2);
    assert_eq!(cards_on_canvas(&cards, phone()), 1);
    assert_eq!(cards_on_canvas(&cards, tablet()), 0);
}

#[test]
fn canvas_names_and_sizes_label_the_tabs() {
    assert_eq!(canvas_name(Viewport::default()), "Desktop");
    assert_eq!(canvas_name(phone()), "Phone");
    assert_eq!(canvas_name(tablet()), "Tablet");
    assert_eq!(canvas_name(DECK_VIEWPORT), "Deck");
    assert_eq!(canvas_name(A4_VIEWPORT), "A4");
    assert_eq!(canvas_name(LETTER_VIEWPORT), "Letter");
    assert_eq!(canvas_name(SQUARE_VIEWPORT), "Square");
    assert_eq!(canvas_name(PORTRAIT_VIEWPORT), "Portrait");
    assert_eq!(canvas_name(STORY_VIEWPORT), "Story");
    assert_eq!(canvas_name(LANDSCAPE_VIEWPORT), "Landscape");
    assert_eq!(canvas_name(design_model::A3_VIEWPORT), "A3");
    assert_eq!(canvas_name(design_model::A5_VIEWPORT), "A5");
    assert_eq!(canvas_name(design_model::TABLOID_VIEWPORT), "Tabloid");
    assert_eq!(
        canvas_name(Orientation::Landscape.apply(design_model::A3_VIEWPORT)),
        "A3 landscape"
    );
    assert_eq!(
        canvas_name(Orientation::Landscape.apply(A4_VIEWPORT)),
        "A4 landscape"
    );
    assert_eq!(canvas_name(design_model::STANDARD_EMAIL_VIEWPORT), "Email");
    assert_eq!(
        canvas_name(design_model::SHORT_EMAIL_VIEWPORT),
        "Short email"
    );
    assert_eq!(canvas_name(design_model::LONG_EMAIL_VIEWPORT), "Long email");
    assert_eq!(
        canvas_name(design_model::MEDIUM_RECTANGLE_AD_VIEWPORT),
        "Medium rectangle"
    );
    assert_eq!(
        canvas_name(design_model::LEADERBOARD_AD_VIEWPORT),
        "Leaderboard"
    );
    assert_eq!(
        canvas_name(design_model::MOBILE_BANNER_AD_VIEWPORT),
        "Mobile banner"
    );
    assert_eq!(
        canvas_name(design_model::THUMBNAIL_COVER_VIEWPORT),
        "Thumbnail"
    );
    assert_eq!(
        canvas_name(design_model::BANNER_COVER_VIEWPORT),
        "Channel banner"
    );
    assert_eq!(canvas_name(design_model::BOOK_COVER_VIEWPORT), "Book cover");
    assert_eq!(canvas_size(Viewport::default()), "1440 × 900");
    assert_eq!(canvas_size(DECK_VIEWPORT), "1920 × 1080");
}

#[test]
fn only_a_demo_on_a_portrait_canvas_gets_the_bezel() {
    let phone_card = CanvasCard {
        id: "app-candidate-1".to_owned(),
        title: "App".to_owned(),
        kind: ArtifactKind::Demo,
        count: 1,
        outline_count: 0,
        ratio: phone().aspect_ratio_css(),
        viewport: phone(),
    };
    assert!(phone_card.is_bezelled());
    let page_card = CanvasCard {
        kind: ArtifactKind::Document,
        ratio: A4_VIEWPORT.aspect_ratio_css(),
        viewport: A4_VIEWPORT,
        ..phone_card.clone()
    };
    assert!(is_portrait_canvas(A4_VIEWPORT));
    assert!(!page_card.is_bezelled());
    assert_eq!(
        page_card.preview_url(3, 2),
        "/documents/app-candidate-1/render?v=3&page=2"
    );
    let frame_card = CanvasCard {
        kind: ArtifactKind::Social,
        ratio: PORTRAIT_VIEWPORT.aspect_ratio_css(),
        viewport: PORTRAIT_VIEWPORT,
        ..phone_card
    };
    assert!(is_portrait_canvas(PORTRAIT_VIEWPORT));
    assert!(!frame_card.is_bezelled());
    assert_eq!(
        frame_card.preview_url(3, 2),
        "/socials/app-candidate-1/render?v=3&frame=2"
    );
}

#[test]
fn social_cards_take_their_canvas_from_the_format() {
    let socials = vec![
        SocialSummary {
            id: "post-candidate-1".to_owned(),
            title: "Launch".to_owned(),
            theme: "slate".to_owned(),
            format: Format::Landscape,
            frame_count: 1,
            outline_count: 0,
            pending_count: 0,
        },
        SocialSummary {
            id: "post-candidate-2".to_owned(),
            title: "Story".to_owned(),
            theme: "slate".to_owned(),
            format: Format::Story,
            frame_count: 2,
            outline_count: 4,
            pending_count: 0,
        },
        SocialSummary {
            id: "other-candidate-1".to_owned(),
            title: "Elsewhere".to_owned(),
            theme: "slate".to_owned(),
            format: Format::Square,
            frame_count: 1,
            outline_count: 0,
            pending_count: 0,
        },
    ];
    let cards = cards_from_socials(&socials, "post", Some("post-candidate-2"));
    assert_eq!(cards.len(), 2);
    // The chosen social comes first.
    assert_eq!(cards[0].id, "post-candidate-2");
    assert_eq!(cards[0].kind, ArtifactKind::Social);
    assert_eq!(cards[0].viewport, STORY_VIEWPORT);
    assert_eq!(cards[0].count, 2);
    assert_eq!(cards[0].remaining_count(), 2);
    assert!(cards[0].is_preview());
    assert_eq!(cards[1].viewport, LANDSCAPE_VIEWPORT);
    assert_eq!(cards[1].ratio, "1200 / 630");
    assert!(!cards[1].is_bezelled());
}

#[test]
fn phones_and_tablets_are_narrow_but_only_phones_are_portrait() {
    assert!(is_narrow_canvas(phone()));
    assert!(is_narrow_canvas(tablet()));
    assert!(!is_narrow_canvas(Viewport::default()));
    assert!(!is_narrow_canvas(DECK_VIEWPORT));
    assert!(is_portrait_canvas(phone()));
    assert!(!is_portrait_canvas(tablet()));
    assert!(!is_portrait_canvas(Viewport::default()));
    assert!(!is_portrait_canvas(DECK_VIEWPORT));
}

#[test]
fn frame_widths_follow_the_canvas_ratio() {
    assert_eq!(frame_width_rem(Viewport::default(), 13.5), "21.60");
    assert_eq!(frame_width_rem(tablet(), 13.5), "18.00");
    assert_eq!(frame_width_rem(DECK_VIEWPORT, 13.5), "24.00");
    assert_eq!(frame_width_rem(phone(), 12.0), "5.55");
    assert_eq!(frame_width_rem(tablet(), 5.5), "7.33");
    assert_eq!(frame_width_rem(phone(), 5.5), "2.54");
}

#[test]
fn the_stage_height_fits_the_cards_of_the_tab_in_one_row() {
    let desktop = Viewport::default();
    assert!((stage_height_rem(desktop, 1) - 30.0).abs() < 0.01);
    assert!((stage_height_rem(desktop, 2) - 25.94).abs() < 0.01);
    assert!((stage_height_rem(desktop, 3) - 17.08).abs() < 0.01);
    assert!((stage_height_rem(desktop, 4) - 13.5).abs() < 0.01);
    assert!((stage_height_rem(desktop, 0) - 30.0).abs() < 0.01);
    // A phone is narrow, so six still share a tall row.
    assert!((stage_height_rem(phone(), 6) - 28.5).abs() < 0.01);
    assert!((stage_height_rem(tablet(), 3) - 20.5).abs() < 0.01);
}

#[test]
fn a_wide_ad_canvas_drops_the_stage_floor_instead_of_overflowing_the_row() {
    let leaderboard = design_model::LEADERBOARD_AD_VIEWPORT;
    // One 728 by 90 card fills the row at its natural height; the
    // 13.5 rem floor would make it 109 rem wide.
    assert!((stage_height_rem(leaderboard, 1) - 10.39).abs() < 0.01);
    assert!((stage_height_rem(leaderboard, 2) - 5.13).abs() < 0.01);
    // A 320 by 100 banner still fits the row when floored, so it
    // keeps the floor like every other canvas.
    let banner = design_model::MOBILE_BANNER_AD_VIEWPORT;
    assert!((stage_height_rem(banner, 2) - 13.5).abs() < 0.01);
}

#[test]
fn a_card_frame_sets_the_width_and_both_heights() {
    let frame = card_frame(Viewport::default(), 3, false);
    assert_eq!(frame.stage_height, "17.08");
    assert_eq!(frame.width, "27.33");
    assert_eq!(
        frame.style(),
        "--frame-width: 27.33rem; --stage-height: 17.08rem; --bezel-height: 15.58rem"
    );
    // A phone frame follows the bezel, which sits inside the stage.
    let phone_frame = card_frame(phone(), 2, true);
    assert_eq!(phone_frame.stage_height, "30.00");
    assert_eq!(phone_frame.bezel_height, "28.50");
    assert_eq!(phone_frame.width, "13.17");
    // A page is portrait too, but it has no bezel: the frame takes
    // the whole stage.
    let page_frame = card_frame(A4_VIEWPORT, 2, false);
    assert_eq!(page_frame.stage_height, "30.00");
    assert_eq!(page_frame.width, "21.21");
}

#[test]
fn candidate_labels_shorten_candidate_ids() {
    assert_eq!(candidate_label("talk-candidate-2"), "Candidate 2");
    assert_eq!(candidate_label("talk-candidate-12"), "Candidate 12");
    assert_eq!(candidate_label("talk"), "Candidate");
    assert_eq!(candidate_number("talk-candidate-12"), "12");
    assert_eq!(candidate_number("talk-candidate-x"), "");
    assert_eq!(candidate_number("talk"), "");
}

#[test]
fn card_classes_carry_the_flags() {
    assert_eq!(card_class(CardFlags::default()), "canvas-card");
    let flags = CardFlags {
        is_chosen: true,
        is_phone: true,
        is_placeholder: true,
        is_selected: true,
    };
    assert_eq!(
        card_class(flags),
        "canvas-card chosen phone placeholder selected"
    );
}

#[test]
fn a_preview_counts_its_remaining_units() {
    let mut preview = card("a-candidate-1", Viewport::default());
    preview.count = 6;
    preview.outline_count = 10;
    assert!(preview.is_preview());
    assert_eq!(preview.remaining_count(), 4);
    let complete = card("a-candidate-2", Viewport::default());
    assert!(!complete.is_preview());
    assert_eq!(complete.remaining_count(), 0);
}

#[test]
fn session_designs_filter_and_put_the_chosen_first() {
    let designs = [
        summary("talk-candidate-1"),
        summary("talk-candidate-2"),
        summary("other"),
    ];
    let mine = cards_from_designs(&designs, "talk", Some("talk-candidate-2"));
    assert_eq!(mine.len(), 2);
    assert_eq!(mine[0].id, "talk-candidate-2");
    assert_eq!(mine[0].kind, ArtifactKind::Demo);
    assert_eq!(
        mine[0].preview_url(7, 2),
        "/designs/talk-candidate-2/render?v=7&screen=2"
    );
}

#[test]
fn deck_cards_count_slides_and_use_the_deck_render_url() {
    let decks = [deck_summary("talk-candidate-1"), deck_summary("other")];
    let cards = cards_from_decks(&decks, "talk", None);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].kind, ArtifactKind::Deck);
    assert_eq!(cards[0].viewport, DECK_VIEWPORT);
    assert_eq!(cards[0].ratio, "1920 / 1080");
    assert_eq!(
        cards[0].preview_url(3, 4),
        "/decks/talk-candidate-1/render?v=3&slide=4"
    );
}
