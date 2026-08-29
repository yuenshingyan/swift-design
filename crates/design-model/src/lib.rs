//! Core data model for Swift Design.
//!
//! The structs in this crate are the source of truth for the design JSON
//! format that LLM agents write. `schemars` derives the JSON Schema from
//! them; regenerate `schemas/` after any change here. This crate does no IO.
//!
//! There are two artifact kinds. A design is a theme, a viewport, and
//! screens, plus an optional page transition. A deck is a theme and
//! slides on a fixed 1920 by 1080 px canvas, plus an optional page
//! transition. A screen or a slide is one HTML fragment plus its own
//! CSS. `markup` checks both.
//!
//! The crate also holds the brief-first workflow types the server and
//! the studio share: the `workflow` state machine, the `question`
//! protocol, and the `brief`.

pub mod artifact_kind;
pub mod deck;
pub mod deck_questions;
pub mod design;
pub mod markup;
pub mod question;
pub mod run_questions;
pub mod screen;
pub mod slide;
pub mod text;
pub mod theme;
pub mod transition;
pub mod validation;
pub mod viewport;
pub mod workflow;

pub use artifact_kind::ArtifactKind;
pub use deck::{DECK_HEIGHT, DECK_VIEWPORT, DECK_WIDTH, Deck};
pub use deck_questions::{DECK_SCENARIOS, DECK_VARIETY_LEVELS, is_deck_scenario};
pub use design::Design;
pub use question::{
    AnswerError, AnsweredQuestion, BriefQuestion, BriefQuestionSet, QUESTIONS_PER_TURN_LIMIT,
    QuestionAnswer, QuestionKind, QuestionOption, QuestionSetError, validate_answers,
    validate_question_set,
};
pub use run_questions::{
    AUDIENCES, AppAxis, COLOR_MODES, CUSTOM_ANSWER_LIMIT, DATA_STATES, DECK_AXES, DEMO_AXES,
    DEMO_SCOPES, EVIDENCE_STYLES, PRODUCT_KINDS, SHARED_AXES, SLIDE_DENSITIES, TONES, app_axes,
    audience_label, axis_by_key, axis_label, demo_scope_label, is_custom_answer, tone_label,
};
pub use screen::Screen;
pub use slide::Slide;
pub use theme::{FontSet, Palette, Theme};
pub use transition::{Transition, TransitionAxis, TransitionEffect};
pub use validation::ValidationError;
pub use viewport::Viewport;
pub use workflow::{WorkflowError, WorkflowEvent, WorkflowState, transition};

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{Deck, Design, FontSet, Palette, Screen, Slide, Theme, Viewport};

    /// The theme every sample artifact uses.
    fn sample_theme() -> Theme {
        Theme {
            name: "midnight".to_owned(),
            colors: Palette {
                background: "#101418".to_owned(),
                text: "#f5f5f5".to_owned(),
                accent: "#4f8cff".to_owned(),
                muted: "#8a94a6".to_owned(),
            },
            fonts: FontSet {
                heading: "Inter".to_owned(),
                body: "Inter".to_owned(),
                mono: "JetBrains Mono".to_owned(),
            },
        }
    }

    /// Builds a small deck that passes validation.
    pub fn sample_deck() -> Deck {
        Deck {
            title: "Sample".to_owned(),
            theme: sample_theme(),
            slides: vec![Slide {
                html: "<h1 class='title'>Sample</h1>".to_owned(),
                css: Some(".title { font-size: 96px; }".to_owned()),
                notes: None,
            }],
            outline: Vec::new(),
            transition: None,
        }
    }

    /// Builds a small design that passes validation.
    pub fn sample_design() -> Design {
        Design {
            title: "Sample".to_owned(),
            theme: sample_theme(),
            viewport: Viewport::default(),
            screens: vec![Screen {
                name: "Home".to_owned(),
                html: "<h1 class='title'>Sample</h1>".to_owned(),
                css: Some(".title { font-size: 96px; }".to_owned()),
                notes: None,
            }],
            outline: Vec::new(),
            transition: None,
        }
    }
}
