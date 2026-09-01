//! Artwork patches: the model's reply to an artwork edit request.
//!
//! The artwork twin of `deck_patch.rs`. A chat edit rarely touches
//! more than a cover or two, so the model replies with only what changed
//! instead of the whole artwork: an optional title, an optional theme,
//! and cover operations keyed by the cover index in the artwork as it
//! was before the edit. The server applies the patch and validates the
//! result. The wording differs from `deck_patch.rs` (covers, not slides)
//! on purpose: the model sees one vocabulary per artifact kind.

use design_model::{Artwork, Cover, Theme};
use serde::Deserialize;

/// One cover operation in a patch.
#[derive(Debug, Deserialize)]
pub struct CoverPatch {
    /// Zero-based index into the artwork before the edit. For an
    /// insert, the new cover goes before this index; `index == cover
    /// count` appends.
    pub index: usize,
    /// True to insert a new cover at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The cover. `null` with `insert: false` deletes the cover.
    #[serde(default)]
    pub cover: Option<Cover>,
}

/// What the model sends back for an artwork edit.
#[derive(Debug, Default, Deserialize)]
pub struct ArtworkPatch {
    /// New artwork title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Cover operations. Indexes refer to the artwork before the edit.
    #[serde(default)]
    pub covers: Vec<CoverPatch>,
}

/// The artwork patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole artwork:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"covers\":[{\"index\":2,\"cover\":{the full replacement cover}},\
{\"index\":4,\"cover\":null},\
{\"index\":5,\"insert\":true,\"cover\":{a new cover}}]}\n\
Every index is zero-based and refers to the artwork as it is now, before your changes. \
A replacement carries the complete cover: html, css, and notes, changed or not. \
\"cover\": null deletes that cover. \"insert\": true puts the new cover before that index; \
an index equal to the cover count appends. Omit title, theme, and untouched covers.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<ArtworkPatch, String> {
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

/// Applies `patch` to a copy of `artwork`. Fails when an index is out
/// of range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(artwork: &Artwork, patch: ArtworkPatch) -> Result<Artwork, String> {
    let mut result = artwork.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = artwork.covers.len();
    let mut replacements: Vec<Option<Option<Cover>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Cover>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.covers {
        if operation.insert {
            let Some(cover) = operation.cover else {
                return Err(format!(
                    "covers[{}] has insert true but no cover: include the new cover",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "covers[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(cover);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "covers[{}] does not exist: the artwork has {count} covers, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.cover);
        }
    }
    let mut covers = Vec::with_capacity(count + 1);
    for index in 0..=count {
        covers.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => covers.push(replacement),
                None => covers.push(artwork.covers[index].clone()),
            }
        }
    }
    result.covers = covers;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn artwork() -> Artwork {
        serde_json::from_str(include_str!("../../../fixtures/sample-artwork.json")).unwrap()
    }

    fn note_of(cover: &Cover) -> &str {
        cover.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = artwork();
        let mut replacement = original.covers[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.covers[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"covers\":[{{\"index\":0,\"cover\":{}}},{{\"index\":1,\"cover\":null}},{{\"index\":2,\"insert\":true,\"cover\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.covers.len(), 2);
        assert_eq!(note_of(&patched.covers[0]), "replaced");
        assert_eq!(note_of(&patched.covers[1]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = artwork();
        let patch = parse_patch("{\"covers\":[{\"index\":7,\"cover\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"covers\":[{\"index\":9,\"insert\":true,\"cover\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_artwork() {
        let original = artwork();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_covers() {
        assert!(PATCH_FORMAT.contains("\"covers\""));
        assert!(!PATCH_FORMAT.contains("screen"));
        assert!(!PATCH_FORMAT.contains("slide"));
    }
}
