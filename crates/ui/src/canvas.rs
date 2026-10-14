//! The candidate canvas: the designs, decks, documents, or socials a
//! generation run writes, each in a live preview iframe, plus
//! placeholder cards for ids the run reports before they reach disk.
//!
//! Every card shares one stage height. The frame width follows the
//! canvas ratio, so a desktop card is wide, a deck card wider, a
//! document page tall, and a phone card is a small device inside a
//! bezel. Only a demo's portrait canvas gets the bezel: a page and a
//! portrait social frame are portrait too, and neither is a phone.

use std::collections::{HashMap, HashSet};

use design_model::{
    A4_VIEWPORT, AdSize, ArtifactKind, CoverSize, DECK_VIEWPORT, EmailFormat, Format,
    LETTER_VIEWPORT, Orientation, PrintSize, Viewport,
};
use dioxus::prelude::*;

use crate::api;
use crate::icons;
use crate::settings::{artifact_project, stepped_screen};

mod cards;
mod components;

pub(crate) use cards::*;
pub(crate) use components::CandidateCanvas;

#[cfg(test)]
use components::{key_step, toggle_selection};

#[cfg(test)]
mod tests;
