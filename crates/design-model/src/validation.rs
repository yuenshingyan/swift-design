//! Semantic checks that JSON Schema alone cannot express.
//!
//! Every error names the screen, slide, page, frame, sheet, email,
//! ad, or cover index and the field, so an agent can fix the artifact
//! from the message alone. HTML and CSS are checked by `markup`, which
//! rejects unsafe and malformed markup. The design, the deck, the
//! document, the social, the print, the mailing, the campaign, and the
//! artwork share one error type and the same checks; only the field
//! paths differ.

use crate::artwork::COVER_COUNT_LIMIT;
use crate::campaign::AD_COUNT_LIMIT;
use crate::mailing::EMAIL_COUNT_LIMIT;
use crate::markup::{SCREEN_CSS_LIMIT, SCREEN_HTML_LIMIT, css_problems, html_problems};
use crate::print::SHEET_COUNT_LIMIT;
use crate::transition::{MAX_TRANSITION_MS, Transition};
use crate::viewport::{MAX_VIEWPORT_SIDE, MIN_VIEWPORT_SIDE};
use crate::{
    Ad, Artwork, Campaign, Cover, Deck, Design, Document, Email, Frame, Mailing, Page, Print,
    Screen, Sheet, Slide, Social, Theme,
};

/// A single problem found in a design, a deck, a document, a social,
/// a print, a mailing, a campaign, or an artwork.
///
/// Messages address the agent that wrote the artifact: they state what
/// is wrong and how to fix it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// The design `title` field is empty.
    #[error("design title is empty: set a non-empty `title`")]
    EmptyDesignTitle,
    /// The design has no screens.
    #[error("design has no screens: add at least one entry to `screens`")]
    NoScreens,
    /// The deck `title` field is empty.
    #[error("deck title is empty: set a non-empty `title`")]
    EmptyDeckTitle,
    /// The deck has no slides.
    #[error("deck has no slides: add at least one entry to `slides`")]
    NoSlides,
    /// The document `title` field is empty.
    #[error("document title is empty: set a non-empty `title`")]
    EmptyDocumentTitle,
    /// The document has no pages.
    #[error("document has no pages: add at least one entry to `pages`")]
    NoPages,
    /// The social `title` field is empty.
    #[error("social title is empty: set a non-empty `title`")]
    EmptySocialTitle,
    /// The social has no frames.
    #[error("social has no frames: add at least one entry to `frames`")]
    NoFrames,
    /// The print `title` field is empty.
    #[error("print title is empty: set a non-empty `title`")]
    EmptyPrintTitle,
    /// The print has no sheets.
    #[error("print has no sheets: add at least one entry to `sheets`")]
    NoSheets,
    /// The print has more sheets than the limit.
    #[error("print has {count} sheets: use at most {limit}")]
    TooManySheets {
        /// The rejected sheet count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// The mailing `title` field is empty.
    #[error("mailing title is empty: set a non-empty `title`")]
    EmptyMailingTitle,
    /// The mailing has no emails.
    #[error("mailing has no emails: add at least one entry to `emails`")]
    NoEmails,
    /// The mailing has more emails than the limit.
    #[error("mailing has {count} emails: use at most {limit}")]
    TooManyEmails {
        /// The rejected email count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// The campaign `title` field is empty.
    #[error("campaign title is empty: set a non-empty `title`")]
    EmptyCampaignTitle,
    /// The campaign has no ads.
    #[error("campaign has no ads: add at least one entry to `ads`")]
    NoAds,
    /// The campaign has more ads than the limit.
    #[error("campaign has {count} ads: use at most {limit}")]
    TooManyAds {
        /// The rejected ad count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// The artwork `title` field is empty.
    #[error("artwork title is empty: set a non-empty `title`")]
    EmptyArtworkTitle,
    /// The artwork has no covers.
    #[error("artwork has no covers: add at least one entry to `covers`")]
    NoCovers,
    /// The artwork has more covers than the limit.
    #[error("artwork has {count} covers: use at most {limit}")]
    TooManyCovers {
        /// The rejected cover count.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// A theme color is not a `#rrggbb` hex string.
    #[error("theme.colors.{field} has value `{value}`: use the form #rrggbb")]
    InvalidThemeColor {
        /// Which palette field is wrong.
        field: &'static str,
        /// The rejected value.
        value: String,
    },
    /// A screen's `html` is blank.
    #[error("screens[{index}].html is empty: write the screen as an HTML fragment")]
    EmptyScreen {
        /// Zero-based screen index.
        index: usize,
    },
    /// A slide's `html` is blank.
    #[error("slides[{index}].html is empty: write the slide as an HTML fragment")]
    EmptySlide {
        /// Zero-based slide index.
        index: usize,
    },
    /// A page's `html` is blank.
    #[error("pages[{index}].html is empty: write the page as an HTML fragment")]
    EmptyPage {
        /// Zero-based page index.
        index: usize,
    },
    /// A frame's `html` is blank.
    #[error("frames[{index}].html is empty: write the frame as an HTML fragment")]
    EmptyFrame {
        /// Zero-based frame index.
        index: usize,
    },
    /// A sheet's `html` is blank.
    #[error("sheets[{index}].html is empty: write the sheet as an HTML fragment")]
    EmptySheet {
        /// Zero-based sheet index.
        index: usize,
    },
    /// An email's `html` is blank.
    #[error("emails[{index}].html is empty: write the email as an HTML fragment")]
    EmptyEmail {
        /// Zero-based email index.
        index: usize,
    },
    /// An ad's `html` is blank.
    #[error("ads[{index}].html is empty: write the ad as an HTML fragment")]
    EmptyAd {
        /// Zero-based ad index.
        index: usize,
    },
    /// A cover's `html` is blank.
    #[error("covers[{index}].html is empty: write the cover as an HTML fragment")]
    EmptyCover {
        /// Zero-based cover index.
        index: usize,
    },
    /// A screen's or slide's `html` or `css` is longer than the limit.
    #[error("{path} has {length} characters: keep it under {limit}")]
    TooLarge {
        /// Field path like `screens[2].html` or `slides[2].html`.
        path: String,
        /// The actual length in characters.
        length: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// A forbidden or malformed construct in a screen's or slide's `html`.
    #[error("{path}: {rule}")]
    InvalidHtml {
        /// Field path like `screens[2].html` or `slides[2].html`.
        path: String,
        /// What is wrong and how to fix it.
        rule: String,
    },
    /// The transition lasts longer than `MAX_TRANSITION_MS`.
    #[error("transition.duration_ms is {duration_ms}: use 0 to {limit}")]
    TransitionTooLong {
        /// The rejected duration in milliseconds.
        duration_ms: u32,
        /// The allowed maximum in milliseconds.
        limit: u32,
    },
    /// A forbidden or malformed construct in a screen's or slide's `css`.
    #[error("{path}: {rule}")]
    InvalidCss {
        /// Field path like `screens[2].css` or `slides[2].css`.
        path: String,
        /// What is wrong and how to fix it.
        rule: String,
    },
    /// A viewport side is outside the allowed range.
    #[error("viewport is {width} by {height}: use {min} to {max} px for each side")]
    InvalidViewport {
        /// The rejected width in px.
        width: u32,
        /// The rejected height in px.
        height: u32,
        /// The shortest allowed side in px.
        min: u32,
        /// The longest allowed side in px.
        max: u32,
    },
}

impl Design {
    /// Checks the design and returns every problem found, not only the first.
    ///
    /// Agents fix designs from these messages, so an empty result means the
    /// design is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyDesignTitle);
        }
        if self.screens.is_empty() {
            errors.push(ValidationError::NoScreens);
        }
        theme_problems(&self.theme, &mut errors);
        transition_problems(self.transition, &mut errors);
        if !self.viewport.is_valid() {
            errors.push(ValidationError::InvalidViewport {
                width: self.viewport.width,
                height: self.viewport.height,
                min: MIN_VIEWPORT_SIDE,
                max: MAX_VIEWPORT_SIDE,
            });
        }
        for (index, screen) in self.screens.iter().enumerate() {
            validate_screen(screen, index, &mut errors);
        }
        errors
    }
}

impl Deck {
    /// Checks the deck and returns every problem found, not only the first.
    ///
    /// Agents fix decks from these messages, so an empty result means the
    /// deck is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyDeckTitle);
        }
        if self.slides.is_empty() {
            errors.push(ValidationError::NoSlides);
        }
        theme_problems(&self.theme, &mut errors);
        transition_problems(self.transition, &mut errors);
        for (index, slide) in self.slides.iter().enumerate() {
            validate_slide(slide, index, &mut errors);
        }
        errors
    }
}

