//! The polish pass: a design review of each candidate after it
//! validates.
//!
//! Validation checks that a design is safe and well formed. Polish checks
//! that it looks right. Demo completion requires a complete Chrome audit.
//! Layout findings go through bounded tweaking rounds. New candidates require
//! clean layout; existing demo edits can save with warnings. The exact chosen
//! version is measured again before saving.
//! Cosmetic warnings remain best-effort. Rendered screenshots accompany the
//! generation review when the model supports images. The audit script itself
//! is `crate::render::AUDIT_SCRIPT`. Other artifact pipelines share the script and
//! finding vocabulary but retain their existing completion policies.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use design_model::Design;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::design::patch::{PATCH_FORMAT, apply_patch, parse_patch};
use crate::polish::*;
use crate::screenshots::dump_design_dom;

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
            let dom = dump_design_dom(&design, &base_url)
                .await
                .map_err(browser_audit_failure)?;
            if !dom.contains("data-swift-design-prototype-checks=\"") {
                return Err("Chrome did not return a prototype audit report.".into());
            }
            Ok(dom)
        })
    })
}

fn browser_audit_failure(error: anyhow::Error) -> String {
    tracing::warn!(%error, "demo layout audit failed");
    if error.is::<tokio::time::error::Elapsed>() {
        return "The browser layout audit timed out after one browser-only retry; fonts or other page resources may be slow. Layout verification is incomplete.".to_owned();
    }
    "Could not run the browser layout audit. Check Chrome and retry.".to_owned()
}

/// A complete browser measurement of one exact design version.
#[derive(Debug)]
pub(crate) struct DemoAudit {
    findings: Vec<Finding>,
    prototype_checks: Vec<PrototypeCheck>,
}

#[derive(Debug, Deserialize)]
struct PrototypeCheck {
    screen: usize,
    group: String,
    clicks: usize,
    keys: usize,
    focus: usize,
    failures: usize,
    issues: Vec<String>,
}

fn audit_attribute<T: DeserializeOwned>(dom: &str, marker: &str) -> Option<T> {
    let rest = dom.split_once(marker)?.1;
    let value = rest.split_once('"')?.0;
    serde_json::from_str(&value.replace("&quot;", "\"")).ok()
}

impl DemoAudit {
    /// Reads a valid report and verifies that every screen was measured.
    pub(crate) fn from_dom(dom: &str, screen_count: usize) -> Result<Self, String> {
        let findings = measured_findings(dom).ok_or_else(|| {
            "The layout audit returned no valid report. Retry the run.".to_owned()
        })?;
        let coverage = audit_attribute::<Vec<usize>>(dom, "data-swift-design-audited-screens=\"");
        let expected: Vec<usize> = (0..screen_count).collect();
        if coverage.as_ref() != Some(&expected)
            || findings
                .iter()
                .any(|finding| finding.screen >= screen_count || !is_known_finding(&finding.kind))
        {
            return Err("The layout audit did not measure every screen. Retry the run.".to_owned());
        }
        let marker = "data-swift-design-prototype-checks=\"";
        let prototype_checks: Vec<PrototypeCheck> = if dom.contains(marker) {
            audit_attribute(dom, marker)
                .ok_or("The prototype browser audit returned an invalid report.")?
        } else {
            Vec::new()
        };
        if prototype_checks
            .iter()
            .any(|check| check.screen >= screen_count || check.clicks == 0 || check.keys == 0)
        {
            return Err(
                "The prototype browser audit returned invalid screen or check counts.".into(),
            );
        }
        Ok(Self {
            findings,
            prototype_checks,
        })
    }

    /// Reports measured browser probes, including passed checks, for the run log.
    pub(crate) fn prototype_evidence(&self) -> String {
        self.prototype_checks.iter().map(|check| format!(
            "Screen {}: {} programmatic click, {} synthetic keyboard and {} programmatic focus checks. {} failures. Group: {}. {}",
            check.screen + 1, check.clicks, check.keys, check.focus, check.failures, check.group, check.issues.join("; "),
        )).collect::<Vec<_>>().join("\n")
    }

    fn prototype_failures(&self, indexes: Option<&[usize]>) -> impl Iterator<Item = String> {
        self.failed_prototype_checks(indexes)
            .map(|(_, detail)| detail)
    }

    fn failed_prototype_checks(
        &self,
        indexes: Option<&[usize]>,
    ) -> impl Iterator<Item = (usize, String)> {
        self.prototype_checks
            .iter()
            .filter(move |check| {
                check.failures != 0 && indexes.is_none_or(|indexes| indexes.contains(&check.screen))
            })
            .map(|check| {
                (
                    check.screen,
                    format!(
                        "screen {} prototype group {}: {}",
                        check.screen + 1,
                        check.group,
                        check.issues.join("; ")
                    ),
                )
            })
    }

