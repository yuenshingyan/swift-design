//! Shared visual-review findings, prioritization, browser checks, and effort limits.

use serde::Deserialize;

use crate::screenshots::find_chrome;

/// One finding from the in-browser audit script.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Finding {
    /// Zero-based screen or slide index.
    pub(crate) screen: usize,
    /// The node, like `h2.title (0/1)`, or `root`.
    #[serde(default)]
    pub(crate) node: String,
    /// `overflow`, `off_screen`, `overfull`, `tiny_text`, `overlap`,
    /// `empty`, `contrast`, `long_lines`, `static_control`, `alignment`,
    /// `alignment_unmeasured`, `minor_overflow`, or `window_geometry`.
    #[serde(default)]
    pub(crate) kind: String,
    /// What was measured.
    #[serde(default)]
    pub(crate) detail: String,
}

/// The most polish rounds one candidate may take, by effort level.
///
/// This is a ceiling, not a target. A round runs only while the browser
/// still measures something wrong and the last round improved it, so a
/// page that comes out clean spends none of these.
pub fn polish_round_limit(effort: &str) -> usize {
    match effort {
        "low" => 1,
        "high" => 5,
        _ => 3,
    }
}

/// Why the polish loop stopped. Reported in the run log, so a run that
/// ends with a flawed page says which of these happened.
pub enum PolishStop {
    /// The browser measured nothing wrong.
    Clean,
    /// No Chrome, so nothing was measured. The candidate is unchecked,
    /// not clean.
    NotMeasured,
    /// The last round did not reduce the findings, so another will not.
    NoImprovement,
    /// The effort's round limit ran out with findings left.
    OutOfRounds,
}

impl PolishStop {
    /// One line for the run log, naming the state the candidate is in.
    pub fn describe(&self, rounds: usize, findings: usize) -> String {
        match self {
            PolishStop::Clean => format!("measures clean after {rounds} polish round(s)"),
            PolishStop::NotMeasured => {
                "not polished: no Chrome, so the layout was never measured".to_owned()
            }
            PolishStop::NoImprovement => format!(
                "polish stopped after {rounds} round(s): {findings} finding(s) left and the last round fixed none"
            ),
            PolishStop::OutOfRounds => {
                format!("polish used all {rounds} round(s) with {findings} finding(s) left")
            }
        }
    }
}

/// True when a layout audit can run at all. Without Chrome nothing is
/// measured, and an unmeasured candidate must not be reported as clean.
pub fn can_audit() -> bool {
    find_chrome().is_some()
}

/// The most findings of one kind on one screen that reach the model.
/// The rest fold into one summary line.
pub(crate) const FINDINGS_PER_KIND_LIMIT: usize = 6;

/// The rank of a finding kind: lower comes first. Layout breakage
/// first, then legibility. A candidate with hundreds of small labels
/// once buried eight overflowing lines under the small-text findings,
/// and the model fixed none of them.
fn kind_rank(kind: &str) -> usize {
    match kind {
        "window_geometry" | "overfull" => 0,
        "off_screen" => 1,
        "overflow" => 2,
        "overlap" | "alignment" => 3,
        "static_control" => 4,
        "empty" => 5,
        "contrast" => 6,
        "long_lines" => 7,
        "tiny_text" => 8,
        _ => 9,
    }
}

/// The findings ordered by kind severity, then by screen, with at
/// most `FINDINGS_PER_KIND_LIMIT` of one kind per screen. The rest of
/// that kind on that screen become one finding that says how many
/// more there are, so the model still knows the scale of it.
pub(crate) fn prioritized(findings: Vec<Finding>) -> Vec<Finding> {
    let mut findings = findings;
    findings.sort_by_key(|finding| (kind_rank(&finding.kind), finding.screen));
    let mut kept: Vec<Finding> = Vec::new();
    let mut counts: std::collections::HashMap<(usize, String), usize> =
        std::collections::HashMap::new();
    for finding in findings {
        let count = counts
            .entry((finding.screen, finding.kind.clone()))
            .or_insert(0);
        *count += 1;
        if *count <= FINDINGS_PER_KIND_LIMIT {
            kept.push(finding);
        } else if *count == FINDINGS_PER_KIND_LIMIT + 1 {
            kept.push(Finding {
                node: "and more".to_owned(),
                detail: String::new(),
                ..finding
            });
        }
    }
    for finding in &mut kept {
        if finding.node == "and more" {
            let extra = counts
                .get(&(finding.screen, finding.kind.clone()))
                .copied()
                .unwrap_or(0)
                .saturating_sub(FINDINGS_PER_KIND_LIMIT);
            finding.detail = format!(
                "{extra} more {} finding(s) on this screen, not listed",
                finding.kind
            );
        }
    }
    kept
}

/// The audit findings in a dumped DOM, as records. Empty when the page
/// carries no report or the report does not parse.
pub(crate) fn raw_findings(dom: &str) -> Vec<Finding> {
    measured_findings(dom).unwrap_or_default()
}

/// Reads a report without treating a missing or malformed audit as clean.
pub(crate) fn measured_findings(dom: &str) -> Option<Vec<Finding>> {
    let marker = "data-swift-design-findings=\"";
    let start = dom.find(marker)?;
    let rest = &dom[start + marker.len()..];
    let end = rest.find('"')?;
    let json = rest[..end]
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    serde_json::from_str(&json).ok()
}

/// The fix advice for one finding kind.
pub(crate) fn finding_advice(kind: &str) -> &'static str {
    match kind {
        "window_geometry" => {
            "preserve app_window and its top-level shell marker; reduce overflowing content so every screen renders the shared bounds without shrinking"
        }
        "alignment" => {
            "review the measured group alignment; preserve intentional offsets and correct or remove obsolete annotations"
        }
        "alignment_unmeasured" => {
            "not a measured layout defect: review the annotation only if this group should have enough visible children; do not add filler content to satisfy it"
        }
        "overflow" | "minor_overflow" => {
            "shorten the text, enlarge the box, or reduce the font size"
        }
        "off_screen" => "move or shrink it so it stays inside the canvas",
        "tiny_text" => "use a larger font size",
        "overlap" => "move or resize one of them",
        "empty" => "add content or delete it",
        "contrast" => "use a darker or lighter text color, or change the background",
        "long_lines" => "narrow the box, split the text, or use a larger font size",
        "overfull" => "cut a section, shorten the text, or reduce the sizes until it fits",
        "static_control" => {
            "give it href='#screen-N' to the screen it opens, or make it a <label for> of a checkbox or radio input, or the <summary> of a <details>"
        }
        _ => "fix it",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::AUDIT_SCRIPT;

    #[test]
    fn a_static_control_is_a_layout_finding_with_its_own_advice() {
        assert!(kind_rank("static_control") < kind_rank("contrast"));
        assert!(kind_rank("overlap") < kind_rank("static_control"));
        assert!(finding_advice("static_control").contains("href='#screen-N'"));
        assert!(AUDIT_SCRIPT.contains("kind: 'static_control'"));
        assert!(AUDIT_SCRIPT.contains("controlPattern"));
    }
}