impl Document {
    /// Checks the document and returns every problem found, not only
    /// the first.
    ///
    /// Agents fix documents from these messages, so an empty result
    /// means the document is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyDocumentTitle);
        }
        if self.pages.is_empty() {
            errors.push(ValidationError::NoPages);
        }
        theme_problems(&self.theme, &mut errors);
        for (index, page) in self.pages.iter().enumerate() {
            validate_page(page, index, &mut errors);
        }
        errors
    }
}

impl Social {
    /// Checks the social and returns every problem found, not only the
    /// first.
    ///
    /// Agents fix socials from these messages, so an empty result means
    /// the social is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptySocialTitle);
        }
        if self.frames.is_empty() {
            errors.push(ValidationError::NoFrames);
        }
        theme_problems(&self.theme, &mut errors);
        for (index, frame) in self.frames.iter().enumerate() {
            validate_frame(frame, index, &mut errors);
        }
        errors
    }
}

impl Print {
    /// Checks the print and returns every problem found, not only the
    /// first.
    ///
    /// Agents fix prints from these messages, so an empty result means
    /// the print is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyPrintTitle);
        }
        if self.sheets.is_empty() {
            errors.push(ValidationError::NoSheets);
        }
        if self.sheets.len() > SHEET_COUNT_LIMIT as usize {
            errors.push(ValidationError::TooManySheets {
                count: self.sheets.len(),
                limit: SHEET_COUNT_LIMIT as usize,
            });
        }
        theme_problems(&self.theme, &mut errors);
        for (index, sheet) in self.sheets.iter().enumerate() {
            validate_sheet(sheet, index, &mut errors);
        }
        errors
    }
}

