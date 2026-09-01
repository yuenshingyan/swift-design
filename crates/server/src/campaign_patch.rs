//! Campaign patches: the model's reply to a campaign edit request.
//!
//! The campaign twin of `deck_patch.rs`. A chat edit rarely touches
//! more than an ad or two, so the model replies with only what changed
//! instead of the whole campaign: an optional title, an optional theme,
//! and ad operations keyed by the ad index in the campaign as it
//! was before the edit. The server applies the patch and validates the
//! result. The wording differs from `deck_patch.rs` (ads, not slides)
//! on purpose: the model sees one vocabulary per artifact kind.

use design_model::{Ad, Campaign, Theme};
use serde::Deserialize;

/// One ad operation in a patch.
#[derive(Debug, Deserialize)]
pub struct AdPatch {
    /// Zero-based index into the campaign before the edit. For an
    /// insert, the new ad goes before this index; `index == ad
    /// count` appends.
    pub index: usize,
    /// True to insert a new ad at `index` instead of replacing.
    #[serde(default)]
    pub insert: bool,
    /// The ad. `null` with `insert: false` deletes the ad.
    #[serde(default)]
    pub ad: Option<Ad>,
}

/// What the model sends back for a campaign edit.
#[derive(Debug, Default, Deserialize)]
pub struct CampaignPatch {
    /// New campaign title, when it changes.
    #[serde(default)]
    pub title: Option<String>,
    /// New theme, when it changes.
    #[serde(default)]
    pub theme: Option<Theme>,
    /// Ad operations. Indexes refer to the campaign before the edit.
    #[serde(default)]
    pub ads: Vec<AdPatch>,
}

/// The campaign patch format, stated for the model.
pub const PATCH_FORMAT: &str = "\
Reply with only a JSON patch, not the whole campaign:\n\
{\"title\":\"only if it changes\",\"theme\":{only if it changes},\
\"ads\":[{\"index\":2,\"ad\":{the full replacement ad}},\
{\"index\":4,\"ad\":null},\
{\"index\":5,\"insert\":true,\"ad\":{a new ad}}]}\n\
Every index is zero-based and refers to the campaign as it is now, before your changes. \
A replacement carries the complete ad: html, css, and notes, changed or not. \
\"ad\": null deletes that ad. \"insert\": true puts the new ad before that index; \
an index equal to the ad count appends. Omit title, theme, and untouched ads.";

/// Extracts and parses the patch JSON from a model reply.
pub fn parse_patch(content: &str) -> Result<CampaignPatch, String> {
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

/// Applies `patch` to a copy of `campaign`. Fails when an index is out
/// of range, so the model gets a clear message instead of a silent drop.
pub fn apply_patch(campaign: &Campaign, patch: CampaignPatch) -> Result<Campaign, String> {
    let mut result = campaign.clone();
    if let Some(title) = patch.title {
        result.title = title;
    }
    if let Some(theme) = patch.theme {
        result.theme = theme;
    }
    let count = campaign.ads.len();
    let mut replacements: Vec<Option<Option<Ad>>> = (0..count).map(|_| None).collect();
    let mut inserts: Vec<Vec<Ad>> = (0..=count).map(|_| Vec::new()).collect();
    for operation in patch.ads {
        if operation.insert {
            let Some(ad) = operation.ad else {
                return Err(format!(
                    "ads[{}] has insert true but no ad: include the new ad",
                    operation.index
                ));
            };
            if operation.index > count {
                return Err(format!(
                    "ads[{}] insert index is past the end: use 0 to {count}",
                    operation.index
                ));
            }
            inserts[operation.index].push(ad);
        } else {
            if operation.index >= count {
                return Err(format!(
                    "ads[{}] does not exist: the campaign has {count} ads, use 0 to {}",
                    operation.index,
                    count.saturating_sub(1)
                ));
            }
            replacements[operation.index] = Some(operation.ad);
        }
    }
    let mut ads = Vec::with_capacity(count + 1);
    for index in 0..=count {
        ads.append(&mut inserts[index]);
        if index < count {
            match replacements[index].take() {
                // Deleted.
                Some(None) => {}
                Some(Some(replacement)) => ads.push(replacement),
                None => ads.push(campaign.ads[index].clone()),
            }
        }
    }
    result.ads = ads;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn campaign() -> Campaign {
        serde_json::from_str(include_str!("../../../fixtures/sample-campaign.json")).unwrap()
    }

    fn note_of(ad: &Ad) -> &str {
        ad.notes.as_deref().unwrap_or("")
    }

    #[test]
    fn replace_delete_and_insert_use_original_indexes() {
        let original = campaign();
        let mut replacement = original.ads[0].clone();
        replacement.notes = Some("replaced".to_owned());
        let mut inserted = original.ads[1].clone();
        inserted.notes = Some("inserted".to_owned());
        let patch = parse_patch(&format!(
            "{{\"title\":\"New\",\"ads\":[{{\"index\":0,\"ad\":{}}},{{\"index\":1,\"ad\":null}},{{\"index\":2,\"insert\":true,\"ad\":{}}}]}}",
            serde_json::to_string(&replacement).unwrap(),
            serde_json::to_string(&inserted).unwrap()
        ))
        .unwrap();
        let patched = apply_patch(&original, patch).unwrap();
        assert_eq!(patched.title, "New");
        assert_eq!(patched.ads.len(), 2);
        assert_eq!(note_of(&patched.ads[0]), "replaced");
        assert_eq!(note_of(&patched.ads[1]), "inserted");
        assert_eq!(patched.validate(), Vec::new());
    }

    #[test]
    fn out_of_range_indexes_are_errors() {
        let original = campaign();
        let patch = parse_patch("{\"ads\":[{\"index\":7,\"ad\":null}]}").unwrap();
        let error = apply_patch(&original, patch).unwrap_err();
        assert!(error.contains("does not exist"));
        let patch = parse_patch("{\"ads\":[{\"index\":9,\"insert\":true,\"ad\":null}]}").unwrap();
        assert!(apply_patch(&original, patch).is_err());
    }

    #[test]
    fn an_empty_patch_keeps_the_campaign() {
        let original = campaign();
        let patched = apply_patch(&original, parse_patch("Sure: {}").unwrap()).unwrap();
        assert_eq!(patched, original);
        assert!(parse_patch("no json").is_err());
    }

    #[test]
    fn the_format_speaks_of_ads() {
        assert!(PATCH_FORMAT.contains("\"ads\""));
        assert!(!PATCH_FORMAT.contains("screen"));
        assert!(!PATCH_FORMAT.contains("slide"));
    }
}
