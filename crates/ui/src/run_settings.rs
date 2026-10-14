//! The run settings the app asks for instead of the agent: the canvas
//! for a demo, the scenario, the length, the candidate count, and the
//! variety for a deck, the paper and the length for a document, the
//! platform, the format, and the length for a social, the print kind,
//! the size, the orientation, and the length for a print, the email
//! kind, the format, and the length for a mailing, the ad kind, the
//! size, and the length for a campaign, the cover kind, the size, and
//! the length for an artwork, and the number of variations. Each has a
//! closed set of answers, so a chip settles it in one click.

use std::collections::HashMap;

use design_model::{
    AD_KINDS, AD_SIZES, AUDIENCES, AnsweredQuestion, ArtifactKind, COLOR_MODES, COVER_KINDS,
    COVER_SIZES, CUSTOM_ANSWER_LIMIT, DATA_STATES, DECK_SCENARIOS, DECK_VARIETY_LEVELS,
    DEMO_SCOPES, DOCUMENT_KINDS, EMAIL_FORMATS, EMAIL_KINDS, EVIDENCE_STYLES, FIDELITIES, FORMATS,
    ORIENTATIONS, PAPERS, PLATFORMS, POST_GOALS, PRINT_KINDS, PRINT_SIZES, PRODUCT_KINDS,
    SLIDE_DENSITIES, TONES, Viewport, WorkflowState,
};
use dioxus::prelude::*;

use crate::api;
use crate::chat_controls::CountChips;
use crate::icons;

mod components;
mod questions;

pub(crate) use components::{
    ArtworkQuestions, CampaignQuestions, DeckQuestions, DocumentQuestions, MailingQuestions,
    PrintQuestions, RunSettings, SharedQuestions, SocialQuestions,
};
pub(crate) use questions::app_answers;

#[cfg(test)]
use components::{
    candidate_choices, is_judgment_choice, scenario_choices, typed_answer, variety_choices,
};
#[cfg(test)]
use questions::{
    axis_value, fixed_choices, is_suggested, is_wide_card, with_axis, with_pick, with_picks,
};

/// The readable name of a workflow state.
pub(crate) fn state_label(state: WorkflowState) -> &'static str {
    match state {
        WorkflowState::Intake => "Starting",
        WorkflowState::Clarifying => "Questions",
        WorkflowState::Generating => "Generating",
        WorkflowState::Reviewing => "Reviewing",
        WorkflowState::Stopped => "Stopped",
        WorkflowState::Error => "Error",
    }
}

/// One canvas the user can pick, as it appears in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlatformChoice {
    /// The value written to the brief.
    pub value: &'static str,
    /// The name under the icon.
    pub label: &'static str,
    /// The canvas size, for the caption.
    pub size: &'static str,
    /// The icon markup.
    pub icon: &'static str,
}

/// The canvases the app offers. The three cover every viewport the
/// renderer has, so a pick always resolves to a real canvas.
pub(crate) fn platform_choices() -> Vec<PlatformChoice> {
    vec![
        PlatformChoice {
            value: "desktop web",
            label: "Desktop",
            size: "1440 × 900",
            icon: icons::MONITOR,
        },
        PlatformChoice {
            value: "phone",
            label: "Phone",
            size: "390 × 844",
            icon: icons::PHONE,
        },
        PlatformChoice {
            value: "tablet",
            label: "Tablet",
            size: "1024 × 768",
            icon: icons::TABLET,
        },
    ]
}

/// The canvases the run has picked, as choice values.
///
/// Free text from a model still lands on a choice, because the viewport
/// decides which one it is. Never empty: no pick means the desktop
/// canvas, which is what the renderer defaults to.
pub(crate) fn picked_platforms(platforms: &[String]) -> Vec<String> {
    let choices = platform_choices();
    let mut picked: Vec<String> = Vec::new();
    for platform in platforms {
        let viewport = Viewport::for_platform(platform);
        let Some(choice) = choices
            .iter()
            .find(|choice| Viewport::for_platform(choice.value) == viewport)
        else {
            continue;
        };
        if !picked.iter().any(|value| value == choice.value) {
            picked.push(choice.value.to_owned());
        }
    }
    if picked.is_empty() {
        picked.push("desktop web".to_owned());
    }
    picked
}

