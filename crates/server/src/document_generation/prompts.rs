use super::*;

/// The document system prompt: role, document rules, the document
/// schema, the clarification protocol, and one example document.
pub(super) fn document_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Document)).unwrap_or_default();
    format!(
        "You build paged documents as JSON documents: reports, memos, proposals, letters, and guides. \
         Each page is one HTML fragment plus its own CSS, for the px canvas of the document's paper: \
         794 by 1123 px for A4, 816 by 1056 px for Letter.\n\
         Follow these rules:\n{rules}\n\
         The document must conform to this JSON Schema:\n{schema}\n\
         Example document:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one document JSON. No prose, no code fences.",
        rules = DOCUMENT_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-document.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` pages and
/// the full outline.
pub(super) fn document_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} pages of the document, in order, starting with \
         the first page. Put the page titles of the complete document in `outline`, in order, \
         every page title of the complete document. The app asks you for the remaining pages \
         later. Make these {count} pages show the theme, the layout language, and the text \
         density of the whole document.\n"
    )
}

/// The prompt line for the app's paper choice. Empty when the agent
/// decides, or when the user typed a paper the JSON does not carry.
pub(super) fn paper_note(paper: Option<&str>) -> String {
    match paper.and_then(Paper::from_name) {
        Some(paper) => {
            let viewport = paper.viewport();
            format!(
                "Lay the pages out on {} paper: {} by {} px. Set `paper` to `{}`.\n",
                paper.label(),
                viewport.width,
                viewport.height,
                paper.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the document to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// pages than the length, so the count goes to the outline instead.
pub(super) fn page_count_note(page_count: Option<u32>, preview_pages: Option<usize>) -> String {
    let Some(count) = page_count else {
        return String::new();
    };
    match preview_pages {
        Some(_) => {
            format!("The user asked for {count} pages. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} pages. Write exactly {count} pages.\n"),
    }
}

/// The user prompt for one document candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
pub(super) fn document_candidate_prompt(request: &DocumentCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a document for this request. The request and the answers are authoritative; do \
         not override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are pages or slides of another artifact. Use them for the \
             look only.\n",
        );
    }
    if let Some(count) = request.preview_pages {
        prompt.push_str(&document_preview_note(count));
    }
    prompt.push_str(&page_count_note(options.page_count, request.preview_pages));
    prompt.push_str(&paper_note(options.paper.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("document", merge));
    }
    let count = options.variation_count();
    // A merge is one candidate on its own, not one of the run's set.
    if count > 1 && request.merge.is_none() {
        prompt.push_str(&format!(
            "This is candidate {candidate_number} of {count}. Make it distinct from the other \
             candidates in theme, structure, and angle.\n"
        ));
        prompt.push_str(&concept_note(request.concepts, candidate_number - 1));
    }
    match options.effort.as_str() {
        "low" => prompt.push_str("Keep the document concise: fewer pages, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the document JSON.");
    prompt
}

/// The user prompt for a document edit: the document as it is, the
/// request, and the change the user asked for.
pub(super) fn document_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the document to change:\n{document_json}\n{note}\
         The document is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [page 3, node 0/1 <h2.title>: What changed] names a page \
         (1-based) and one element in that page's html by its index path from the page root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [page 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one page the same way, without their text. A reference like [page 3] \
         names the page alone: the change is about that page. Change only what the critique asks for. Keep every other page and \
         value as it is. Return every changed page complete: html, css, and notes.\n{format}",
        document_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The document as a focused edit sees it: the title, the theme, the
/// paper, the page count, and only the pages at `indexes`, each with
/// its index.
pub(super) fn focused_document_json(
    document: &Document,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let pages: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            document.pages.get(*index).map(|page| {
                let page = if is_fresh {
                    fresh_page(page)
                } else {
                    page.clone()
                };
                serde_json::json!({ "index": index, "page": page })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": document.title,
        "theme": document.theme,
        "paper": document.paper,
        "page_count": document.pages.len(),
        "pages": pages,
    }))
}

/// The page as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
pub(super) fn fresh_page(page: &Page) -> Page {
    Page {
        html: String::new(),
        css: None,
        ..page.clone()
    }
}

/// The user prompt for one document continuation chunk: the preview
/// document and the chunk's pages to add, as a patch of inserts.
pub(super) fn document_continue_prompt(
    request: &SessionRequest,
    document: &Document,
    document_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = document.pages.len();
    let planned = document.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = document
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a document in progress: its theme, its paper, its first {written} pages, and \
         `outline`, the page titles of the complete document:\n{document_json}\n\
         The document is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} pages: outline titles {} to {last} of {planned}, in order, one page per \
         title:\n{}\n\
         Keep the theme. Match the existing pages in CSS style, font sizes, spacing, colors, \
         and visual language, so the document reads as one document. Do not change or repeat \
         the existing pages.\n",
        next_titles.len(),
        first + 1,
        next_titles.join("\n")
    ));
    if first > written || last < planned {
        prompt.push_str(
            "Other requests write the other outline titles at the same time. Write only these.\n",
        );
    }
    prompt.push_str(&format!(
        "Reply with only a JSON patch that appends the new pages, not the whole document:\n\
         {{\"pages\":[{{\"index\":{written},\"insert\":true,\"page\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new page index {written} and insert true, in reading order. Each page \
         carries html, css, and notes. Omit title, theme, paper, outline, and the existing pages."
    ));
    prompt
}

/// Extracts and parses the document JSON from a model reply.
pub(super) fn parse_document(content: &str) -> Result<Document, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid document: {error}"))
}