impl Mailing {
    /// Checks the mailing and returns every problem found, not only
    /// the first.
    ///
    /// Agents fix mailings from these messages, so an empty result
    /// means the mailing is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyMailingTitle);
        }
        if self.emails.is_empty() {
            errors.push(ValidationError::NoEmails);
        }
        if self.emails.len() > EMAIL_COUNT_LIMIT as usize {
            errors.push(ValidationError::TooManyEmails {
                count: self.emails.len(),
                limit: EMAIL_COUNT_LIMIT as usize,
            });
        }
        theme_problems(&self.theme, &mut errors);
        for (index, email) in self.emails.iter().enumerate() {
            validate_email(email, index, &mut errors);
        }
        errors
    }
}

impl Campaign {
    /// Checks the campaign and returns every problem found, not only
    /// the first.
    ///
    /// Agents fix campaigns from these messages, so an empty result
    /// means the campaign is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyCampaignTitle);
        }
        if self.ads.is_empty() {
            errors.push(ValidationError::NoAds);
        }
        if self.ads.len() > AD_COUNT_LIMIT as usize {
            errors.push(ValidationError::TooManyAds {
                count: self.ads.len(),
                limit: AD_COUNT_LIMIT as usize,
            });
        }
        theme_problems(&self.theme, &mut errors);
        for (index, ad) in self.ads.iter().enumerate() {
            validate_ad(ad, index, &mut errors);
        }
        errors
    }
}

