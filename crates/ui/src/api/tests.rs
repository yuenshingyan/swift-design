use design_model::{Format, STORY_VIEWPORT};

use crate::api::{SessionOptions, SocialSummary, VARIATION_LIMIT};

#[test]
fn a_social_summary_takes_its_canvas_from_the_format() {
    let summary = SocialSummary {
        id: "post-candidate-1".to_owned(),
        title: "Launch".to_owned(),
        theme: "slate".to_owned(),
        format: Format::Story,
        frame_count: 2,
        outline_count: 3,
        pending_count: 0,
    };
    assert_eq!(summary.viewport(), STORY_VIEWPORT);
    assert_eq!(summary.aspect_ratio(), "1080 / 1920");
    assert!(summary.is_preview());
    assert!(summary.is_unfinished());
    let finished = SocialSummary {
        outline_count: 2,
        ..summary
    };
    assert!(!finished.is_preview());
    assert!(!finished.is_unfinished());
}

#[test]
fn the_social_options_start_blank() {
    let options = SessionOptions::default();
    assert_eq!(options.platform, None);
    assert_eq!(options.format, None);
    assert_eq!(options.post_goal, None);
    assert_eq!(options.frame_count, None);
}

#[test]
fn the_variation_count_defaults_to_two_and_stays_inside_the_limit() {
    assert_eq!(SessionOptions::default().variation_count(), 2);
    let counted = SessionOptions {
        variations: Some(4),
        ..SessionOptions::default()
    };
    assert_eq!(counted.variation_count(), 4);
    let too_many = SessionOptions {
        variations: Some(99),
        ..SessionOptions::default()
    };
    assert_eq!(too_many.variation_count(), VARIATION_LIMIT);
    let none = SessionOptions {
        variations: Some(0),
        ..SessionOptions::default()
    };
    assert_eq!(none.variation_count(), 1);
}
