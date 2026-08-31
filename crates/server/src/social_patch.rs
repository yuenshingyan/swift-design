//! Social patches: the model's reply to a social edit request.
//!
//! The social twin of `deck_patch.rs`. A chat edit rarely touches
//! more than a frame or two, so the model replies with only what changed
//! instead of the whole social: an optional title, an optional theme,
//! and frame operations keyed by the frame index in the social as it
//! was before the edit. The server applies the patch and validates the
//! result. The wording differs from `deck_patch.rs` (frames, not slides)
//! on purpose: the model sees one vocabulary per artifact kind.

use design_model::{Frame, Social, Theme};
use serde::Deserialize;

/// One frame operation in a patch.
#[derive(Debug, Deserialize)]
pub struct FramePatch {
    /// Zero-based index into the social before the edit. For an
    /// insert, the new frame goes before this index; `index == frame
    /// count` appends.
    pub index: usize,
    /// True to insert a new frame at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The frame. `null` with `insert: false` deletes the frame.
    #[serde(default)]
    pub frame: Option<Frame>,
}

/// What the model sends back for a social edit.
#[derive(Debug, Default, Deserialize)]
pub struct SocialPatch {
    /// New social title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Frame operations. Indexes refer to the social before the edit.
    #[serde(default)]
    pub frames: Vec<FramePatch>,
}

/// The social patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole social:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"frames\":[{\"index\":2,\"frame\":{the full replacement frame}},\
{\"index\":4,\"frame\":null},\
{\"index\":5,\"insert\":true,\"frame\":{a new frame}}]}\n\
Every index is zero-based and refers to the social as it is now, before your changes. \
A replacement carries the complete frame: html, css, and notes, changed or not. \
\"frame\": null deletes that frame. \"insert\": true puts the new frame before that index; \
an index equal to the frame count appends. Omit title, theme, and untouched frames.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<SocialPatch, String> {
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

/// Applies `patch` to a copy of `social`. Fails when an index is out
/// of range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(social: &Social, patch: SocialPatch) -> Result<Social, String> {
    let mut result = social.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = social.frames.len();
    let mut replacements: Vec<Option<Option<Frame>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Frame>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.frames {
        if operation.insert {
            let Some(frame) = operation.frame else {
                return Err(format!(
                    "frames[{}] has insert true but no frame: include the new frame",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "frames[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(frame);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "frames[{}] does not exist: the social has {count} frames, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.frame);
        }
    }
    let mut frames = Vec::with_capacity(count + 1);
    for index in 0..=count {
        frames.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => frames.push(replacement),
                None => frames.push(social.frames[index].clone()),
            }
        }
    }
    result.frames = frames;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn social() -> Social {
        serde_json::from_str(include_str!("../../../fixtures/sample-social.json")).unwrap()
    }

    fn note_of(frame: &Frame) -> &str {
        frame.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = social();
        let mut replacement = original.frames[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.frames[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"frames\":[{{\"index\":0,\"frame\":{}}},{{\"index\":1,\"frame\":null}},{{\"index\":3,\"insert\":true,\"frame\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.frames.len(), 3);
        assert_eq!(note_of(&patched.frames[0]), "replaced");
        assert_eq!(note_of(&patched.frames[1]), note_of(&original.frames[2]));
        assert_eq!(note_of(&patched.frames[2]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = social();
        let patch = parse_patch("{\"frames\":[{\"index\":7,\"frame\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"frames\":[{\"index\":9,\"insert\":true,\"frame\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_social() {
        let original = social();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_frames() {
        assert!(PATCH_FORMAT.contains("\"frames\""));
        assert!(!PATCH_FORMAT.contains("screen"));
        assert!(!PATCH_FORMAT.contains("slide"));
    }
}
