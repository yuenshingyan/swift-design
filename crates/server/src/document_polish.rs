//! The document polish pass: a design review of each document candidate
//! after it validates.
//!
//! The document twin of `deck_polish.rs`. The in-browser audit and the
//! finding parser are shared; the finding paths (`pages[n]`), the
//! reviewer role, and the patch format are document-specific, so the
//! model sees one vocabulary per artifact kind.

use design_model::Document;

use crate::model_client::LogSink;
use crate::polish::{Finding, finding_advice, prioritized, raw_findings};

/// Layout findings for a document, measured in a browser. Empty when no
/// Chrome is installed or the audit fails; both are logged.
pub async fn dom_findings(
    document: &Document,
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
    match crate::screenshots::dump_document_dom(document, base_url).await {
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

/// Reads the audit report out of a dumped document DOM as page findings.
pub fn parse_findings(dom: &str) -> Vec<String> {
    // A page is read, not clicked, so a box styled as a button is a
    // design choice there, not a static control.
    let findings = raw_findings(dom)
        .into_iter()
        .filter(|finding| finding.kind != "static_control")
        .collect();
    prioritized(findings).iter().map(format_finding).collect()
}

/// One finding as a fix instruction for the model, with a page path.
fn format_finding(finding: &Finding) -> String {
    format!(
        "pages[{}] {}: {}: {}",
        finding.screen,
        finding.node,
        finding.detail,
        finding_advice(&finding.kind)
    )
}

/// The review request for one document candidate: the document, the
/// findings, and the design checklist. `image_count` page images follow
/// the prompt when the model can see.
pub fn polish_prompt(document_json: &str, findings: &[String], image_count: usize) -> String {
    let mut prompt = format!(
        "Review this candidate document as an editorial designer and improve it.\n\
         Document JSON:\n{document_json}\n"
    );
    if image_count > 0 {
        prompt.push_str(&format!(
            "The next {image_count} images show the rendered pages in order, one per page. \
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
        "Then check and improve: text that overflows its box or the page, text blocks that \
         overlap or touch, uneven margins, misaligned edges, inconsistent sizes for the same \
         role across pages, low contrast between text and background, pages that are too \
         dense or too empty, orphan headings at the foot of a page, and a weak first page. \
         Keep the concept, the outline, and the content. Do not add or delete pages. Change \
         only the pages that need it.\n",
    );
    prompt.push_str(crate::document_patch::PATCH_FORMAT);
    prompt.push_str(" When nothing needs a change, reply with {}.");
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_drops_the_static_control_findings() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:0,&quot;node&quot;:&quot;a.cta (0/1)&quot;,&quot;kind&quot;:&quot;static_control&quot;,&quot;detail&quot;:&quot;looks like a control but does nothing when clicked&quot;}]\"><head>";
        assert_eq!(parse_findings(dom), Vec::<String>::new());
    }

    #[test]
    fn document_findings_use_page_paths() {
        let dom = "<html lang=\"en\" data-swift-design-findings=\"[{&quot;screen&quot;:2,&quot;node&quot;:&quot;h2.title (0/1)&quot;,&quot;kind&quot;:&quot;overflow&quot;,&quot;detail&quot;:&quot;content needs 410px but the box is 376px tall&quot;}]\"><head>";
        let findings = parse_findings(dom);
        assert_eq!(
            findings,
            [
                "pages[2] h2.title (0/1): content needs 410px but the box is 376px tall: shorten the text, enlarge the box, or reduce the font size"
            ]
        );
        assert!(parse_findings("<html></html>").is_empty());
    }

    #[test]
    fn document_prompts_speak_of_pages_and_the_document_patch() {
        let prompt = polish_prompt("{}", &["pages[0] x".to_owned()], 2);
        assert!(prompt.contains("as an editorial designer"));
        assert!(prompt.contains("- pages[0] x"));
        assert!(prompt.contains("next 2 images show the rendered pages"));
        assert!(prompt.contains("Do not add or delete pages"));
        assert!(prompt.contains("\"pages\":[{\"index\":2,\"page\""));
        assert!(!prompt.contains("screen"));
        assert!(!prompt.contains("slide"));
        assert!(polish_prompt("{}", &[], 0).contains("no problems found"));
    }
}
