//! The polish pass: a design review of each candidate after it
//! validates.
//!
//! Validation checks that a design is safe and well formed. Polish checks
//! that it looks right. Demo completion requires a complete Chrome audit.
//! Hard layout errors go through bounded tweaking rounds and block completion
//! if they remain. The exact chosen version is measured again before saving.
//! Cosmetic warnings remain best-effort. Rendered screenshots accompany the
//! generation review when the model supports images. The audit script itself
//! is `render::AUDIT_SCRIPT`. Other artifact pipelines share the script and
//! finding vocabulary but retain their existing completion policies.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use design_model::Design;
use design_model::markup::layout_declarations;
use serde::Deserialize;

use crate::patch::{PATCH_FORMAT, apply_patch, parse_patch};
use crate::screenshots::{dump_design_dom, find_chrome};

/// One finding from the in-browser audit script.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Finding {
    /// Zero-based screen or slide index.
    pub(crate) screen: usize,
    /// The node, like `h2.title (0/1)`, or `root`.
    #[serde(default)]
    pub(crate) node: String,
    /// `overflow`, `off_screen`, `overfull`, `tiny_text`, `overlap`,
    /// `empty`, `contrast`, `long_lines`, `static_control`, `alignment`, or `window_geometry`.
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

/// Reads the audit report out of a dumped DOM: the JSON in the
/// `data-swift-design-findings` attribute on `<html>`. The findings
/// come ordered by severity and capped per kind, see `prioritized`.
#[cfg(test)]
pub fn parse_findings(dom: &str) -> Vec<String> {
    prioritized(raw_findings(dom))
        .iter()
        .map(format_finding)
        .collect()
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
fn measured_findings(dom: &str) -> Option<Vec<Finding>> {
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

/// The browser boundary used by mandatory demo audits.
pub(crate) type DemoDomRunner = Arc<
    dyn Fn(Design, String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
        + Send
        + Sync,
>;

/// Runs the real browser without exposing browser errors or paths to session records.
pub(crate) fn chrome_dom_runner() -> DemoDomRunner {
    Arc::new(|design, base_url| {
        Box::pin(async move {
            dump_design_dom(&design, &base_url).await.map_err(|error| {
                tracing::warn!(%error, "demo layout audit failed");
                "Could not run the browser layout audit. Check Chrome and retry.".to_owned()
            })
        })
    })
}

/// A complete browser measurement of one exact design version.
pub(crate) struct DemoAudit {
    findings: Vec<Finding>,
}

impl DemoAudit {
    /// Reads a valid report and verifies that every screen was measured.
    pub(crate) fn from_dom(dom: &str, screen_count: usize) -> Result<Self, String> {
        let findings = measured_findings(dom).ok_or_else(|| {
            "The layout audit returned no valid report. Retry the run.".to_owned()
        })?;
        let marker = "data-swift-design-audited-screens=\"";
        let coverage = dom.find(marker).and_then(|start| {
            let rest = &dom[start + marker.len()..];
            let end = rest.find('"')?;
            serde_json::from_str::<Vec<usize>>(&rest[..end].replace("&quot;", "\"")).ok()
        });
        let expected: Vec<usize> = (0..screen_count).collect();
        if coverage.as_ref() != Some(&expected)
            || findings
                .iter()
                .any(|finding| finding.screen >= screen_count || !is_known_finding(&finding.kind))
        {
            return Err("The layout audit did not measure every screen. Retry the run.".to_owned());
        }
        Ok(Self { findings })
    }

    /// Returns prioritized instructions for all screens or a focused edit.
    pub(crate) fn instructions(&self, indexes: Option<&[usize]>) -> Vec<String> {
        prioritized(
            self.findings
                .iter()
                .filter(|finding| indexes.is_none_or(|indexes| indexes.contains(&finding.screen)))
                .cloned()
                .collect(),
        )
        .iter()
        .map(format_finding)
        .collect()
    }

    /// Returns only layout blockers for repair rounds after a focused user edit.
    pub(crate) fn layout_instructions(&self, indexes: &[usize]) -> Vec<String> {
        prioritized(
            self.findings
                .iter()
                .filter(|finding| indexes.contains(&finding.screen) && is_blocking(&finding.kind))
                .cloned()
                .collect(),
        )
        .iter()
        .map(format_finding)
        .collect()
    }

    /// Scores hard layout errors before optional visual improvements.
    pub(crate) fn score(&self, indexes: Option<&[usize]>) -> (usize, usize) {
        let findings: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|finding| indexes.is_none_or(|indexes| indexes.contains(&finding.screen)))
            .collect();
        (
            findings
                .iter()
                .filter(|finding| is_blocking(&finding.kind))
                .count(),
            findings.len(),
        )
    }

    /// Blocks completion while any measured layout error remains on any screen.
    pub(crate) fn require_layout_clean(&self) -> Result<(), String> {
        self.require_layout_clean_on(None)
    }

    /// Verifies the touched screens without claiming that untouched legacy screens are clean.
    pub(crate) fn require_layout_clean_on(&self, indexes: Option<&[usize]>) -> Result<(), String> {
        let blockers: Vec<String> = self
            .findings
            .iter()
            .filter(|finding| {
                is_blocking(&finding.kind)
                    && indexes.is_none_or(|indexes| indexes.contains(&finding.screen))
            })
            .map(|finding| {
                format!(
                    "screen {} {} at {}: {}",
                    finding.screen + 1,
                    finding.kind,
                    finding.node,
                    finding.detail
                )
            })
            .take(8)
            .collect();
        if blockers.is_empty() {
            return Ok(());
        }
        Err(format!(
            "Layout verification failed after tweaking. No verified final version was saved. {}",
            blockers.join("; ")
        ))
    }
}

/// Applies a layout-only tweak without inserting, deleting, or touching unrequested screens.
pub(crate) fn apply_tweak(
    design: &Design,
    content: &str,
    indexes: Option<&[usize]>,
) -> Result<Design, String> {
    let patch = parse_patch(content)?;
    if patch.screens.iter().any(|operation| {
        operation.insert
            || operation.screen.is_none()
            || indexes.is_some_and(|indexes| !indexes.contains(&operation.index))
    }) {
        return Err("A layout tweak must replace only the requested screens. Do not insert or delete screens.".to_owned());
    }
    if indexes.is_some_and(|indexes| indexes.len() < design.screens.len())
        && patch
            .theme
            .as_ref()
            .is_some_and(|theme| theme != &design.theme)
    {
        return Err("A focused layout tweak must not change the shared theme.".to_owned());
    }
    let improved = apply_patch(design, patch)?;
    if !improved.validate().is_empty() || !respects_layout_contracts(design, &improved) {
        return Err("The tweak failed validation or changed a layout contract.".to_owned());
    }
    Ok(improved)
}

/// Prevents a tweak from hiding a failure by deleting its layout contract.
pub(crate) fn respects_layout_contracts(before: &Design, after: &Design) -> bool {
    if before.viewport != after.viewport
        || before.app_window != after.app_window
        || before.screens.len() != after.screens.len()
    {
        return false;
    }
    before
        .screens
        .iter()
        .zip(&after.screens)
        .all(|(before, after)| {
            let mut remaining = layout_declarations(&after.html);
            layout_declarations(&before.html)
                .into_iter()
                .all(|declaration| {
                    let Some(index) = remaining
                        .iter()
                        .position(|candidate| candidate == &declaration)
                    else {
                        return false;
                    };
                    remaining.remove(index);
                    true
                })
        })
}

fn is_known_finding(kind: &str) -> bool {
    is_blocking(kind)
        || matches!(
            kind,
            "tiny_text" | "contrast" | "long_lines" | "static_control"
        )
}

fn is_blocking(kind: &str) -> bool {
    matches!(
        kind,
        "window_geometry"
            | "overflow"
            | "off_screen"
            | "overfull"
            | "overlap"
            | "alignment"
            | "empty"
    )
}

/// The fix advice for one finding kind.
pub(crate) fn finding_advice(kind: &str) -> &'static str {
    match kind {
        "window_geometry" => {
            "preserve app_window and its top-level shell marker; reduce overflowing content so every screen renders the shared bounds without shrinking"
        }
        "alignment" => {
            "align the declared group edges or dimensions; use equal gaps on the declared axis; preserve intentional unmarked offsets"
        }
        "overflow" => "shorten the text, enlarge the box, or reduce the font size",
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

/// One finding as a fix instruction for the model.
fn format_finding(finding: &Finding) -> String {
    format!(
        "screens[{}] {}: {}: {}",
        finding.screen,
        finding.node,
        finding.detail,
        finding_advice(&finding.kind)
    )
}

/// The review request for one candidate: the design, the findings, and
/// the design checklist. `image_count` screen images follow the prompt
/// when the model can see.
pub fn polish_prompt(design_json: &str, findings: &[String], image_count: usize) -> String {
    let mut prompt = format!(
        "Review this candidate design as a product designer and improve it.\n\
         Design JSON:\n{design_json}\n"
    );
    if image_count > 0 {
        prompt.push_str(&format!(
            "The next {image_count} images show the rendered screens in order, one per screen. \
             Look at them. Fix what looks wrong, not only what the JSON says.\n"
        ));
    }
    if findings.is_empty() {
        prompt.push_str("Automatic layout audit: no problems found.\n");
    } else {
        prompt.push_str("Automatic layout audit found these problems. Fix every one:\n");
        for finding in findings {
            prompt.push_str("- ");
            prompt.push_str(finding);
            prompt.push('\n');
        }
    }
    prompt.push_str(
        "Then check and improve: text that overflows its box or the screen, text blocks that \
         overlap or touch, uneven margins, misaligned edges, inconsistent sizes for the same \
         role across screens, low contrast between text and background, screens that are too \
         dense or too empty, and a weak title screen. Keep the concept, the outline, and the \
         content. Preserve app_window and all shell markers. Keep the shared window bounds, \
         window chrome, header height, and sidebar width. Fix overflowing content instead of \
         resizing one window. Preserve data-layout-align and data-layout-gap declarations. Completion is blocked by unresolved overflow, off-screen content, overlap, shrinking, or declared alignment errors. Your reply is rendered and measured again. Do not add or delete screens. Change only the screens that need it.\n",
    );
    prompt.push_str(PATCH_FORMAT);
    prompt.push_str(" When nothing needs a change, reply with {}.");
    prompt
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    use crate::render::AUDIT_SCRIPT;

    #[test]
    fn mandatory_audits_reject_missing_malformed_or_incomplete_reports() {
        for dom in [
            "<html>",
            "<html data-swift-design-findings=\"bad\">",
            "<html data-swift-design-findings=\"[]\">",
            "<html data-swift-design-findings=\"[]\" data-swift-design-audited-screens=\"[0]\">",
        ] {
            assert!(DemoAudit::from_dom(dom, 2).is_err());
        }
        assert!(DemoAudit::from_dom("<html data-swift-design-findings=\"[]\" data-swift-design-audited-screens=\"[0,1]\">", 2).is_ok());
    }

    #[test]
    fn incomplete_or_unknown_finding_records_are_not_a_successful_audit() {
        for finding in [
            r#"{"screen":2,"kind":"overflow"}"#,
            r#"{"screen":0,"kind":"unknown_error"}"#,
            r#"{"screen":0}"#,
        ] {
            let dom = format!(
                "<html data-swift-design-findings=\"[{}]\" data-swift-design-audited-screens=\"[0]\">",
                finding.replace('"', "&quot;")
            );
            assert!(DemoAudit::from_dom(&dom, 1).is_err());
        }
    }

    #[test]
    fn hard_layout_errors_cannot_be_saved_as_best_effort() {
        for kind in [
            "window_geometry",
            "overflow",
            "off_screen",
            "overfull",
            "overlap",
            "alignment",
            "empty",
        ] {
            let dom = format!(
                "<html data-swift-design-findings=\"[{{&quot;screen&quot;:0,&quot;kind&quot;:&quot;{kind}&quot;}}]\" data-swift-design-audited-screens=\"[0]\">"
            );
            let audit = DemoAudit::from_dom(&dom, 1).unwrap();
            assert!(audit.require_layout_clean().is_err());
            assert_eq!(audit.score(None), (1, 1));
            assert!(audit.instructions(None).len() == 1);
            assert!(audit.instructions(Some(&[1])).is_empty());
        }
        assert!(finding_advice("window_geometry").contains("preserve app_window"));
    }

    #[test]
    fn tweaks_cannot_delete_insert_or_replace_untouched_screens() {
        let design: Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        assert!(apply_tweak(&design, r#"{"screens":[{"index":0,"screen":null}]}"#, None).is_err());
        let screen = serde_json::to_value(&design.screens[0]).unwrap();
        let insertion =
            serde_json::json!({"screens":[{"index":0,"insert":true,"screen":screen}]}).to_string();
        assert!(apply_tweak(&design, &insertion, None).is_err());
        let replacement = serde_json::json!({"screens":[{"index":1,"screen":screen}]}).to_string();
        assert!(apply_tweak(&design, &replacement, Some(&[0])).is_err());
        assert!(apply_tweak(&design, &replacement, Some(&[1])).is_ok());
    }

    #[test]
    fn tweaks_cannot_remove_alignment_contracts_or_resize_the_canvas() {
        let mut before: Design =
            serde_json::from_str(include_str!("../../../fixtures/sample-design.json")).unwrap();
        before.screens[0].html =
            "<div data-layout-align='left'><p>One</p><p>Two</p></div>".to_owned();
        let mut after = before.clone();
        assert!(respects_layout_contracts(&before, &after));
        after.screens[0].html = "<div><p>One</p><p>Two</p></div>".to_owned();
        assert!(!respects_layout_contracts(&before, &after));
        after = before.clone();
        after.viewport.width += 100;
        assert!(!respects_layout_contracts(&before, &after));
    }

    #[test]
    fn focused_verification_does_not_claim_untouched_legacy_errors_are_clean() {
        let audit = DemoAudit {
            findings: vec![Finding {
                screen: 1,
                node: String::new(),
                kind: "overflow".to_owned(),
                detail: "legacy clipping".to_owned(),
            }],
        };
        assert!(audit.require_layout_clean_on(Some(&[0])).is_ok());
        assert!(audit.require_layout_clean_on(Some(&[1])).is_err());
        assert!(audit.require_layout_clean().is_err());
        assert_eq!(audit.score(Some(&[0])), (0, 0));
        assert!(audit.layout_instructions(&[0]).is_empty());
        assert_eq!(audit.layout_instructions(&[1]).len(), 1);
    }

    #[test]
    fn optional_visual_findings_do_not_block_layout_verification() {
        let audit = DemoAudit {
            findings: vec![Finding {
                screen: 0,
                node: String::new(),
                kind: "contrast".to_owned(),
                detail: "low contrast".to_owned(),
            }],
        };
        assert!(audit.require_layout_clean().is_ok());
        assert_eq!(audit.score(None), (0, 1));
        assert!(audit.layout_instructions(&[0]).is_empty());
    }

    #[test]
    fn findings_are_read_from_the_dumped_dom() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:2,&quot;node&quot;:&quot;h2.title (0/1)&quot;,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;content needs 410px but the box is 376px tall&quot;}]\"><head>";
        let findings = parse_findings(dom);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0],
            "screens[2] h2.title (0/1): content needs 410px but the box is 376px tall: shorten the text, enlarge the box, or reduce the font size"
        );
        assert!(parse_findings("<html><head></head></html>").is_empty());
        assert!(parse_findings("<html data-swift-design-findings=\"not json\">").is_empty());
    }

    #[test]
    fn findings_come_layout_first_and_capped_per_kind() {
        let mut raw: Vec<Finding> = (0..10)
            .map(|index| Finding {
                screen: 0,
                node: format!("p ({index})"),
                kind: "tiny_text".to_owned(),
                detail: "font-size 11px is too small".to_owned(),
            })
            .collect();
        raw.push(Finding {
            screen: 1,
            node: "h2 (0/1)".to_owned(),
            kind: "overflow".to_owned(),
            detail: "content needs 59px but the box is 30px tall".to_owned(),
        });
        raw.push(Finding {
            screen: 0,
            node: "p (0/2)".to_owned(),
            kind: "contrast".to_owned(),
            detail: "contrast 2.1:1".to_owned(),
        });
        let ordered = prioritized(raw);
        let kinds: Vec<&str> = ordered
            .iter()
            .map(|finding| finding.kind.as_str())
            .collect();
        // The overflow leads, then the contrast, then six small-text
        // findings and one line for the other four.
        assert_eq!(kinds[0], "overflow");
        assert_eq!(kinds[1], "contrast");
        assert_eq!(kinds.iter().filter(|kind| **kind == "tiny_text").count(), 7);
        let summary = ordered.last().expect("a summary line");
        assert_eq!(summary.node, "and more");
        assert_eq!(
            summary.detail,
            "4 more tiny_text finding(s) on this screen, not listed"
        );
        assert_eq!(ordered.len(), 9);
    }

    #[test]
    fn a_static_control_is_a_layout_finding_with_its_own_advice() {
        assert!(kind_rank("static_control") < kind_rank("contrast"));
        assert!(kind_rank("overlap") < kind_rank("static_control"));
        assert!(finding_advice("static_control").contains("href='#screen-N'"));
        assert!(AUDIT_SCRIPT.contains("kind: 'static_control'"));
        assert!(AUDIT_SCRIPT.contains("controlPattern"));
    }

    #[test]
    fn the_audit_script_sets_the_text_floor_by_canvas() {
        let script = AUDIT_SCRIPT;
        assert!(script.contains("canvasWidth >= 1920 ? { flag: 20, ask: 24 }"));
        assert!(script.contains("canvasWidth >= 1000 ? { flag: 12, ask: 14 }"));
        assert!(script.contains("{ flag: 11, ask: 12 }"));
        assert!(script.contains("if (size < textFloor.flag)"));
    }

    #[test]
    fn the_audit_script_holds_the_contrast_check() {
        let script = AUDIT_SCRIPT;
        assert!(script.contains("const contrastRatio = (a, b) =>"));
        assert!(script.contains("const luminance = (rgb) =>"));
        assert!(script.contains("(Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)"));
        assert!(script.contains("kind: 'contrast'"));
        assert!(script.contains("size >= 24 || (weight >= 700 && size >= 18.66) ? 3.0 : 4.5"));
    }

    #[test]
    fn the_audit_script_holds_the_line_length_check() {
        let script = AUDIT_SCRIPT;
        assert!(script.contains("kind: 'long_lines'"));
        assert!(script.contains("rect.height / scale / lineHeight"));
        assert!(script.contains("if (perLine > 100)"));
    }

    #[test]
    fn contrast_findings_are_parsed_from_the_dom() {
        let dom = "<html data-swift-design-findings=\"[{&quot;screen&quot;:1,&quot;node&quot;:&quot;p.s2-caption (0/1)&quot;,&quot;kind&quot;:&quot;contrast&quot;,&quot;detail&quot;:&quot;contrast 2.31:1 is below the 4.5:1 limit for rgb(138, 148, 166) text on rgb(16, 20, 24)&quot;}]\">";
        let findings = parse_findings(dom);
        assert_eq!(
            findings,
            [
                "screens[1] p.s2-caption (0/1): contrast 2.31:1 is below the 4.5:1 limit for rgb(138, 148, 166) text on rgb(16, 20, 24): use a darker or lighter text color, or change the background"
            ]
        );
    }

    #[test]
    fn line_length_findings_are_parsed_from_the_dom() {
        let dom = "<html data-swift-design-findings=\"[{&quot;screen&quot;:0,&quot;node&quot;:&quot;p (0/2)&quot;,&quot;kind&quot;:&quot;long_lines&quot;,&quot;detail&quot;:&quot;about 132 characters per line over 2 lines: keep lines under 100 characters&quot;}]\">";
        let findings = parse_findings(dom);
        assert_eq!(
            findings,
            [
                "screens[0] p (0/2): about 132 characters per line over 2 lines: keep lines under 100 characters: narrow the box, split the text, or use a larger font size"
            ]
        );
    }

    #[test]
    fn the_formatter_renders_both_new_kinds() {
        let contrast = format_finding(&Finding {
            screen: 3,
            node: "h2 (0)".to_owned(),
            kind: "contrast".to_owned(),
            detail: "contrast 1.50:1 is below the 3.0:1 limit".to_owned(),
        });
        assert!(contrast.starts_with("screens[3] h2 (0): contrast 1.50:1"));
        assert!(contrast.ends_with("change the background"));
        let lines = format_finding(&Finding {
            screen: 4,
            node: "p (1)".to_owned(),
            kind: "long_lines".to_owned(),
            detail: "about 120 characters per line over 1 line".to_owned(),
        });
        assert!(lines.starts_with("screens[4] p (1): about 120 characters per line"));
        assert!(lines.ends_with("use a larger font size"));
    }

    #[test]
    fn each_stop_reason_says_what_state_the_page_is_in() {
        assert_eq!(
            PolishStop::Clean.describe(2, 0),
            "measures clean after 2 polish round(s)"
        );
        // A page left with findings must say so, not read as finished.
        let stalled = PolishStop::NoImprovement.describe(3, 4);
        assert!(stalled.contains("4 finding(s) left"));
        assert!(stalled.contains("fixed none"));
        let spent = PolishStop::OutOfRounds.describe(5, 2);
        assert!(spent.contains("all 5 round(s)"));
        assert!(spent.contains("2 finding(s) left"));
    }

    #[test]
    fn prompts_carry_findings_and_the_checklist() {
        let prompt = polish_prompt("{}", &["screens[0] x".to_owned()], 0);
        assert!(prompt.contains("- screens[0] x"));
        assert!(prompt.contains("Keep the concept"));
        assert!(prompt.contains("JSON patch"));
        assert!(prompt.contains("reply with {}"));
        assert!(!prompt.contains("images show"));
        assert!(polish_prompt("{}", &[], 3).contains("no problems found"));
        assert!(polish_prompt("{}", &[], 3).contains("next 3 images"));
        assert_eq!(polish_round_limit("low"), 1);
        assert_eq!(polish_round_limit("medium"), 3);
        assert_eq!(polish_round_limit("high"), 5);
        // A ceiling, not a target: the loop exits as soon as the page
        // measures clean, so these are rarely all spent.
        assert!(polish_round_limit("low") < polish_round_limit("high"));
    }
}
