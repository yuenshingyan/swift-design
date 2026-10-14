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
use crate::markup::{
    SCREEN_CSS_LIMIT, SCREEN_HTML_LIMIT, css_problems, html_problems, window_marker_summary,
};
use crate::print::SHEET_COUNT_LIMIT;
use crate::transition::{MAX_TRANSITION_MS, Transition};
use crate::viewport::{MAX_VIEWPORT_SIDE, MIN_VIEWPORT_SIDE};
use crate::{
    Ad, Artwork, Campaign, Cover, Deck, Design, Document, Email, Frame, Mailing, Page, Print,
    Screen, Sheet, Slide, Social, Theme,
};

mod errors;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

pub use errors::ValidationError;

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
        app_window_problems(self, &mut errors);
        for (index, screen) in self.screens.iter().enumerate() {
            validate_screen(screen, index, &mut errors);
            app_window_marker_problems(self.app_window.is_some(), screen, index, &mut errors);
        }
        errors
    }
}

fn app_window_problems(design: &Design, errors: &mut Vec<ValidationError>) {
    let Some(app_window) = design.app_window else {
        return;
    };
    if app_window.width == 0 || app_window.height == 0 {
        errors.push(ValidationError::InvalidAppWindowDimensions {
            width: app_window.width,
            height: app_window.height,
        });
    }
    let right = app_window.left.checked_add(app_window.width);
    let bottom = app_window.top.checked_add(app_window.height);
    let is_out_of_bounds = right.is_none_or(|right| right > design.viewport.width)
        || bottom.is_none_or(|bottom| bottom > design.viewport.height);
    if is_out_of_bounds {
        errors.push(ValidationError::AppWindowOutOfBounds {
            left: app_window.left,
            top: app_window.top,
            width: app_window.width,
            height: app_window.height,
            viewport_width: design.viewport.width,
            viewport_height: design.viewport.height,
        });
    }
}

fn app_window_marker_problems(
    has_app_window: bool,
    screen: &Screen,
    index: usize,
    errors: &mut Vec<ValidationError>,
) {
    if !has_app_window || screen.html.trim().is_empty() {
        return;
    }
    let summary = window_marker_summary(&screen.html);
    if summary.top_level == 0 {
        errors.push(ValidationError::MissingAppWindowMarker { index });
    } else if summary.top_level > 1 {
        errors.push(ValidationError::DuplicateAppWindowMarker {
            index,
            count: summary.top_level,
        });
    }
    if summary.nested > 0 {
        errors.push(ValidationError::NestedAppWindowMarker {
            index,
            count: summary.nested,
        });
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
