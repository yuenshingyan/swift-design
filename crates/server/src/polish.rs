//! The polish pass: a design review of each candidate after it
//! validates.
//!
//! Validation checks that a design is safe and well formed. Polish checks
//! that it looks right. When Chrome is available, the rendered design runs
//! a layout audit in the browser (overflowing boxes, content off the
//! screen, tiny text, overlapping text blocks, empty screens, low
//! contrast, long lines) and the findings go to the model together with
//! screen screenshots. An improved design that validates replaces the
//! original. The audit script itself is `render::AUDIT_SCRIPT`. The deck
//! twin, `deck_polish.rs`, shares the audit and the parser and differs in
//! wording only.

use design_model::Design;
use serde::Deserialize;

use crate::model_client::LogSink;

/// One finding from the in-browser audit script.
#[derive(Debug, Deserialize)]
pub(crate) struct Finding {
    /// Zero-based screen or slide index.
    pub(crate) screen: usize,
    /// The node, like `h2.title (0/1)`, or `root`.
    #[serde(default)]
    pub(crate) node: String,
    /// `overflow`, `off_screen`, `overfull`, `tiny_text`, `overlap`,
    /// `empty`, `contrast`, `long_lines`, or `static_control`.
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
    crate::screenshots::find_chrome().is_some()
}

/// Layout findings measured in a browser. Empty when no Chrome is
/// installed or the audit fails; both are logged.
pub async fn dom_findings(
    design: &Design,
    base_url: &str,
    label: &str,
    log: &LogSink,
) -> Vec<String> {
    if crate::screenshots::find_chrome().is_none() {
        log(&format!(
            "{label}: no Chrome found for the layout audit; reviewing from JSON only"
        ));
        return Vec::new();
    }
    match crate::screenshots::dump_design_dom(design, base_url).await {
        Ok(dom) => {
            let findings = parse_findings(&dom);
            if findings.is_empty() && !dom.contains("data-swift-design-findings") {
                log(&format!("{label}: the layout audit returned no report"));
            }
            findings
        }
        Err(error) => {
            log(&format!("{label}: layout audit failed: {error}"));
            Vec::new()
        }
    }
}

/// Reads the audit report out of a dumped DOM: the JSON in the
/// `data-swift-design-findings` attribute on `<html>`. The findings
/// come ordered by severity and capped per kind, see `prioritized`.
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
        "overfull" => 0,
        "off_screen" => 1,
        "overflow" => 2,
        "overlap" => 3,
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
    let marker = "data-swift-design-findings=\"";
    let Some(start) = dom.find(marker) else {
        return Vec::new();
    };
    let rest = &dom[start + marker.len()..];
    let Some(end) = rest.find('"') else {
        return Vec::new();
    };
    let json = rest[..end]
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    serde_json::from_str(&json).unwrap_or_default()
}

/// The fix advice for one finding kind.
pub(crate) fn finding_advice(kind: &str) -> &'static str {
    match kind {
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
         content. Do not add or delete screens. Change only the screens that need it.\n",
    );
    prompt.push_str(crate::patch::PATCH_FORMAT);
    prompt.push_str(" When nothing needs a change, reply with {}.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::AUDIT_SCRIPT;

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
        let script = crate::render::AUDIT_SCRIPT;
        assert!(script.contains("canvasWidth >= 1920 ? { flag: 20, ask: 24 }"));
        assert!(script.contains("canvasWidth >= 1000 ? { flag: 12, ask: 14 }"));
        assert!(script.contains("{ flag: 11, ask: 12 }"));
        assert!(script.contains("if (size < textFloor.flag)"));
    }

    #[test]
    fn the_audit_script_holds_the_contrast_check() {
        let script = crate::render::AUDIT_SCRIPT;
        assert!(script.contains("const contrastRatio = (a, b) =>"));
        assert!(script.contains("const luminance = (rgb) =>"));
        assert!(script.contains("(Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)"));
        assert!(script.contains("kind: 'contrast'"));
        assert!(script.contains("size >= 24 || (weight >= 700 && size >= 18.66) ? 3.0 : 4.5"));
    }

    #[test]
    fn the_audit_script_holds_the_line_length_check() {
        let script = crate::render::AUDIT_SCRIPT;
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
