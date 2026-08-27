//! Deck patches: the model's reply to a deck edit request.
//!
//! The deck twin of `patch.rs`. A chat edit rarely touches more than a
//! slide or two, so the model replies with only what changed instead of
//! the whole deck: an optional title, an optional theme, and slide
//! operations keyed by the slide index in the deck as it was before the
//! edit. The server applies the patch and validates the result. The
//! wording differs from `patch.rs` (slides, not screens) on purpose: the
//! model sees one vocabulary per artifact kind.

use design_model::{Deck, Slide, Theme};
use serde::Deserialize;

/// One slide operation in a patch.
#[derive(Debug, Deserialize)]
pub struct SlidePatch {
    /// Zero-based index into the deck before the edit. For an insert,
    /// the new slide goes before this index; `index == slide count`
    /// appends.
    pub index: usize,
    /// True to insert a new slide at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The slide. `null` with `insert: false` deletes the slide.
    #[serde(default)]
    pub slide: Option<Slide>,
}

/// What the model sends back for a deck edit.
#[derive(Debug, Default, Deserialize)]
pub struct DeckPatch {
    /// New deck title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Slide operations. Indexes refer to the deck before the edit.
    #[serde(default)]
    pub slides: Vec<SlidePatch>,
}

/// The deck patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole deck:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"slides\":[{\"index\":2,\"slide\":{the full replacement slide}},\
{\"index\":4,\"slide\":null},\
{\"index\":5,\"insert\":true,\"slide\":{a new slide}}]}\n\
Every index is zero-based and refers to the deck as it is now, before your changes. \
A replacement carries the complete slide: html, css, and notes, changed or not. \
\"slide\": null deletes that slide. \"insert\": true puts the new slide before that index; \
an index equal to the slide count appends. Omit title, theme, and untouched slides.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<DeckPatch, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid patch: {error}"))
}

/// Applies `patch` to a copy of `deck`. Fails when an index is out of
/// range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(deck: &Deck, patch: DeckPatch) -> Result<Deck, String> {
    let mut result = deck.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = deck.slides.len();
    let mut replacements: Vec<Option<Option<Slide>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Slide>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.slides {
        if operation.insert {
            let Some(slide) = operation.slide else {
                return Err(format!(
                    "slides[{}] has insert true but no slide: include the new slide",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "slides[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(slide);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "slides[{}] does not exist: the deck has {count} slides, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.slide);
        }
    }
    let mut slides = Vec::with_capacity(count + 1);
    for index in 0..=count {
        slides.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => slides.push(replacement),
                None => slides.push(deck.slides[index].clone()),
            }
        }
    }
    result.slides = slides;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn deck() -> Deck {
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json")).unwrap()
    }

    fn note_of(slide: &Slide) -> &str {
        slide.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = deck();
        let mut replacement = original.slides[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.slides[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"slides\":[{{\"index\":0,\"slide\":{}}},{{\"index\":1,\"slide\":null}},{{\"index\":3,\"insert\":true,\"slide\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.slides.len(), 3);
        assert_eq!(note_of(&patched.slides[0]), "replaced");
        assert_eq!(note_of(&patched.slides[1]), note_of(&original.slides[2]));
        assert_eq!(note_of(&patched.slides[2]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = deck();
        let patch = parse_patch("{\"slides\":[{\"index\":7,\"slide\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"slides\":[{\"index\":9,\"insert\":true,\"slide\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_deck() {
        let original = deck();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_slides() {
        assert!(PATCH_FORMAT.contains("\"slides\""));
        assert!(!PATCH_FORMAT.contains("screen"));
    }
}
