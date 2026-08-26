//! The polish pass: a design review of each candidate after it
//! validates.
//!
//! Validation checks that a design is safe and well formed. Polish checks
//! that it looks right. When Chrome is available, the rendered design runs
//! a layout audit in the browser (overflowing boxes, content off the
//! screen, tiny text, overlapping text blocks, empty screens, low
//! contrast, long lines) and the findings go to the model together with
//! screen screenshots. An improved design that validates replaces the
//! original. The audit script itself is `render::AUDIT_SCRIPT`.

use design_model::Design;
use serde::Deserialize;

use crate::generation::LogSink;

/// One finding from the in-browser audit script.
#[derive(Debug, Deserialize)]
struct Finding {
    /// Zero-based screen index.
    screen: usize,
    /// The node, like `h2.title (0/1)`, or `root`.
    #[serde(default)]
    node: String,
    /// `overflow`, `off_screen`, `tiny_text`, `overlap`, `empty`,
    /// `contrast`, or `long_lines`.
    #[serde(default)]
    kind: String,
    /// What was measured.
    #[serde(default)]
    detail: String,
}

/// Polish rounds per candidate, by effort level.
pub fn polish_rounds(effort: &str) -> usize {
    match effort {
        "low" => 0,
        "high" => 2,
        _ => 1,
    }
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
/// `data-swift-design-findings` attribute on `<html>`.
pub fn parse_findings(dom: &str) -> Vec<String> {
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
    let findings: Vec<Finding> = serde_json::from_str(&json).unwrap_or_default();
    findings.iter().map(format_finding).collect()
}

/// One finding as a fix instruction for the model.
fn format_finding(finding: &Finding) -> String {
    let advice = match finding.kind.as_str() {
        "overflow" => "shorten the text, enlarge the box, or reduce the font size",
        "off_screen" => "move or shrink it so it stays inside the screen",
        "tiny_text" => "use a larger font size",
        "overlap" => "move or resize one of them",
        "empty" => "add content or delete the screen",
        "contrast" => "use a darker or lighter text color, or change the background",
        "long_lines" => "narrow the box, split the text, or use a larger font size",
        _ => "fix it",
    };
    format!(
        "screens[{}] {}: {}: {advice}",
        finding.screen, finding.node, finding.detail
    )
}

/// The review request for one candidate: the design, the findings, and
/// the design checklist. `image_count` screen images follow the prompt
/// when the model can see.
pub fn polish_prompt(design_json: &str, findings: &[String], image_count: usize) -> String {
    let mut prompt = format!(
        "Review this candidate design as a presentation designer and improve it.\n\
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
    fn prompts_carry_findings_and_the_checklist() {
        let prompt = polish_prompt("{}", &["screens[0] x".to_owned()], 0);
        assert!(prompt.contains("- screens[0] x"));
        assert!(prompt.contains("Keep the concept"));
        assert!(prompt.contains("JSON patch"));
        assert!(prompt.contains("reply with {}"));
        assert!(!prompt.contains("images show"));
        assert!(polish_prompt("{}", &[], 3).contains("no problems found"));
        assert!(polish_prompt("{}", &[], 3).contains("next 3 images"));
        assert_eq!(polish_rounds("low"), 0);
        assert_eq!(polish_rounds("high"), 2);
    }
}