impl Artwork {
    /// Checks the artwork and returns every problem found, not only
    /// the first.
    ///
    /// Agents fix artworks from these messages, so an empty result
    /// means the artwork is ready to render.
    pub fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.title.trim().is_empty() {
            errors.push(ValidationError::EmptyArtworkTitle);
        }
        if self.covers.is_empty() {
            errors.push(ValidationError::NoCovers);
        }
        if self.covers.len() > COVER_COUNT_LIMIT as usize {
            errors.push(ValidationError::TooManyCovers {
                count: self.covers.len(),
                limit: COVER_COUNT_LIMIT as usize,
            });
        }
        theme_problems(&self.theme, &mut errors);
        for (index, cover) in self.covers.iter().enumerate() {
            validate_cover(cover, index, &mut errors);
        }
        errors
    }
}

/// Adds one error per theme color that is not `#rrggbb`.
fn theme_problems(theme: &Theme, errors: &mut Vec<ValidationError>) {
    let colors = &theme.colors;
    for (field, value) in [
        ("background", &colors.background),
        ("text", &colors.text),
        ("accent", &colors.accent),
        ("muted", &colors.muted),
    ] {
        if !is_hex_color(value) {
            errors.push(ValidationError::InvalidThemeColor {
                field,
                value: value.clone(),
            });
        }
    }
}

/// Adds an error when the transition lasts longer than the limit.
fn transition_problems(transition: Option<Transition>, errors: &mut Vec<ValidationError>) {
    if let Some(transition) = transition
        && transition.duration_ms > MAX_TRANSITION_MS
    {
        errors.push(ValidationError::TransitionTooLong {
            duration_ms: transition.duration_ms,
            limit: MAX_TRANSITION_MS,
        });
    }
}

/// Checks one screen's html and css.
fn validate_screen(screen: &Screen, index: usize, errors: &mut Vec<ValidationError>) {
    if screen.html.trim().is_empty() {
        errors.push(ValidationError::EmptyScreen { index });
        css_fragment_problems(
            &format!("screens[{index}].css"),
            screen.css.as_deref(),
            errors,
        );
        return;
    }
    fragment_problems(
        &format!("screens[{index}]"),
        &screen.html,
        screen.css.as_deref(),
        errors,
    );
}

/// Checks one slide's html and css.
fn validate_slide(slide: &Slide, index: usize, errors: &mut Vec<ValidationError>) {
    if slide.html.trim().is_empty() {
        errors.push(ValidationError::EmptySlide { index });
        css_fragment_problems(
            &format!("slides[{index}].css"),
            slide.css.as_deref(),
            errors,
        );
        return;
    }
    fragment_problems(
        &format!("slides[{index}]"),
        &slide.html,
        slide.css.as_deref(),
        errors,
    );
}

/// Checks one page's html and css.
fn validate_page(page: &Page, index: usize, errors: &mut Vec<ValidationError>) {
    if page.html.trim().is_empty() {
        errors.push(ValidationError::EmptyPage { index });
        css_fragment_problems(&format!("pages[{index}].css"), page.css.as_deref(), errors);
        return;
    }
    fragment_problems(
        &format!("pages[{index}]"),
        &page.html,
        page.css.as_deref(),
        errors,
    );
}

/// Checks one frame's html and css.
fn validate_frame(frame: &Frame, index: usize, errors: &mut Vec<ValidationError>) {
    if frame.html.trim().is_empty() {
        errors.push(ValidationError::EmptyFrame { index });
        css_fragment_problems(
            &format!("frames[{index}].css"),
            frame.css.as_deref(),
            errors,
        );
        return;
    }
    fragment_problems(
        &format!("frames[{index}]"),
        &frame.html,
        frame.css.as_deref(),
        errors,
    );
}

