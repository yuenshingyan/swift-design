//! Semantic checks that JSON Schema alone cannot express.
//!
//! Every error names the screen or slide index and the field, so an
//! agent can fix the artifact from the message alone. HTML and CSS are
//! checked by `markup`, which rejects unsafe and malformed markup. The
//! design and the deck share one error type and the same checks; only
//! the field paths differ.

use crate::markup::{SCREEN_CSS_LIMIT, SCREEN_HTML_LIMIT, css_problems, html_problems};
use crate::transition::{MAX_TRANSITION_MS, Transition};
use crate::viewport::{MAX_VIEWPORT_SIDE, MIN_VIEWPORT_SIDE};
use crate::{Deck, Design, Screen, Slide, Theme};

/// A single problem found in a design or a deck.
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

/// Checks a non-empty html fragment and its css. `path_base` is the
/// field path of the screen or slide, like `screens[2]`.
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
    use crate::test_support::{sample_deck, sample_design};
    use crate::transition::MAX_TRANSITION_MS;
    use crate::validation::ValidationError;
    use crate::viewport::{MAX_VIEWPORT_SIDE, MIN_VIEWPORT_SIDE};
    use crate::{Screen, Slide, Transition, Viewport};

    #[test]
    fn accepts_a_valid_design() {
        assert_eq!(sample_design().validate(), Vec::new());
    }

    #[test]
    fn a_valid_deck_has_no_errors() {
        assert_eq!(sample_deck().validate(), Vec::new());
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
