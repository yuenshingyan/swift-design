//! Design patches: the model's reply to an edit request.
//!
//! A chat edit rarely touches more than a screen or two, so the model
//! replies with only what changed instead of the whole design: an
//! optional title, an optional theme, and screen operations keyed by the
//! screen index in the design as it was before the edit. The server applies
//! the patch and validates the result.

use design_model::{Design, Screen, Theme};
use serde::Deserialize;

/// One screen operation in a patch.
#[derive(Debug, Deserialize)]
pub struct ScreenPatch {
    /// Zero-based index into the design before the edit. For an insert,
    /// the new screen goes before this index; `index == screen count`
    /// appends.
    pub index: usize,
    /// True to insert a new screen at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The screen. `null` with `insert: false` deletes the screen.
    #[serde(default)]
    pub screen: Option<Screen>,
}

/// What the model sends back for an edit.
#[derive(Debug, Default, Deserialize)]
pub struct DesignPatch {
    /// New design title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Screen operations. Indexes refer to the design before the edit.
    #[serde(default)]
    pub screens: Vec<ScreenPatch>,
}

/// The patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole design:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"screens\":[{\"index\":2,\"screen\":{the full replacement screen}},\
{\"index\":4,\"screen\":null},\
{\"index\":5,\"insert\":true,\"screen\":{a new screen}}]}\n\
Every index is zero-based and refers to the design as it is now, before your changes. \
A replacement carries the complete screen: html, css, and notes, changed or not. \
\"screen\": null deletes that screen. \"insert\": true puts the new screen before that index; \
an index equal to the screen count appends. Omit title, theme, and untouched screens.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<DesignPatch, String> {
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

/// Applies `patch` to a copy of `design`. Fails when an index is out of
/// range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(design: &Design, patch: DesignPatch) -> Result<Design, String> {
    let mut result = design.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = design.screens.len();
    let mut replacements: Vec<Option<Option<Screen>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Screen>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.screens {
        if operation.insert {
            let Some(screen) = operation.screen else {
                return Err(format!(
                    "screens[{}] has insert true but no screen: include the new screen",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "screens[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(screen);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "screens[{}] does not exist: the design has {count} screens, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.screen);
        }
    }
    let mut screens = Vec::with_capacity(count + 1);
    for index in 0..=count {
        screens.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => screens.push(replacement),
                None => screens.push(design.screens[index].clone()),
            }
        }
    }
    result.screens = screens;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn design() -> Design {
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap()
    }

    fn note_of(screen: &Screen) -> &str {
        screen.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = design();
        let mut replacement = original.screens[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.screens[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"screens\":[{{\"index\":0,\"screen\":{}}},{{\"index\":1,\"screen\":null}},{{\"index\":3,\"insert\":true,\"screen\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.screens.len(), 3);
        assert_eq!(note_of(&patched.screens[0]), "replaced");
        assert_eq!(note_of(&patched.screens[1]), note_of(&original.screens[2]));
        assert_eq!(note_of(&patched.screens[2]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = design();
        let patch = parse_patch("{\"screens\":[{\"index\":7,\"screen\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"screens\":[{\"index\":9,\"insert\":true,\"screen\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_design() {
        let original = design();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }
}