/// Checks one sheet's html and css.
fn validate_sheet(sheet: &Sheet, index: usize, errors: &mut Vec<ValidationError>) {
    if sheet.html.trim().is_empty() {
        errors.push(ValidationError::EmptySheet { index });
        css_fragment_problems(
            &format!("sheets[{index}].css"),
            sheet.css.as_deref(),
            errors,
        );
        return;
    }
    fragment_problems(
        &format!("sheets[{index}]"),
        &sheet.html,
        sheet.css.as_deref(),
        errors,
    );
}

/// Checks one email's html and css.
fn validate_email(email: &Email, index: usize, errors: &mut Vec<ValidationError>) {
    if email.html.trim().is_empty() {
        errors.push(ValidationError::EmptyEmail { index });
        css_fragment_problems(
            &format!("emails[{index}].css"),
            email.css.as_deref(),
            errors,
        );
        return;
    }
    fragment_problems(
        &format!("emails[{index}]"),
        &email.html,
        email.css.as_deref(),
        errors,
    );
}

/// Checks one ad's html and css.
fn validate_ad(ad: &Ad, index: usize, errors: &mut Vec<ValidationError>) {
    if ad.html.trim().is_empty() {
        errors.push(ValidationError::EmptyAd { index });
        css_fragment_problems(&format!("ads[{index}].css"), ad.css.as_deref(), errors);
        return;
    }
    fragment_problems(
        &format!("ads[{index}]"),
        &ad.html,
        ad.css.as_deref(),
        errors,
    );
}

/// Checks one cover's html and css.
fn validate_cover(cover: &Cover, index: usize, errors: &mut Vec<ValidationError>) {
    if cover.html.trim().is_empty() {
        errors.push(ValidationError::EmptyCover { index });
        css_fragment_problems(
            &format!("covers[{index}].css"),
            cover.css.as_deref(),
            errors,
        );
        return;
    }
    fragment_problems(
        &format!("covers[{index}]"),
        &cover.html,
        cover.css.as_deref(),
        errors,
    );
}

/// Checks a non-empty html fragment and its css. `path_base` is the
/// field path of the screen, slide, page, frame, sheet, email, ad, or
/// cover, like `screens[2]`.
fn fragment_problems(
    path_base: &str,
    html: &str,
    css: Option<&str>,
    errors: &mut Vec<ValidationError>,
) {
    let html_path = format!("{path_base}.html");
    let length = html.chars().count();
    if length > SCREEN_HTML_LIMIT {
        errors.push(ValidationError::TooLarge {
            path: html_path.clone(),
            length,
            limit: SCREEN_HTML_LIMIT,
        });
    }
    for rule in html_problems(html) {
        errors.push(ValidationError::InvalidHtml {
            path: html_path.clone(),
            rule,
        });
    }
    css_fragment_problems(&format!("{path_base}.css"), css, errors);
}

/// Checks a css block, when there is one. `css_path` is its field path.
fn css_fragment_problems(css_path: &str, css: Option<&str>, errors: &mut Vec<ValidationError>) {
    let Some(css) = css else {
        return;
    };
    let length = css.chars().count();
    if length > SCREEN_CSS_LIMIT {
        errors.push(ValidationError::TooLarge {
            path: css_path.to_owned(),
            length,
            limit: SCREEN_CSS_LIMIT,
        });
    }
    for rule in css_problems(css) {
        errors.push(ValidationError::InvalidCss {
            path: css_path.to_owned(),
            rule,
        });
    }
}

/// True for strings of the form `#rrggbb`.
fn is_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::test_support::{
        sample_artwork, sample_campaign, sample_deck, sample_design, sample_document,
        sample_mailing, sample_print, sample_social,
    };
    use crate::transition::MAX_TRANSITION_MS;
    use crate::validation::ValidationError;
    use crate::viewport::{MAX_VIEWPORT_SIDE, MIN_VIEWPORT_SIDE};
    use crate::{
        AD_COUNT_LIMIT, Ad, COVER_COUNT_LIMIT, Cover, EMAIL_COUNT_LIMIT, Email, Frame, Page,
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
}
