//! Mailing patches: the model's reply to a mailing edit request.
//!
//! The mailing twin of `deck_patch.rs`. A chat edit rarely touches
//! more than an email or two, so the model replies with only what changed
//! instead of the whole mailing: an optional title, an optional theme,
//! and email operations keyed by the email index in the mailing as it
//! was before the edit. The server applies the patch and validates the
//! result. The wording differs from `deck_patch.rs` (emails, not slides)
//! on purpose: the model sees one vocabulary per artifact kind.

use design_model::{Email, Mailing, Theme};
use serde::Deserialize;

/// One email operation in a patch.
#[derive(Debug, Deserialize)]
pub struct EmailPatch {
    /// Zero-based index into the mailing before the edit. For an
    /// insert, the new email goes before this index; `index == email
    /// count` appends.
    pub index: usize,
    /// True to insert a new email at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The email. `null` with `insert: false` deletes the email.
    #[serde(default)]
    pub email: Option<Email>,
}

/// What the model sends back for a mailing edit.
#[derive(Debug, Default, Deserialize)]
pub struct MailingPatch {
    /// New mailing title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Email operations. Indexes refer to the mailing before the edit.
    #[serde(default)]
    pub emails: Vec<EmailPatch>,
}

/// The mailing patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole mailing:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"emails\":[{\"index\":2,\"email\":{the full replacement email}},\
{\"index\":4,\"email\":null},\
{\"index\":5,\"insert\":true,\"email\":{a new email}}]}\n\
Every index is zero-based and refers to the mailing as it is now, before your changes. \
A replacement carries the complete email: html, css, and notes, changed or not. \
\"email\": null deletes that email. \"insert\": true puts the new email before that index; \
an index equal to the email count appends. Omit title, theme, and untouched emails.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<MailingPatch, String> {
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

/// Applies `patch` to a copy of `mailing`. Fails when an index is out
/// of range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(mailing: &Mailing, patch: MailingPatch) -> Result<Mailing, String> {
    let mut result = mailing.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = mailing.emails.len();
    let mut replacements: Vec<Option<Option<Email>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Email>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.emails {
        if operation.insert {
            let Some(email) = operation.email else {
                return Err(format!(
                    "emails[{}] has insert true but no email: include the new email",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "emails[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(email);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "emails[{}] does not exist: the mailing has {count} emails, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.email);
        }
    }
    let mut emails = Vec::with_capacity(count + 1);
    for index in 0..=count {
        emails.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => emails.push(replacement),
                None => emails.push(mailing.emails[index].clone()),
            }
        }
    }
    result.emails = emails;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn mailing() -> Mailing {
        serde_json::from_str(include_str!("../../../fixtures/sample-mailing.json")).unwrap()
    }

    fn note_of(email: &Email) -> &str {
        email.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = mailing();
        let mut replacement = original.emails[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.emails[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"emails\":[{{\"index\":0,\"email\":{}}},{{\"index\":1,\"email\":null}},{{\"index\":2,\"insert\":true,\"email\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.emails.len(), 2);
        assert_eq!(note_of(&patched.emails[0]), "replaced");
        assert_eq!(note_of(&patched.emails[1]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = mailing();
        let patch = parse_patch("{\"emails\":[{\"index\":7,\"email\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch =
            parse_patch("{\"emails\":[{\"index\":9,\"insert\":true,\"email\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_mailing() {
        let original = mailing();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_emails() {
        assert!(PATCH_FORMAT.contains("\"emails\""));
        assert!(!PATCH_FORMAT.contains("screen"));
        assert!(!PATCH_FORMAT.contains("slide"));
    }
}