/// The platforms after the user clicked `value`.
///
/// Clicking an unpicked canvas adds it; clicking a picked one removes
/// it. The last one cannot be removed: a run needs a canvas.
pub(crate) fn toggled_platforms(picked: &[String], value: &str) -> Vec<String> {
    if picked.iter().any(|platform| platform == value) {
        if picked.len() == 1 {
            return picked.to_vec();
        }
        return picked
            .iter()
            .filter(|platform| *platform != value)
            .cloned()
            .collect();
    }
    // Keep the offered order, so the tabs never jump about.
    platform_choices()
        .into_iter()
        .map(|choice| choice.value.to_owned())
        .filter(|choice| choice == value || picked.iter().any(|platform| platform == choice))
        .collect()
}

/// The slide counts the app offers, as (brief value, label). The empty
/// value leaves the length to the agent.
pub(crate) fn slide_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [5, 8, 10, 12, 15, 20, 30] {
        options.push((count.to_string(), format!("{count} slides")));
    }
    options
}

/// The page counts the app offers, as (brief value, label). The empty
/// value leaves the length to the agent.
pub(crate) fn page_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [1, 2, 3, 5, 8, 12, 20] {
        if count > design_model::PAGE_COUNT_LIMIT {
            break;
        }
        let label = if count == 1 {
            "1 page".to_owned()
        } else {
            format!("{count} pages")
        };
        options.push((count.to_string(), label));
    }
    options
}

/// The frame counts the app offers, as (brief value, label). The empty
/// value leaves the length to the agent. One frame is a single post;
/// more are a carousel.
pub(crate) fn frame_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [1, 3, 5, 7, 10] {
        if count > design_model::FRAME_COUNT_LIMIT {
            break;
        }
        let label = if count == 1 {
            "1 frame".to_owned()
        } else {
            format!("{count} frames")
        };
        options.push((count.to_string(), label));
    }
    options
}

/// The sheet counts the app offers, as (brief value, label). The empty
/// value leaves the length to the agent. One sheet is a poster; two
/// are the front and the back of a flyer.
pub(crate) fn sheet_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [1, 2, 3, 4] {
        if count > design_model::SHEET_COUNT_LIMIT {
            break;
        }
        let label = if count == 1 {
            "1 sheet".to_owned()
        } else {
            format!("{count} sheets")
        };
        options.push((count.to_string(), label));
    }
    options
}

/// The email counts the app offers, as (brief value, label). The
/// empty value leaves the length to the agent. One email is a single
/// send; two or more are a sequence.
pub(crate) fn email_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [1, 2, 3, 4, 5] {
        if count > design_model::EMAIL_COUNT_LIMIT {
            break;
        }
        let label = if count == 1 {
            "A single email".to_owned()
        } else {
            format!("{count} emails")
        };
        options.push((count.to_string(), label));
    }
    options
}

/// The ad counts the app offers, as (brief value, label). The empty
/// value leaves the length to the agent. One ad is a single placement;
/// two or more are A/B variants.
pub(crate) fn ad_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [1, 2, 3, 4] {
        if count > design_model::AD_COUNT_LIMIT {
            break;
        }
        let label = if count == 1 {
            "A single ad".to_owned()
        } else {
            format!("{count} ad variants")
        };
        options.push((count.to_string(), label));
    }
    options
}

/// The cover counts the app offers, as (brief value, label). The
/// empty value leaves the length to the agent. One cover is a single
/// piece; two or more are A/B variants.
pub(crate) fn cover_count_options() -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "The agent decides".to_owned())];
    for count in [1, 2, 3, 4] {
        if count > design_model::COVER_COUNT_LIMIT {
            break;
        }
        let label = if count == 1 {
            "A single cover".to_owned()
        } else {
            format!("{count} cover variants")
        };
        options.push((count.to_string(), label));
    }
    options
}

#[cfg(test)]
mod tests;
