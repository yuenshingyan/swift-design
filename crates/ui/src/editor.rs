//! Editor components and helpers shared by all artifact types.
//!
//! Provides the preview bridge, node inspector, history, theme and transition
//! forms, autosave scheduling, and thumbnail helpers. Artifact-specific editors
//! live under their owning folders, including `design/editor.rs` for demos.

use design_model::transition::MAX_TRANSITION_MS;
use design_model::{
    ArtifactKind, LinkedUnit, Theme, Transition, TransitionAxis, TransitionEffect, Viewport,
    reorder_units,
};
use dioxus::document;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::canvas::{is_narrow_canvas, is_portrait_canvas};
use crate::icons;
use crate::select::{Select, plain_options};

mod history;
mod node;
mod preview;
mod support;
mod theme;
mod transition;

pub(crate) use history::HistorySection;
pub(crate) use node::{NodeInspector, node_reference, selection_paths};
pub(crate) use preview::{
    APPLY_TO_PREVIEW, NodeCommand, PREVIEW_LISTENER, PreviewFrame, PreviewMessage, PreviewMode,
    SelectedNode, SelectionEntry, pin_reference, selection_of,
};
pub(crate) use support::{
    STRIP_TILE_HEIGHT_REM, ThumbnailState, fragment_label, move_screen, navigation_target,
    optional, outline_entry, page_reference, preview_stage_class, schedule_save, strip_summary,
    thumbnail_class, toggle_pin,
};
pub(crate) use theme::ThemeForm;
pub(crate) use transition::TransitionForm;

#[cfg(test)]
pub(crate) use support::{first_heading, strip_tags};
