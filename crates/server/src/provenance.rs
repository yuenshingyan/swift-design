//! Field-level authorship: which design or deck fields the user changed.
//!
//! Artifacts start agent-authored. When the editor saves with the
//! `x-swift-design-author: user` header, the server diffs the incoming
//! artifact against the stored one and records every changed field path.
//! An agent save clears the marks on paths it overwrites. The editor
//! shows the result as `agent` / `you` badges.
//!
//! Paths are slash-separated: `title`, `theme/colors/background`,
//! `screens/0/html`, `slides/0/html`. The functions work on any
//! serializable value, so the design and the deck store share them.

use std::collections::BTreeMap;

use serde::Serialize;

/// Header the editor sends so its saves count as user edits.
pub const AUTHOR_HEADER: &str = "x-swift-design-author";

/// Collects every leaf field path of `value` with its value.
fn leaf_values(
    value: &serde_json::Value,
    prefix: &str,
    out: &mut BTreeMap<String, serde_json::Value>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}/{key}")
                };
                leaf_values(child, &path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                leaf_values(child, &format!("{prefix}/{index}"), out);
            }
        }
        leaf => {
            out.insert(prefix.to_owned(), leaf.clone());
        }
    }
}

/// Every leaf field path of an artifact.
pub fn field_paths<T: Serialize>(artifact: &T) -> anyhow::Result<Vec<String>> {
    let value = serde_json::to_value(artifact)?;
    let mut leaves = BTreeMap::new();
    leaf_values(&value, "", &mut leaves);
    Ok(leaves.into_keys().collect())
}

/// Paths whose value differs between `old` and `new`, plus paths that
/// exist only in `old` (removed fields, whose marks must be dropped).
pub fn diff_paths<T: Serialize>(old: &T, new: &T) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let old_value = serde_json::to_value(old)?;
    let new_value = serde_json::to_value(new)?;
    let mut old_leaves = BTreeMap::new();
    let mut new_leaves = BTreeMap::new();
    leaf_values(&old_value, "", &mut old_leaves);
    leaf_values(&new_value, "", &mut new_leaves);
    let changed = new_leaves
        .iter()
        .filter(|(path, value)| old_leaves.get(*path) != Some(value))
        .map(|(path, _)| path.clone())
        .collect();
    let removed = old_leaves
        .keys()
        .filter(|path| !new_leaves.contains_key(*path))
        .cloned()
        .collect();
    Ok((changed, removed))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use design_model::{Deck, Design};

    use crate::provenance::{diff_paths, field_paths};

    fn sample_design() -> Design {
        serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap()
    }

    fn sample_deck() -> Deck {
        serde_json::from_str(include_str!("../../../fixtures/sample-deck.json")).unwrap()
    }

    #[test]
    fn field_paths_cover_theme_and_screens() {
        let paths = field_paths(&sample_design()).unwrap();
        assert!(paths.contains(&"title".to_owned()));
        assert!(paths.contains(&"theme/colors/background".to_owned()));
        assert!(paths.contains(&"screens/0/html".to_owned()));
        assert!(paths.contains(&"screens/0/css".to_owned()));
        assert!(paths.contains(&"screens/0/notes".to_owned()));
    }

    #[test]
    fn diff_reports_changed_and_removed_paths() {
        let old = sample_design();
        let mut new = old.clone();
        new.title = "Changed".to_owned();
        new.screens.pop();
        let (changed, removed) = diff_paths(&old, &new).unwrap();
        assert_eq!(changed, ["title"]);
        assert!(removed.iter().all(|path| path.starts_with("screens/2/")));
        assert!(!removed.is_empty());
    }

    #[test]
    fn identical_designs_have_no_diff() {
        let design = sample_design();
        let (changed, removed) = diff_paths(&design, &design).unwrap();
        assert!(changed.is_empty());
        assert!(removed.is_empty());
    }

    #[test]
    fn diff_paths_work_for_a_deck() {
        let old = sample_deck();
        let paths = field_paths(&old).unwrap();
        assert!(paths.contains(&"slides/0/html".to_owned()));
        assert!(!paths.iter().any(|path| path.starts_with("screens/")));
        let mut new = old.clone();
        new.slides[1].html = "<p>Changed</p>".to_owned();
        let (changed, removed) = diff_paths(&old, &new).unwrap();
        assert_eq!(changed, ["slides/1/html"]);
        assert!(removed.is_empty());
    }
}
