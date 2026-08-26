//! Semantic checks that JSON Schema alone cannot express.
//!
//! Every error names the screen index and the field, so an agent can fix
//! the design from the message alone. Screen HTML and CSS are checked by
//! `markup`, which rejects unsafe and malformed markup.

use crate::markup::{SCREEN_CSS_LIMIT, SCREEN_HTML_LIMIT, css_problems, html_problems};
use crate::transition::MAX_TRANSITION_MS;
use crate::{Design, Screen};

/// A single problem found in a design.
///
/// Messages address the agent that wrote the design: they state what is
/// wrong and how to fix it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// The design `title` field is empty.
    #[error("design title is empty: set a non-empty `title`")]
    EmptyDesignTitle,
    /// The design has no screens.
    #[error("design has no screens: add at least one entry to `screens`")]
    NoScreens,
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
    /// A screen's `html` or `css` is longer than the limit.
    #[error("{path} has {length} characters: keep it under {limit}")]
    TooLarge {
        /// Field path like `screens[2].html`.
        path: String,
        /// The actual length in characters.
        length: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// A forbidden or malformed construct in a screen's `html`.
    #[error("{path}: {rule}")]
    InvalidHtml {
        /// Field path like `screens[2].html`.
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
    /// A forbidden or malformed construct in a screen's `css`.
    #[error("{path}: {rule}")]
    InvalidCss {
        /// Field path like `screens[2].css`.
        path: String,
        /// What is wrong and how to fix it.
        rule: String,
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
        let colors = &self.theme.colors;
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
        if let Some(transition) = &self.transition
            && transition.duration_ms > MAX_TRANSITION_MS
        {
            errors.push(ValidationError::TransitionTooLong {
                duration_ms: transition.duration_ms,
                limit: MAX_TRANSITION_MS,
            });
        }
        for (index, screen) in self.screens.iter().enumerate() {
            validate_screen(screen, index, &mut errors);
        }
        errors
    }
}

/// Checks one screen's html and css.
fn validate_screen(screen: &Screen, index: usize, errors: &mut Vec<ValidationError>) {
    let html_path = format!("screens[{index}].html");
    if screen.html.trim().is_empty() {
        errors.push(ValidationError::EmptyScreen { index });
    } else {
        let length = screen.html.chars().count();
        if length > SCREEN_HTML_LIMIT {
            errors.push(ValidationError::TooLarge {
                path: html_path.clone(),
                length,
                limit: SCREEN_HTML_LIMIT,
            });
        }
        for rule in html_problems(&screen.html) {
            errors.push(ValidationError::InvalidHtml {
                path: html_path.clone(),
                rule,
            });
        }
    }
    if let Some(css) = &screen.css {
        let css_path = format!("screens[{index}].css");
        let length = css.chars().count();
        if length > SCREEN_CSS_LIMIT {
            errors.push(ValidationError::TooLarge {
                path: css_path.clone(),
                length,
                limit: SCREEN_CSS_LIMIT,
            });
        }
        for rule in css_problems(css) {
            errors.push(ValidationError::InvalidCss {
                path: css_path.clone(),
                rule,
            });
        }
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
    use crate::test_support::sample_design;
    use crate::transition::MAX_TRANSITION_MS;
    use crate::validation::ValidationError;
    use crate::{Screen, Transition};

    #[test]
    fn accepts_a_valid_design() {
        assert_eq!(sample_design().validate(), Vec::new());
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
    fn rejects_a_blank_screen() {
        let mut design = sample_design();
        design.screens.push(Screen {
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
    fn html_and_css_problems_carry_field_paths() {
        let mut design = sample_design();
        design.screens.push(Screen {
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
