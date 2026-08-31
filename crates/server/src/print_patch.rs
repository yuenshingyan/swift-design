//! Print patches: the model's reply to a print edit request.
//!
//! The print twin of `deck_patch.rs`. A chat edit rarely touches
//! more than a sheet or two, so the model replies with only what changed
//! instead of the whole print: an optional title, an optional theme,
//! and sheet operations keyed by the sheet index in the print as it
//! was before the edit. The server applies the patch and validates the
//! result. The wording differs from `deck_patch.rs` (sheets, not slides)
//! on purpose: the model sees one vocabulary per artifact kind.

use design_model::{Print, Sheet, Theme};
use serde::Deserialize;

/// One sheet operation in a patch.
#[derive(Debug, Deserialize)]
pub struct SheetPatch {
    /// Zero-based index into the print before the edit. For an
    /// insert, the new sheet goes before this index; `index == sheet
    /// count` appends.
    pub index: usize,
    /// True to insert a new sheet at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The sheet. `null` with `insert: false` deletes the sheet.
    #[serde(default)]
    pub sheet: Option<Sheet>,
}

/// What the model sends back for a print edit.
#[derive(Debug, Default, Deserialize)]
pub struct PrintPatch {
    /// New print title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Sheet operations. Indexes refer to the print before the edit.
    #[serde(default)]
    pub sheets: Vec<SheetPatch>,
}

/// The print patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole print:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"sheets\":[{\"index\":2,\"sheet\":{the full replacement sheet}},\
{\"index\":4,\"sheet\":null},\
{\"index\":5,\"insert\":true,\"sheet\":{a new sheet}}]}\n\
Every index is zero-based and refers to the print as it is now, before your changes. \
A replacement carries the complete sheet: html, css, and notes, changed or not. \
\"sheet\": null deletes that sheet. \"insert\": true puts the new sheet before that index; \
an index equal to the sheet count appends. Omit title, theme, and untouched sheets.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<PrintPatch, String> {
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

/// Applies `patch` to a copy of `print`. Fails when an index is out
/// of range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(print: &Print, patch: PrintPatch) -> Result<Print, String> {
    let mut result = print.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = print.sheets.len();
    let mut replacements: Vec<Option<Option<Sheet>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Sheet>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.sheets {
        if operation.insert {
            let Some(sheet) = operation.sheet else {
                return Err(format!(
                    "sheets[{}] has insert true but no sheet: include the new sheet",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "sheets[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(sheet);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "sheets[{}] does not exist: the print has {count} sheets, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.sheet);
        }
    }
    let mut sheets = Vec::with_capacity(count + 1);
    for index in 0..=count {
        sheets.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => sheets.push(replacement),
                None => sheets.push(print.sheets[index].clone()),
            }
        }
    }
    result.sheets = sheets;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn print() -> Print {
        serde_json::from_str(include_str!("../../../fixtures/sample-print.json")).unwrap()
    }

    fn note_of(sheet: &Sheet) -> &str {
        sheet.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = print();
        let mut replacement = original.sheets[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.sheets[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"sheets\":[{{\"index\":0,\"sheet\":{}}},{{\"index\":1,\"sheet\":null}},{{\"index\":2,\"insert\":true,\"sheet\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.sheets.len(), 2);
        assert_eq!(note_of(&patched.sheets[0]), "replaced");
        assert_eq!(note_of(&patched.sheets[1]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = print();
        let patch = parse_patch("{\"sheets\":[{\"index\":7,\"sheet\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"sheets\":[{\"index\":9,\"insert\":true,\"sheet\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_print() {
        let original = print();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_sheets() {
        assert!(PATCH_FORMAT.contains("\"sheets\""));
        assert!(!PATCH_FORMAT.contains("screen"));
        assert!(!PATCH_FORMAT.contains("slide"));
    }
}