    /// Returns prioritized instructions for all screens or a focused edit.
    pub(crate) fn instructions(&self, indexes: Option<&[usize]>) -> Vec<String> {
        self.prototype_failures(indexes)
            .chain(
                prioritized(
                    self.findings
                        .iter()
                        .filter(|finding| {
                            indexes.is_none_or(|indexes| indexes.contains(&finding.screen))
                        })
                        .cloned()
                        .collect(),
                )
                .iter()
                .map(format_finding),
            )
            .collect()
    }

    /// Summarizes uncertain findings in screen order without exposing model repair instructions.
    pub(crate) fn edit_warnings(&self, design: &Design, indexes: &[usize]) -> Vec<String> {
        let mut by_screen = BTreeMap::<usize, BTreeSet<&str>>::new();
        for finding in &self.findings {
            if !indexes.contains(&finding.screen) {
                continue;
            }
            let warning = match finding.kind.as_str() {
                "overflow" | "minor_overflow" => "content may be clipped",
                "off_screen" => "content may extend beyond the screen",
                "overfull" => "the preview was scaled down to fit",
                "overlap" => "text may overlap",
                "tiny_text" => "some text may be too small",
                "contrast" => "some text may have low contrast",
                "long_lines" => "some lines of text may be too long",
                "static_control" => "some elements look clickable but have no detected action",
                "alignment" => "some items may be unevenly aligned or spaced",
                "alignment_unmeasured" => "some alignment or spacing could not be measured",
                _ => continue,
            };
            by_screen.entry(finding.screen).or_default().insert(warning);
        }
        by_screen
            .into_iter()
            .filter_map(|(index, warnings)| {
                design.screens.get(index).map(|screen| {
                    let warnings: Vec<_> = warnings.into_iter().collect();
                    format!(
                        "Screen {} ({}): {}.",
                        index + 1,
                        screen.name,
                        warnings.join("; ")
                    )
                })
            })
            .collect()
    }

    /// Returns only layout blockers for repair rounds after a focused user edit.
    #[cfg(test)]
    pub(crate) fn layout_instructions(&self, indexes: &[usize]) -> Vec<String> {
        self.prototype_failures(Some(indexes))
            .chain(
                prioritized(
                    self.findings
                        .iter()
                        .filter(|finding| {
                            indexes.contains(&finding.screen) && is_blocking(&finding.kind)
                        })
                        .cloned()
                        .collect(),
                )
                .iter()
                .map(format_finding),
            )
            .collect()
    }

    /// Scores hard layout errors before optional visual improvements.
    pub(crate) fn score(&self, indexes: Option<&[usize]>) -> (usize, usize) {
        let findings: Vec<&Finding> = self
            .findings
            .iter()
            .filter(|finding| indexes.is_none_or(|indexes| indexes.contains(&finding.screen)))
            .collect();
        let prototype_failures = self.prototype_failures(indexes).count();
        (
            prototype_failures
                + findings
                    .iter()
                    .filter(|finding| is_blocking(&finding.kind))
                    .count(),
            prototype_failures + findings.len(),
        )
    }

    /// Tests progress without trading an existing blocker for a new one.
    pub(crate) fn improves_on(&self, previous: &Self, indexes: Option<&[usize]>) -> bool {
        let is_in_scope = |screen| indexes.is_none_or(|indexes| indexes.contains(&screen));
        let mut overflow_improved = false;
        for finding in self
            .findings
            .iter()
            .filter(|finding| is_in_scope(finding.screen) && is_blocking(&finding.kind))
        {
            let Some(before) = previous.findings.iter().find(|before| {
                before.screen == finding.screen
                    && before.node == finding.node
                    && before.kind == finding.kind
            }) else {
                return false;
            };
            if let Some([old_x, old_y]) = before.overflow_px {
                let Some([x, y]) = finding.overflow_px else {
                    return false;
                };
                if x > old_x || y > old_y {
                    return false;
                }
                overflow_improved |= x < old_x || y < old_y;
            }
        }
        for check in self
            .prototype_checks
            .iter()
            .filter(|check| check.failures > 0 && is_in_scope(check.screen))
        {
            if !previous.prototype_checks.iter().any(|before| {
                before.screen == check.screen
                    && before.group == check.group
                    && before.failures >= check.failures
                    && check
                        .issues
                        .iter()
                        .all(|issue| before.issues.contains(issue))
            }) {
                return false;
            }
        }
        let score = self.score(indexes);
        let previous_score = previous.score(indexes);
        score < previous_score || (score == previous_score && overflow_improved)
    }

