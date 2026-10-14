use super::*;

/// The print system prompt: role, print rules, the print
/// schema, the clarification protocol, and one example print.
pub(super) fn print_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Print)).unwrap_or_default();
    format!(
        "You build print pieces as JSON prints: posters, flyers, and similar pieces put on paper. \
         Each sheet is one HTML fragment plus its own CSS, for the px canvas of the print's size, \
         rotated by its orientation: 559 by 794 px for A5, 794 by 1123 px for A4, \
         1123 by 1587 px for A3, 816 by 1056 px for Letter, 1056 by 1632 px for Tabloid. \
         One sheet is a poster. Two sheets are a flyer with a front and a back.\n\
         Follow these rules:\n{rules}\n\
         The print must conform to this JSON Schema:\n{schema}\n\
         Example print:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one print JSON. No prose, no code fences.",
        rules = PRINT_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-print.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` sheets and
/// the full outline.
pub(super) fn print_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} sheets of the print, in order, starting with \
         the first sheet. Put the sheet titles of the complete print in `outline`, in order, \
         every sheet title of the complete print. The app asks you for the remaining sheets \
         later. Make these {count} sheets show the theme, the layout language, and the text \
         density of the whole print.\n"
    )
}

/// The prompt line for the app's size and orientation choices. Empty
/// when the agent decides both, or when the user typed values the JSON
/// does not carry.
pub(super) fn size_note(size: Option<&str>, orientation: Option<&str>) -> String {
    let size = size.and_then(PrintSize::from_name);
    let orientation = orientation.and_then(Orientation::from_name);
    match (size, orientation) {
        (Some(size), Some(orientation)) => {
            let viewport = orientation.apply(size.viewport());
            format!(
                "Lay the sheets out on the {} size, {}: {} by {} px. Set `size` to `{}` and \
                 `orientation` to `{}`.\n",
                size.as_str(),
                orientation.as_str(),
                viewport.width,
                viewport.height,
                size.as_str(),
                orientation.as_str()
            )
        }
        (Some(size), None) => format!(
            "Lay the sheets out on the {} size. Set `size` to `{}`.\n",
            size.as_str(),
            size.as_str()
        ),
        (None, Some(orientation)) => format!(
            "Turn every sheet {}. Set `orientation` to `{}`.\n",
            orientation.as_str(),
            orientation.as_str()
        ),
        (None, None) => String::new(),
    }
}

/// The prompt line that holds the print to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// sheets than the length, so the count goes to the outline instead.
pub(super) fn sheet_count_note(sheet_count: Option<u32>, preview_sheets: Option<usize>) -> String {
    let Some(count) = sheet_count else {
        return String::new();
    };
    match preview_sheets {
        Some(_) => {
            format!("The user asked for {count} sheets. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} sheets. Write exactly {count} sheets.\n"),
    }
}

/// The user prompt for one print candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
pub(super) fn print_candidate_prompt(request: &PrintCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a print piece for this request. The request and the answers are \
         authoritative; do not override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are screens, slides, or pages of another artifact. Use them \
             for the look only.\n",
        );
    }
    if let Some(count) = request.preview_sheets {
        prompt.push_str(&print_preview_note(count));
    }
    prompt.push_str(&sheet_count_note(
        options.sheet_count,
        request.preview_sheets,
    ));
    prompt.push_str(&size_note(
        options.print_size.as_deref(),
        options.orientation.as_deref(),
    ));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("print", merge));
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
        "low" => prompt.push_str("Keep the print concise: fewer sheets, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the print JSON.");
    prompt
}

/// The user prompt for a print edit: the print as it is, the
/// request, and the change the user asked for.
pub(super) fn print_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the print to change:\n{print_json}\n{note}\
         The print is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [sheet 3, node 0/1 <h2.title>: What changed] names a sheet \
         (1-based) and one element in that sheet's html by its index path from the sheet root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [sheet 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one sheet the same way, without their text. A reference like [sheet 3] \
         names the sheet alone: the change is about that sheet. Change only what the critique asks for. Keep every other sheet and \
         value as it is. Return every changed sheet complete: html, css, and notes.\n{format}",
        print_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The print as a focused edit sees it: the title, the theme, the
/// size, the orientation, the sheet count, and only the sheets at
/// `indexes`, each with its index.
pub(super) fn focused_print_json(
    print: &Print,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let sheets: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            print.sheets.get(*index).map(|sheet| {
                let sheet = if is_fresh {
                    fresh_sheet(sheet)
                } else {
                    sheet.clone()
                };
                serde_json::json!({ "index": index, "sheet": sheet })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": print.title,
        "theme": print.theme,
        "size": print.size,
        "orientation": print.orientation,
        "sheet_count": print.sheets.len(),
        "sheets": sheets,
    }))
}

/// The sheet as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
pub(super) fn fresh_sheet(sheet: &Sheet) -> Sheet {
    Sheet {
        html: String::new(),
        css: None,
        ..sheet.clone()
    }
}

/// The user prompt for one print continuation chunk: the preview
/// print and the chunk's sheets to add, as a patch of inserts.
pub(super) fn print_continue_prompt(
    request: &SessionRequest,
    print: &Print,
    print_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = print.sheets.len();
    let planned = print.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = print
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a print in progress: its theme, its size and orientation, its first {written} \
         sheets, and `outline`, the sheet titles of the complete print:\n{print_json}\n\
         The print is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} sheets: outline titles {} to {last} of {planned}, in order, one sheet per \
         title:\n{}\n\
         Keep the theme. Match the existing sheets in CSS style, font sizes, spacing, colors, \
         and visual language, so the print reads as one piece. Do not change or repeat \
         the existing sheets.\n",
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
        "Reply with only a JSON patch that appends the new sheets, not the whole print:\n\
         {{\"sheets\":[{{\"index\":{written},\"insert\":true,\"sheet\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new sheet index {written} and insert true, in reading order. Each sheet \
         carries html, css, and notes. Omit title, theme, size, orientation, outline, and the existing sheets."
    ));
    prompt
}

/// Extracts and parses the print JSON from a model reply.
pub(super) fn parse_print(content: &str) -> Result<Print, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid print: {error}"))
}
