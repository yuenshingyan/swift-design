//! Core data model for Swift Design presentations.
//!
//! The structs in this crate are the source of truth for the design JSON
//! format that LLM agents write. `schemars` derives the JSON Schema from
//! them; regenerate `schemas/` after any change here. This crate does no IO.
//!
//! A design is a theme plus screens, and an optional page transition. A
//! screen is one HTML fragment plus its own CSS for a 1920 by 1080 px
//! canvas. `markup` checks both.

pub mod design;
pub mod markup;
pub mod screen;
pub mod theme;
pub mod transition;
pub mod validation;

pub use design::Design;
pub use screen::Screen;
pub use theme::{FontSet, Palette, Theme};
pub use transition::{Transition, TransitionAxis, TransitionEffect};
pub use validation::ValidationError;

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{Design, FontSet, Palette, Screen, Theme};

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
            screens: vec![Screen {
                html: "<h1 class='title'>Sample</h1>".to_owned(),
                css: Some(".title { font-size: 96px; }".to_owned()),
                notes: None,
            }],
            outline: Vec::new(),
            transition: None,
        }
    }
}
