//! Annotation validation and revision checks for a single demo edit.

use std::collections::BTreeMap;

use design_model::{AppWindow, Design, Screen, Theme, Viewport};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCALE: u32 = 10_000;
const STALE: &str = "A marked screen or its shared layout has changed. Review the current screen and mark it again.";

/// A position on the rendered canvas in ten-thousandths, measured from its top left.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Annotation {
    pub(crate) screen: usize,
    pub(crate) bounds: [u32; 4],
    pub(crate) comment: String,
}

/// Input from the editor, including the exact screen and layout the user marked.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnnotationSubmission {
    expected: BTreeMap<usize, Screen>,
    viewport: Viewport,
    theme: Theme,
    app_window: Option<AppWindow>,
    marks: Vec<Annotation>,
}

/// The queued edit owns its marks and a digest of the screen and layout evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AnnotationBatch {
    revision: String,
    pub(crate) marks: Vec<Annotation>,
}

impl AnnotationSubmission {
    pub(crate) fn bind(self, design: &Design) -> Result<AnnotationBatch, String> {
        let mut batch = AnnotationBatch {
            revision: String::new(),
            marks: self.marks,
        };
        let indexes = batch.indexes();
        if batch.marks.is_empty() || batch.marks.len() > 24 || indexes.len() > 6 {
            return Err("Submit 1–24 boxes on up to 6 screens at a time.".into());
        }
        for mark in &mut batch.marks {
            let [left, top, right, bottom] = mark.bounds;
            mark.comment = mark.comment.trim().to_owned();
            if left >= right
                || top >= bottom
                || right > SCALE
                || bottom > SCALE
                || mark.comment.is_empty()
                || mark.comment.len() > 2_000
            {
                return Err(
                    "Each box needs valid bounds and a comment of at most 2,000 bytes.".into(),
                );
            }
        }
        if self.viewport != design.viewport
            || self.theme != design.theme
            || self.app_window != design.app_window
            || self.expected.len() != indexes.len()
            || indexes.iter().any(|&index| {
                self.expected
                    .get(&index)
                    .is_none_or(|screen| design.screens.get(index) != Some(screen))
            })
        {
            return Err(STALE.into());
        }
        batch.revision = batch.revision_for(design)?;
        Ok(batch)
    }
}

impl AnnotationBatch {
    pub(crate) fn indexes(&self) -> Vec<usize> {
        let mut indexes: Vec<_> = self.marks.iter().map(|mark| mark.screen).collect();
        indexes.sort_unstable();
        indexes.dedup();
        indexes
    }

    fn revision_for(&self, design: &Design) -> Result<String, String> {
        let screens: Vec<_> = self
            .indexes()
            .into_iter()
            .map(|index| design.screens.get(index))
            .collect();
        let bytes =
            serde_json::to_vec(&(design.viewport, &design.theme, &design.app_window, screens))
                .map_err(|_| "Could not read the marked screen revision.".to_owned())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub(crate) fn check_revision(&self, design: &Design) -> Result<(), String> {
        if self.revision != self.revision_for(design)? {
            return Err(STALE.into());
        }
        Ok(())
    }

    pub(crate) fn requirements(&self) -> Vec<String> {
        self.marks.iter().enumerate().map(|(index, mark)| {
            let [left, top, right, bottom] = mark.bounds.map(|value| value as f64 / 100.0);
            format!("Box {} on screen {} (left {left:.2}%, top {top:.2}%, right {right:.2}%, bottom {bottom:.2}%): {}", index + 1, mark.screen + 1, mark.comment)
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn annotations_require_exact_marked_screens_and_ignore_unrelated_edits() {
        let design: Design =
            serde_json::from_str(include_str!("../../../../fixtures/sample-design.json"))
                .expect("demo fixture should parse");
        let payload = |bounds: [u32; 4], comment: &str| -> AnnotationSubmission {
            serde_json::from_value(json!({
                "expected":{"0":design.screens[0]}, "viewport":design.viewport, "theme":design.theme,
                "app_window":design.app_window, "marks":[{"screen":0,"bounds":bounds,"comment":comment}]
            })).expect("the submission should parse")
        };
        for (bounds, comment) in [
            ([0, 0, 0, 50], "adjust"),
            ([0, 1, 10_001, 20], "adjust"),
            ([0, 0, 50, 50], " "),
        ] {
            assert!(payload(bounds, comment).bind(&design).is_err());
        }
        let mut current = design.clone();
        let batch = payload([0, 0, 50, 50], "adjust")
            .bind(&design)
            .expect("the current revision should bind");
        current.screens[1].notes = Some("Unrelated change".into());
        assert!(batch.check_revision(&current).is_ok());
        current.screens[0].notes = Some("Changed since marking".into());
        assert_eq!(
            batch
                .check_revision(&current)
                .expect_err("queued marks should not apply to changed screens"),
            STALE
        );
        assert_eq!(
            payload([0, 0, 50, 50], "adjust")
                .bind(&current)
                .expect_err("stale submission should be refused"),
            STALE
        );
    }
}
