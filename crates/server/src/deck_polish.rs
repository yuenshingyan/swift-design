//! The deck polish pass: a design review of each deck candidate after it
//! validates.
//!
//! The deck twin of `polish.rs`. The in-browser audit and the finding
//! parser are shared; the finding paths (`slides[n]`), the reviewer role,
//! and the patch format are deck-specific, so the model sees one
//! vocabulary per artifact kind.

use design_model::Deck;

use crate::model_client::LogSink;
use crate::polish::{Finding, finding_advice, prioritized, raw_findings};

/// Layout findings for a deck, measured in a browser. Empty when no
/// Chrome is installed or the audit fails; both are logged.
pub async fn dom_findings(deck: &Deck, base_url: &str, label: &str, log: &LogSink) -> Vec<String> {
    if crate::screenshots::find_chrome().is_none() {
        log(&format!(
            "{label}: no Chrome found for the layout audit; reviewing from JSON only"
        ));
        return Vec::new();
    }
    match crate::screenshots::dump_deck_dom(deck, base_url).await {
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

/// Reads the audit report out of a dumped deck DOM as slide findings.
pub fn parse_findings(dom: &str) -> Vec<String> {
    prioritized(raw_findings(dom))
        .iter()
        .map(format_finding)
        .collect()
}

/// One finding as a fix instruction for the model, with a slide path.
fn format_finding(finding: &Finding) -> String {
    format!(
        "slides[{}] {}: {}: {}",
        finding.screen,
        finding.node,
        finding.detail,
        finding_advice(&finding.kind)
    )
}

/// The review request for one deck candidate: the deck, the findings,
/// and the design checklist. `image_count` slide images follow the
/// prompt when the model can see.
pub fn polish_prompt(deck_json: &str, findings: &[String], image_count: usize) -> String {
    let mut prompt = format!(
        "Review this candidate deck as a presentation designer and improve it.\n\
         Deck JSON:\n{deck_json}\n"
    );
    if image_count > 0 {
        prompt.push_str(&format!(
            "The next {image_count} images show the rendered slides in order, one per slide. \
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
        "Then check and improve: text that overflows its box or the slide, text blocks that \
         overlap or touch, uneven margins, misaligned edges, inconsistent sizes for the same \
         role across slides, low contrast between text and background, slides that are too \
         dense or too empty, and a weak title slide. Keep the concept, the outline, and the \
         content. Do not add or delete slides. Change only the slides that need it.\n",
    );
    prompt.push_str(crate::deck_patch::PATCH_FORMAT);
    prompt.push_str(" When nothing needs a change, reply with {}.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_findings_use_slide_paths() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:2,&quot;node&quot;:&quot;h2.title (0/1)&quot;,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;content needs 410px but the box is 376px tall&quot;}]\"><head>";
        let findings = parse_findings(dom);
        assert_eq!(
            findings,
            [
                "slides[2] h2.title (0/1): content needs 410px but the box is 376px tall: shorten the text, enlarge the box, or reduce the font size"
            ]
        );
        assert!(parse_findings("<html></html>").is_empty());
    }

    #[test]
    fn deck_prompts_speak_of_slides_and_the_deck_patch() {
        let prompt = polish_prompt("{}", &["slides[0] x".to_owned()], 2);
        assert!(prompt.contains("as a presentation designer"));
        assert!(prompt.contains("- slides[0] x"));
        assert!(prompt.contains("next 2 images show the rendered slides"));
        assert!(prompt.contains("Do not add or delete slides"));
        assert!(prompt.contains("\"slides\":[{\"index\":2,\"slide\""));
        assert!(!prompt.contains("screen"));
        assert!(polish_prompt("{}", &[], 0).contains("no problems found"));
    }
}