    /// Lists concrete blockers on edited screens. Cosmetic findings are excluded.
    pub(crate) fn usable_edit_issues(
        &self,
        indexes: &[usize],
    ) -> impl Iterator<Item = (usize, String)> {
        self.failed_prototype_checks(Some(indexes)).chain(
            self.findings
                .iter()
                .filter(move |finding| {
                    indexes.contains(&finding.screen)
                        && matches!(finding.kind.as_str(), "empty" | "window_geometry")
                })
                .map(|finding| (finding.screen, format_finding(finding))),
        )
    }

    /// Blocks completion while any measured layout error remains on any screen.
    pub(crate) fn require_layout_clean(&self) -> Result<(), String> {
        self.require_layout_clean_on(None)
    }

    /// Verifies the touched screens without claiming that untouched legacy screens are clean.
    pub(crate) fn require_layout_clean_on(&self, indexes: Option<&[usize]>) -> Result<(), String> {
        let blockers: Vec<String> = self
            .prototype_failures(indexes)
            .chain(
                self.findings
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
                    }),
            )
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
    design_model::markup::validate_demo_navigation(&improved, Some(design))?;
    if !improved.validate().is_empty() || !respects_layout_contracts(design, &improved) {
        return Err("The tweak failed validation or changed a layout contract.".to_owned());
    }
    Ok(improved)
}

/// Preserves canvas and shared shell bounds while allowing obsolete polish annotations to change.
pub(crate) fn respects_layout_contracts(before: &Design, after: &Design) -> bool {
    before.viewport == after.viewport
        && before.app_window == after.app_window
        && before.screens.len() == after.screens.len()
}

fn is_known_finding(kind: &str) -> bool {
    is_blocking(kind)
        || matches!(
            kind,
            "tiny_text"
                | "contrast"
                | "long_lines"
                | "static_control"
                | "alignment"
                | "alignment_unmeasured"
                | "minor_overflow"
        )
}

fn is_blocking(kind: &str) -> bool {
    matches!(
        kind,
        "window_geometry" | "overflow" | "off_screen" | "overfull" | "overlap" | "empty"
    )
}

