//! Document patches: the model's reply to a document edit request.
//!
//! The document twin of `deck_patch.rs`. A chat edit rarely touches
//! more than a page or two, so the model replies with only what changed
//! instead of the whole document: an optional title, an optional theme,
//! and page operations keyed by the page index in the document as it
//! was before the edit. The server applies the patch and validates the
//! result. The wording differs from `deck_patch.rs` (pages, not slides)
//! on purpose: the model sees one vocabulary per artifact kind.

use design_model::{Document, Page, Theme};
use serde::Deserialize;

/// One page operation in a patch.
#[derive(Debug, Deserialize)]
pub struct PagePatch {
    /// Zero-based index into the document before the edit. For an
    /// insert, the new page goes before this index; `index == page
    /// count` appends.
    pub index: usize,
    /// True to insert a new page at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The page. `null` with `insert: false` deletes the page.
    #[serde(default)]
    pub page: Option<Page>,
}

/// What the model sends back for a document edit.
#[derive(Debug, Default, Deserialize)]
pub struct DocumentPatch {
    /// New document title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Page operations. Indexes refer to the document before the edit.
    #[serde(default)]
    pub pages: Vec<PagePatch>,
}

/// The document patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole document:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"pages\":[{\"index\":2,\"page\":{the full replacement page}},\
{\"index\":4,\"page\":null},\
{\"index\":5,\"insert\":true,\"page\":{a new page}}]}\n\
Every index is zero-based and refers to the document as it is now, before your changes. \
A replacement carries the complete page: html, css, and notes, changed or not. \
\"page\": null deletes that page. \"insert\": true puts the new page before that index; \
an index equal to the page count appends. Omit title, theme, and untouched pages.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<DocumentPatch, String> {
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

/// Applies `patch` to a copy of `document`. Fails when an index is out
/// of range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(document: &Document, patch: DocumentPatch) -> Result<Document, String> {
    let mut result = document.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = document.pages.len();
    let mut replacements: Vec<Option<Option<Page>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Page>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.pages {
        if operation.insert {
            let Some(page) = operation.page else {
                return Err(format!(
                    "pages[{}] has insert true but no page: include the new page",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "pages[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(page);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "pages[{}] does not exist: the document has {count} pages, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.page);
        }
    }
    let mut pages = Vec::with_capacity(count + 1);
    for index in 0..=count {
        pages.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => pages.push(replacement),
                None => pages.push(document.pages[index].clone()),
            }
        }
    }
    result.pages = pages;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn document() -> Document {
        serde_json::from_str(include_str!("../../../fixtures/sample-document.json")).unwrap()
    }

    fn note_of(page: &Page) -> &str {
        page.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = document();
        let mut replacement = original.pages[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.pages[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"pages\":[{{\"index\":0,\"page\":{}}},{{\"index\":1,\"page\":null}},{{\"index\":3,\"insert\":true,\"page\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.pages.len(), 3);
        assert_eq!(note_of(&patched.pages[0]), "replaced");
        assert_eq!(note_of(&patched.pages[1]), note_of(&original.pages[2]));
        assert_eq!(note_of(&patched.pages[2]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = document();
        let patch = parse_patch("{\"pages\":[{\"index\":7,\"page\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"pages\":[{\"index\":9,\"insert\":true,\"page\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_document() {
        let original = document();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_pages() {
        assert!(PATCH_FORMAT.contains("\"pages\""));
        assert!(!PATCH_FORMAT.contains("screen"));
        assert!(!PATCH_FORMAT.contains("slide"));
    }
}
