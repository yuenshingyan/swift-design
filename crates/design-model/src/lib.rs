//! Core data model for Swift Design.
//!
//! The structs in this crate are the source of truth for the design JSON
//! format that LLM agents write. `schemars` derives the JSON Schema from
//! them; regenerate `schemas/` after any change here. This crate does no IO.
//!
//! A design is a theme, a viewport, and screens, plus an optional page
//! transition. A screen is one HTML fragment plus its own CSS for the
//! viewport's px canvas. `markup` checks both.
//!
//! The crate also holds the brief-first workflow types the server and
//! the studio share: the `workflow` state machine, the `question`
//! protocol, and the `brief`.

pub mod brief;
pub mod design;
pub mod markup;
pub mod question;
pub mod screen;
pub mod theme;
pub mod transition;
pub mod validation;
pub mod viewport;
pub mod workflow;

pub use brief::{
    BriefRevision, BriefSection, Critique, CritiqueCategory, DesignBrief, RevisionSource,
};
pub use design::Design;
pub use question::{
    AnswerError, BriefQuestion, BriefQuestionSet, QUESTIONS_PER_TURN_LIMIT, QuestionAnswer,
    QuestionKind, QuestionOption, QuestionSetError, validate_answers, validate_question_set,
};
pub use screen::Screen;
pub use theme::{FontSet, Palette, Theme};
pub use transition::{Transition, TransitionAxis, TransitionEffect};
pub use validation::ValidationError;
pub use viewport::Viewport;
pub use workflow::{WorkflowError, WorkflowEvent, WorkflowState, transition};

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{Design, FontSet, Palette, Screen, Theme, Viewport};

    /// Builds a small design that passes validation.
    pub fn sample_design() -> Design {
        Design {
            title: "Sample".to_owned(),
            theme: Theme {
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
            },
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
