//! The print polish pass: a design review of each print candidate
//! after it validates.
//!
//! The print twin of `deck_polish.rs`. The in-browser audit and the
//! finding parser are shared; the finding paths (`sheets[n]`), the
//! reviewer role, and the patch format are print-specific, so the
//! model sees one vocabulary per artifact kind.

use design_model::Print;

use crate::model_client::LogSink;
use crate::polish::{Finding, finding_advice, prioritized, raw_findings};

/// Layout findings for a print, measured in a browser. Empty when no
/// Chrome is installed or the audit fails; both are logged.
pub async fn dom_findings(
    print: &Print,
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
    match crate::screenshots::dump_print_dom(print, base_url).await {
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

/// Reads the audit report out of a dumped print DOM as sheet findings.
pub fn parse_findings(dom: &str) -> Vec<String> {
    // A sheet is put on paper, not clicked, so a box styled as a
    // button is a design choice there, not a static control.
    let findings = raw_findings(dom)
        .into_iter()
        .filter(|finding| finding.kind != "static_control")
        .collect();
    prioritized(findings).iter().map(format_finding).collect()
}

/// One finding as a fix instruction for the model, with a sheet path.
fn format_finding(finding: &Finding) -> String {
    format!(
        "sheets[{}] {}: {}: {}",
        finding.screen,
        finding.node,
        finding.detail,
        finding_advice(&finding.kind)
    )
}

/// The review request for one print candidate: the print, the
/// findings, and the design checklist. `image_count` sheet images follow
/// the prompt when the model can see.
pub fn polish_prompt(print_json: &str, findings: &[String], image_count: usize) -> String {
    let mut prompt = format!(
        "Review this candidate print piece as a print designer and improve it.\n\
         Print JSON:\n{print_json}\n"
    );
    if image_count > 0 {
        prompt.push_str(&format!(
            "The next {image_count} images show the rendered sheets in order, one per sheet. \
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
        "Then check and improve: text that overflows its box or the sheet, text blocks that \
         overlap or touch, uneven margins, misaligned edges, inconsistent sizes for the same \
         role across sheets, low contrast between text and background, text too small to \
         read on paper, sheets that are too dense or too empty, and a weak first sheet: \
         the first sheet is the face of the piece. Keep the concept, the outline, and the \
         content. Do not add or delete sheets. Change only the sheets that need it.\n",
    );
    prompt.push_str(crate::print_patch::PATCH_FORMAT);
    prompt.push_str(" When nothing needs a change, reply with {}.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_print_drops_the_static_control_findings() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:0,&quot;node&quot;:&quot;a.cta (0/1)&quot;,&quot;kind&quot;:&quot;static_control&quot;,&quot;detail&quot;:&quot;looks like a control but does nothing when clicked&quot;}]\"><head>";
        assert_eq!(parse_findings(dom), Vec::<String>::new());
    }

    #[test]
    fn print_findings_use_sheet_paths() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:2,&quot;node&quot;:&quot;h2.title (0/1)&quot;,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;content needs 410px but the box is 376px tall&quot;}]\"><head>";
        let findings = parse_findings(dom);
        assert_eq!(
            findings,
            [
                "sheets[2] h2.title (0/1): content needs 410px but the box is 376px tall: shorten the text, enlarge the box, or reduce the font size"
            ]
        );
        assert!(parse_findings("<html></html>").is_empty());
    }

    #[test]
    fn print_prompts_speak_of_sheets_and_the_print_patch() {
        let prompt = polish_prompt("{}", &["sheets[0] x".to_owned()], 2);
        assert!(prompt.contains("as a print designer"));
        assert!(prompt.contains("the face of the piece"));
        assert!(prompt.contains("- sheets[0] x"));
        assert!(prompt.contains("next 2 images show the rendered sheets"));
        assert!(prompt.contains("Do not add or delete sheets"));
        assert!(prompt.contains("\"sheets\":[{\"index\":2,\"sheet\""));
        assert!(!prompt.contains("screen"));
        assert!(!prompt.contains("slide"));
        assert!(polish_prompt("{}", &[], 0).contains("no problems found"));
    }
}