/// One finding as a fix instruction for the model.
fn format_finding(finding: &Finding) -> String {
    let advice = if finding.kind == "overflow" {
        "Fit the content inside the existing viewport. For a dialog, keep its actions reachable and let the body scroll inside the dialog if needed. Do not enlarge the viewport or clip content."
    } else {
        finding_advice(&finding.kind)
    };
    format!(
        "screens[{}] {}: {}: {}",
        finding.screen, finding.node, finding.detail, advice
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
         resizing one window. Preserve applicable data-layout-align and data-layout-gap declarations, but correct or remove obsolete annotations when the group changes. Do not delete valid annotations merely to hide findings. Alignment and unmeasurable groups are advisory, not completion blockers. Serious clipping, off-screen content, overlap, empty screens and shared-shell violations must still be resolved. Your reply is rendered and measured again. Do not add or delete screens. Change only the screens that need it.\n",
    );
    prompt.push_str(PATCH_FORMAT);
    prompt.push_str(" When nothing needs a change, reply with {}.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::render::AUDIT_SCRIPT;

    #[tokio::test]
    async fn audit_timeouts_are_distinguished_from_browser_installation_failures() {
        let elapsed = tokio::time::timeout(
            std::time::Duration::from_millis(1),
            std::future::pending::<()>(),
        )
        .await
        .expect_err("pending browser operation should exceed its deadline");
        let message = browser_audit_failure(
            anyhow::Error::new(elapsed)
                .context("internal rendered page path must not appear in session errors"),
        );
        assert!(message.contains("timed out after one browser-only retry"));
        assert!(message.contains("fonts or other page resources may be slow"));
        assert!(!message.contains("Check Chrome"));
        assert!(!message.contains("internal rendered page"));
        assert_eq!(
            browser_audit_failure(anyhow::anyhow!("private browser path")),
            "Could not run the browser layout audit. Check Chrome and retry."
        );
    }

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
            "empty",
        ] {
            let dom = format!(
                "<html data-swift-design-findings=\"[{{&quot;screen&quot;:0,&quot;kind&quot;:&quot;{kind}&quot;}}]\" data-swift-design-audited-screens=\"[0]\">"
            );
            let audit = DemoAudit::from_dom(&dom, 1)
                .expect("fixture DOM should contain a valid audit result");
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
            serde_json::from_str(include_str!("../../../../fixtures/sample-design.json"))
                .expect("test JSON should deserialize");
        assert!(apply_tweak(&design, r#"{"screens":[{"index":0,"screen":null}]}"#, None).is_err());
        let screen =
            serde_json::to_value(&design.screens[0]).expect("test value should convert to JSON");
        let insertion =
            serde_json::json!({"screens":[{"index":0,"insert":true,"screen":screen}]}).to_string();
        assert!(apply_tweak(&design, &insertion, None).is_err());
        let replacement = serde_json::json!({"screens":[{"index":1,"screen":screen}]}).to_string();
        assert!(apply_tweak(&design, &replacement, Some(&[0])).is_err());
        assert!(apply_tweak(&design, &replacement, Some(&[1])).is_ok());
    }

    #[test]
    fn tweaks_can_remove_obsolete_annotations_but_cannot_resize_the_canvas() {
        let mut before: Design =
            serde_json::from_str(include_str!("../../../../fixtures/sample-design.json"))
                .expect("test JSON should deserialize");
        before.screens[0].html =
            "<div data-layout-align='left'><p>One</p><p>Two</p></div>".to_owned();
        let mut after = before.clone();
        assert!(respects_layout_contracts(&before, &after));
        after.screens[0].html = "<div><p>One</p><p>Two</p></div>".to_owned();
        assert!(respects_layout_contracts(&before, &after));
        after = before.clone();
        after.viewport.width += 100;
        assert!(!respects_layout_contracts(&before, &after));
    }

    #[test]
    fn smaller_overflow_counts_only_when_no_blocker_is_new_or_worse() {
        let overflow = |screen, x, y| Finding {
            screen,
            node: "div.scrim (0)".into(),
            kind: "overflow".into(),
            detail: String::new(),
            overflow_px: Some([x, y]),
        };
        let audit = |findings| DemoAudit {
            findings,
            prototype_checks: vec![],
        };
        let previous = audit(vec![overflow(61, 0, 43), overflow(62, 0, 28)]);
        let smaller = audit(vec![overflow(61, 0, 20), overflow(62, 0, 28)]);
        assert!(smaller.improves_on(&previous, None));
        assert!(!smaller.improves_on(&smaller, None));
        assert!(!audit(vec![overflow(61, 5, 20)]).improves_on(&previous, None));
        assert!(!audit(vec![overflow(61, 0, 46)]).improves_on(&previous, None));
        let new = audit(vec![overflow(63, 0, 10)]);
        assert!(!new.improves_on(&previous, None));
        assert!(new.improves_on(&previous, Some(&[61, 62])));
        assert!(smaller.require_layout_clean().is_err());
    }

    #[test]
    fn focused_verification_does_not_claim_untouched_legacy_errors_are_clean() {
        let audit = DemoAudit {
            prototype_checks: Vec::new(),
            findings: vec![Finding {
                screen: 1,
                node: String::new(),
                kind: "overflow".to_owned(),
                detail: "legacy clipping".to_owned(),
                overflow_px: None,
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
        for kind in [
            "contrast",
            "tiny_text",
            "alignment",
            "alignment_unmeasured",
            "minor_overflow",
        ] {
            let audit = DemoAudit {
                prototype_checks: Vec::new(),
                findings: vec![Finding {
                    screen: 0,
                    node: String::new(),
                    kind: kind.to_owned(),
                    detail: "Review this measurement".to_owned(),
                    overflow_px: None,
                }],
            };
            assert!(is_known_finding(kind));
            assert!(audit.require_layout_clean().is_ok());
            assert_eq!(audit.score(None), (0, 1));
            assert!(audit.layout_instructions(&[0]).is_empty());
            assert_eq!(audit.instructions(None).len(), 1);
        }
    }

    #[test]
    fn findings_are_read_from_the_dumped_dom() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:2,&quot;node&quot;:&quot;h2.title (0/1)&quot;,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;content needs 410px but the box is 376px tall&quot;}]\"><head>";
        let findings = parse_findings(dom);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].starts_with(
            "screens[2] h2.title (0/1): content needs 410px but the box is 376px tall:"
        ));
        assert!(findings[0].contains("Do not enlarge the viewport or clip content."));
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
                overflow_px: None,
            })
            .collect();
        raw.push(Finding {
            screen: 1,
            node: "h2 (0/1)".to_owned(),
            kind: "overflow".to_owned(),
            detail: "content needs 59px but the box is 30px tall".to_owned(),
            overflow_px: Some([0, 29]),
        });
        raw.push(Finding {
            screen: 0,
            node: "p (0/2)".to_owned(),
            kind: "contrast".to_owned(),
            detail: "contrast 2.1:1".to_owned(),
            overflow_px: None,
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
            overflow_px: None,
        });
        assert!(contrast.starts_with("screens[3] h2 (0): contrast 1.50:1"));
        assert!(contrast.ends_with("change the background"));
        let lines = format_finding(&Finding {
            screen: 4,
            node: "p (1)".to_owned(),
            kind: "long_lines".to_owned(),
            detail: "about 120 characters per line over 1 line".to_owned(),
            overflow_px: None,
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
        assert!(stalled.contains("did not improve the layout"));
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
