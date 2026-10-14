use crate::test_support::{
    sample_artwork, sample_campaign, sample_deck, sample_design, sample_document, sample_mailing,
    sample_print, sample_social,
};
use crate::transition::MAX_TRANSITION_MS;
use crate::validation::ValidationError;
use crate::viewport::{MAX_VIEWPORT_SIDE, MIN_VIEWPORT_SIDE};
use crate::{
    AD_COUNT_LIMIT, Ad, AppWindow, COVER_COUNT_LIMIT, Cover, EMAIL_COUNT_LIMIT, Email, Frame, Page,
    SHEET_COUNT_LIMIT, Screen, Sheet, Slide, Transition, Viewport,
};

#[test]
fn a_valid_social_has_no_errors() {
    assert_eq!(sample_social().validate(), Vec::new());
}

#[test]
fn reports_every_social_error_at_once() {
    let mut social = sample_social();
    social.title = String::new();
    social.theme.colors.accent = "blue".to_owned();
    social.frames.clear();
    let errors = social.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptySocialTitle));
    assert!(errors.contains(&ValidationError::NoFrames));
    assert!(errors[0].to_string().starts_with("social title is empty"));
}

#[test]
fn social_frames_use_frame_paths_in_messages() {
    let mut social = sample_social();
    social.frames.push(Frame {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    social.frames.push(Frame {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = social.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptyFrame { index: 1 }));
    assert!(messages[0].starts_with("frames[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("frames[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("frames[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("pages[")));
}

#[test]
fn a_valid_print_has_no_errors() {
    assert_eq!(sample_print().validate(), Vec::new());
}

#[test]
fn reports_every_print_error_at_once() {
    let mut print = sample_print();
    print.title = String::new();
    print.theme.colors.accent = "blue".to_owned();
    print.sheets.clear();
    let errors = print.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptyPrintTitle));
    assert!(errors.contains(&ValidationError::NoSheets));
    assert!(errors[0].to_string().starts_with("print title is empty"));
}

#[test]
fn a_print_past_the_sheet_limit_is_rejected() {
    let mut print = sample_print();
    let sheet = print.sheets[0].clone();
    while print.sheets.len() <= SHEET_COUNT_LIMIT as usize {
        print.sheets.push(sheet.clone());
    }
    let errors = print.validate();
    assert!(errors.contains(&ValidationError::TooManySheets {
        count: SHEET_COUNT_LIMIT as usize + 1,
        limit: SHEET_COUNT_LIMIT as usize,
    }));
    print.sheets.truncate(SHEET_COUNT_LIMIT as usize);
    assert_eq!(print.validate(), Vec::new());
}

#[test]
fn print_sheets_use_sheet_paths_in_messages() {
    let mut print = sample_print();
    print.sheets.push(Sheet {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    print.sheets.push(Sheet {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = print.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptySheet { index: 1 }));
    assert!(messages[0].starts_with("sheets[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("sheets[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("sheets[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("frames[")));
}

#[test]
fn a_valid_mailing_has_no_errors() {
    assert_eq!(sample_mailing().validate(), Vec::new());
}

#[test]
fn reports_every_mailing_error_at_once() {
    let mut mailing = sample_mailing();
    mailing.title = String::new();
    mailing.theme.colors.accent = "blue".to_owned();
    mailing.emails.clear();
    let errors = mailing.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptyMailingTitle));
    assert!(errors.contains(&ValidationError::NoEmails));
    assert!(errors[0].to_string().starts_with("mailing title is empty"));
}

#[test]
fn a_mailing_past_the_email_limit_is_rejected() {
    let mut mailing = sample_mailing();
    let email = mailing.emails[0].clone();
    while mailing.emails.len() <= EMAIL_COUNT_LIMIT as usize {
        mailing.emails.push(email.clone());
    }
    let errors = mailing.validate();
    assert!(errors.contains(&ValidationError::TooManyEmails {
        count: EMAIL_COUNT_LIMIT as usize + 1,
        limit: EMAIL_COUNT_LIMIT as usize,
    }));
    mailing.emails.truncate(EMAIL_COUNT_LIMIT as usize);
    assert_eq!(mailing.validate(), Vec::new());
}

#[test]
fn mailing_emails_use_email_paths_in_messages() {
    let mut mailing = sample_mailing();
    mailing.emails.push(Email {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    mailing.emails.push(Email {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = mailing.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptyEmail { index: 1 }));
    assert!(messages[0].starts_with("emails[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("emails[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("emails[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("sheets[")));
}

#[test]
fn a_valid_campaign_has_no_errors() {
    assert_eq!(sample_campaign().validate(), Vec::new());
}

#[test]
fn reports_every_campaign_error_at_once() {
    let mut campaign = sample_campaign();
    campaign.title = String::new();
    campaign.theme.colors.accent = "blue".to_owned();
    campaign.ads.clear();
    let errors = campaign.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptyCampaignTitle));
    assert!(errors.contains(&ValidationError::NoAds));
    assert!(errors[0].to_string().starts_with("campaign title is empty"));
}

#[test]
fn a_campaign_past_the_ad_limit_is_rejected() {
    let mut campaign = sample_campaign();
    let ad = campaign.ads[0].clone();
    while campaign.ads.len() <= AD_COUNT_LIMIT as usize {
        campaign.ads.push(ad.clone());
    }
    let errors = campaign.validate();
    assert!(errors.contains(&ValidationError::TooManyAds {
        count: AD_COUNT_LIMIT as usize + 1,
        limit: AD_COUNT_LIMIT as usize,
    }));
    campaign.ads.truncate(AD_COUNT_LIMIT as usize);
    assert_eq!(campaign.validate(), Vec::new());
}

#[test]
fn campaign_ads_use_ad_paths_in_messages() {
    let mut campaign = sample_campaign();
    campaign.ads.push(Ad {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    campaign.ads.push(Ad {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = campaign.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptyAd { index: 1 }));
    assert!(messages[0].starts_with("ads[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("ads[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("ads[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("emails[")));
}

#[test]
fn a_valid_artwork_has_no_errors() {
    assert_eq!(sample_artwork().validate(), Vec::new());
}

#[test]
fn reports_every_artwork_error_at_once() {
    let mut artwork = sample_artwork();
    artwork.title = String::new();
    artwork.theme.colors.accent = "blue".to_owned();
    artwork.covers.clear();
    let errors = artwork.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptyArtworkTitle));
    assert!(errors.contains(&ValidationError::NoCovers));
    assert!(errors[0].to_string().starts_with("artwork title is empty"));
}

#[test]
fn an_artwork_past_the_cover_limit_is_rejected() {
    let mut artwork = sample_artwork();
    let cover = artwork.covers[0].clone();
    while artwork.covers.len() <= COVER_COUNT_LIMIT as usize {
        artwork.covers.push(cover.clone());
    }
    let errors = artwork.validate();
    assert!(errors.contains(&ValidationError::TooManyCovers {
        count: COVER_COUNT_LIMIT as usize + 1,
        limit: COVER_COUNT_LIMIT as usize,
    }));
    artwork.covers.truncate(COVER_COUNT_LIMIT as usize);
    assert_eq!(artwork.validate(), Vec::new());
}

#[test]
fn artwork_covers_use_cover_paths_in_messages() {
    let mut artwork = sample_artwork();
    artwork.covers.push(Cover {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    artwork.covers.push(Cover {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = artwork.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptyCover { index: 1 }));
    assert!(messages[0].starts_with("covers[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("covers[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("covers[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("ads[")));
}

#[test]
fn accepts_a_valid_design() {
    assert_eq!(sample_design().validate(), Vec::new());
}

#[test]
fn a_valid_deck_has_no_errors() {
    assert_eq!(sample_deck().validate(), Vec::new());
}

#[test]
fn a_valid_document_has_no_errors() {
    assert_eq!(sample_document().validate(), Vec::new());
}

#[test]
fn reports_every_document_error_at_once() {
    let mut document = sample_document();
    document.title = String::new();
    document.theme.colors.accent = "blue".to_owned();
    document.pages.clear();
    let errors = document.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptyDocumentTitle));
    assert!(errors.contains(&ValidationError::NoPages));
    assert!(errors[0].to_string().starts_with("document title is empty"));
}

#[test]
fn document_pages_use_page_paths_in_messages() {
    let mut document = sample_document();
    document.pages.push(Page {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    document.pages.push(Page {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = document.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptyPage { index: 1 }));
    assert!(messages[0].starts_with("pages[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("pages[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("pages[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("slides[")));
}

#[test]
fn rejects_a_viewport_outside_the_limits() {
    let mut design = sample_design();
    design.viewport = Viewport {
        width: 200,
        height: 900,
    };
    assert_eq!(
        design.validate(),
        vec![ValidationError::InvalidViewport {
            width: 200,
            height: 900,
            min: MIN_VIEWPORT_SIDE,
            max: MAX_VIEWPORT_SIDE,
        }]
    );
    assert!(
        design.validate()[0]
            .to_string()
            .contains("use 320 to 4096 px")
    );
}

#[test]
fn accepts_a_bounded_app_window_with_one_top_level_marker() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 10,
        top: 20,
        width: 800,
        height: 600,
    });
    design.screens[0].html =
        "<main data-swift-design-window><h1 class='title'>Sample</h1></main>".to_owned();
    assert_eq!(design.validate(), Vec::new());
}

#[test]
fn rejects_app_window_zero_dimensions() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 0,
        top: 0,
        width: 0,
        height: 600,
    });
    let errors = design.validate();
    assert!(
        errors.contains(&ValidationError::InvalidAppWindowDimensions {
            width: 0,
            height: 600,
        })
    );
}

#[test]
fn rejects_app_window_bounds_and_overflow() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 1_000,
        top: 0,
        width: 600,
        height: 600,
    });
    let errors = design.validate();
    assert!(errors.contains(&ValidationError::AppWindowOutOfBounds {
        left: 1_000,
        top: 0,
        width: 600,
        height: 600,
        viewport_width: 1440,
        viewport_height: 900,
    }));

    design.app_window = Some(AppWindow {
        left: u32::MAX,
        top: 0,
        width: 1,
        height: 600,
    });
    let errors = design.validate();
    assert!(errors.iter().any(|error| matches!(
        error,
        ValidationError::AppWindowOutOfBounds { left: u32::MAX, .. }
    )));
}

#[test]
fn requires_an_app_window_marker_on_each_screen() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 0,
        top: 0,
        width: 800,
        height: 600,
    });
    let errors = design.validate();
    assert!(errors.contains(&ValidationError::MissingAppWindowMarker { index: 0 }));
}

#[test]
fn rejects_duplicate_app_window_markers() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 0,
        top: 0,
        width: 800,
        height: 600,
    });
    design.screens[0].html = concat!(
        "<section data-swift-design-window>One</section>",
        "<section data-swift-design-window>Two</section>"
    )
    .to_owned();
    let errors = design.validate();
    assert!(errors.contains(&ValidationError::DuplicateAppWindowMarker { index: 0, count: 2 }));
}

#[test]
fn rejects_nested_app_window_markers() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 0,
        top: 0,
        width: 800,
        height: 600,
    });
    design.screens[0].html =
        "<section><div data-swift-design-window>Nested</div></section>".to_owned();
    let errors = design.validate();
    assert!(errors.contains(&ValidationError::MissingAppWindowMarker { index: 0 }));
    assert!(errors.contains(&ValidationError::NestedAppWindowMarker { index: 0, count: 1 }));
}

#[test]
fn ignores_fake_app_window_marker_text() {
    let mut design = sample_design();
    design.app_window = Some(AppWindow {
        left: 0,
        top: 0,
        width: 800,
        height: 600,
    });
    design.screens[0].html = concat!(
        "<!-- <section data-swift-design-window></section> -->",
        "<section data-note='data-swift-design-window'>Fake</section>"
    )
    .to_owned();
    let errors = design.validate();
    assert!(errors.contains(&ValidationError::MissingAppWindowMarker { index: 0 }));
    assert!(!errors.iter().any(|error| matches!(
        error,
        ValidationError::DuplicateAppWindowMarker { .. }
            | ValidationError::NestedAppWindowMarker { .. }
    )));
}

#[test]
fn rejects_a_transition_longer_than_the_limit() {
    let mut design = sample_design();
    design.transition = Some(Transition {
        duration_ms: MAX_TRANSITION_MS,
        ..Transition::default()
    });
    assert_eq!(design.validate(), Vec::new());
    design.transition = Some(Transition {
        duration_ms: MAX_TRANSITION_MS + 1,
        ..Transition::default()
    });
    assert_eq!(
        design.validate(),
        vec![ValidationError::TransitionTooLong {
            duration_ms: MAX_TRANSITION_MS + 1,
            limit: MAX_TRANSITION_MS,
        }]
    );
}

#[test]
fn reports_every_error_not_only_the_first() {
    let mut design = sample_design();
    design.title = String::new();
    design.theme.colors.accent = "blue".to_owned();
    design.screens.clear();
    let errors = design.validate();
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&ValidationError::EmptyDesignTitle));
    assert!(errors.contains(&ValidationError::NoScreens));
}

#[test]
fn reports_every_deck_error_at_once() {
    let mut deck = sample_deck();
    deck.title = String::new();
    deck.theme.colors.accent = "blue".to_owned();
    deck.slides.clear();
    deck.transition = Some(Transition {
        duration_ms: MAX_TRANSITION_MS + 1,
        ..Transition::default()
    });
    let errors = deck.validate();
    assert_eq!(errors.len(), 4);
    assert!(errors.contains(&ValidationError::EmptyDeckTitle));
    assert!(errors.contains(&ValidationError::NoSlides));
    assert!(errors[0].to_string().starts_with("deck title is empty"));
}

#[test]
fn rejects_a_blank_screen() {
    let mut design = sample_design();
    design.screens.push(Screen {
        name: String::new(),
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    assert_eq!(
        design.validate(),
        vec![ValidationError::EmptyScreen { index: 1 }]
    );
}

#[test]
fn a_blank_screen_still_gets_its_css_checked() {
    let mut design = sample_design();
    design.screens.push(Screen {
        name: String::new(),
        html: String::new(),
        css: Some("@import url(x);".to_owned()),
        notes: None,
    });
    let errors = design.validate();
    assert!(errors.len() >= 2);
    assert_eq!(errors[0], ValidationError::EmptyScreen { index: 1 });
    assert!(
        errors[1..]
            .iter()
            .all(|error| error.to_string().starts_with("screens[1].css:"))
    );
}

#[test]
fn deck_slides_use_slide_paths_in_messages() {
    let mut deck = sample_deck();
    deck.slides.push(Slide {
        html: "   ".to_owned(),
        css: None,
        notes: None,
    });
    deck.slides.push(Slide {
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = deck.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(errors.contains(&ValidationError::EmptySlide { index: 1 }));
    assert!(messages[0].starts_with("slides[1].html is empty"));
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("slides[2].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("slides[2].css: contains `@import`"))
    );
    assert!(!messages.iter().any(|message| message.contains("screens[")));
}

#[test]
fn html_and_css_problems_carry_field_paths() {
    let mut design = sample_design();
    design.screens.push(Screen {
        name: String::new(),
        html: "<div><script>x</script>".to_owned(),
        css: Some("@import url(x); .a { width: 10vw }".to_owned()),
        notes: None,
    });
    let errors = design.validate();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("screens[1].html: contains <script>"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("screens[1].html: unclosed tags"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.starts_with("screens[1].css: contains `@import`"))
    );
    assert!(messages.iter().any(|message| message.contains("`vw`")));
}

#[test]
fn oversized_fields_are_rejected() {
    let mut design = sample_design();
    design.screens[0].html = format!("<p>{}</p>", "x".repeat(100_001));
    let errors = design.validate();
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, ValidationError::TooLarge { .. }))
    );
    let mut deck = sample_deck();
    deck.slides[0].css = Some(format!(".a {{ content: '{}' }}", "x".repeat(50_001)));
    assert!(
        deck.validate()
            .iter()
            .any(|error| matches!(error, ValidationError::TooLarge { .. }))
    );
}

#[test]
fn rejects_malformed_theme_colors() {
    let mut design = sample_design();
    design.theme.colors.muted = "#12345".to_owned();
    assert_eq!(
        design.validate(),
        vec![ValidationError::InvalidThemeColor {
            field: "muted",
            value: "#12345".to_owned(),
        }]
    );
}
