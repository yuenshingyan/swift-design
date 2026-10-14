use super::*;

/// The artwork system prompt: role, artwork rules, the artwork
/// schema, the clarification protocol, and one example artwork.
pub(super) fn artwork_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Artwork)).unwrap_or_default();
    format!(
        "You build cover art as JSON artworks: video thumbnails, channel banners, profile headers, album covers, and book covers judged at a glance. \
         Each cover is one HTML fragment plus its own CSS, for the px canvas of the artwork's size: \
         1280 by 720 px for thumbnail, 2560 by 1440 px for banner, 1500 by 500 px for \
         header, 3000 by 3000 px for album, 1600 by 2560 px for book. \
         One cover is a single piece. Two or more covers are A/B variants of the same size, in priority order.\n\
         Follow these rules:\n{rules}\n\
         The artwork must conform to this JSON Schema:\n{schema}\n\
         Example artwork:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one artwork JSON. No prose, no code fences.",
        rules = ARTWORK_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-artwork.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` covers and
/// the full outline.
pub(super) fn artwork_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} covers of the artwork, in order, starting with \
         the first cover. Put the cover titles of the complete artwork in `outline`, in order, \
         every cover title of the complete artwork. The app asks you for the remaining covers \
         later. Make these {count} covers show the theme, the layout language, and the text \
         density of the whole artwork.\n"
    )
}

/// The prompt line for the app's size choice. Empty when the agent
/// decides it, or when the user typed a value the JSON does not carry.
pub(super) fn size_note(size: Option<&str>) -> String {
    match size.and_then(CoverSize::from_name) {
        Some(size) => {
            let viewport = size.viewport();
            format!(
                "Lay the covers out on the {} size: {} by {} px. Set `size` to `{}`.\n",
                size.as_str(),
                viewport.width,
                viewport.height,
                size.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the artwork to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// covers than the length, so the count goes to the outline instead.
pub(super) fn cover_count_note(cover_count: Option<u32>, preview_covers: Option<usize>) -> String {
    let Some(count) = cover_count else {
        return String::new();
    };
    match preview_covers {
        Some(_) => {
            format!("The user asked for {count} covers. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} covers. Write exactly {count} covers.\n"),
    }
}

/// The user prompt for one artwork candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
pub(super) fn artwork_candidate_prompt(request: &ArtworkCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build an artwork for this request. The request and the answers are \
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
    if let Some(count) = request.preview_covers {
        prompt.push_str(&artwork_preview_note(count));
    }
    prompt.push_str(&cover_count_note(
        options.cover_count,
        request.preview_covers,
    ));
    prompt.push_str(&size_note(options.cover_size.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("artwork", merge));
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
        "low" => prompt.push_str("Keep the artwork concise: fewer covers, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the artwork JSON.");
    prompt
}

/// The user prompt for an artwork edit: the artwork as it is, the
/// request, and the change the user asked for.
pub(super) fn artwork_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the artwork to change:\n{artwork_json}\n{note}\
         The artwork is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [cover 3, node 0/1 <h2.title>: What changed] names a cover \
         (1-based) and one element in that cover's html by its index path from the cover root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [cover 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one cover the same way, without their text. A reference like [cover 3] \
         names the cover alone: the change is about that cover. Change only what the critique asks for. Keep every other cover and \
         value as it is. Return every changed cover complete: html, css, and notes.\n{format}",
        artwork_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The artwork as a focused edit sees it: the title, the theme, the
/// size, the cover count, and only the covers at `indexes`, each
/// with its index.
pub(super) fn focused_artwork_json(
    artwork: &Artwork,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let covers: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            artwork.covers.get(*index).map(|cover| {
                let cover = if is_fresh {
                    fresh_cover(cover)
                } else {
                    cover.clone()
                };
                serde_json::json!({ "index": index, "cover": cover })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": artwork.title,
        "theme": artwork.theme,
        "size": artwork.size,
        "cover_count": artwork.covers.len(),
        "covers": covers,
    }))
}

/// The cover as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
pub(super) fn fresh_cover(cover: &Cover) -> Cover {
    Cover {
        html: String::new(),
        css: None,
        ..cover.clone()
    }
}

/// The user prompt for one artwork continuation chunk: the preview
/// artwork and the chunk's covers to add, as a patch of inserts.
pub(super) fn artwork_continue_prompt(
    request: &SessionRequest,
    artwork: &Artwork,
    artwork_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = artwork.covers.len();
    let planned = artwork.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = artwork
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is an artwork in progress: its theme, its size, its first {written} \
         covers, and `outline`, the cover titles of the complete artwork:\n{artwork_json}\n\
         The artwork is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} covers: outline titles {} to {last} of {planned}, in order, one cover per \
         title:\n{}\n\
         Keep the theme. Match the existing covers in CSS style, font sizes, spacing, colors, \
         and visual language, so the artwork reads as one piece. Do not change or repeat \
         the existing covers.\n",
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
        "Reply with only a JSON patch that appends the new covers, not the whole artwork:\n\
         {{\"covers\":[{{\"index\":{written},\"insert\":true,\"cover\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new cover index {written} and insert true, in priority order. Each cover \
         carries html, css, and notes. Omit title, theme, size, outline, and the existing covers."
    ));
    prompt
}

/// Extracts and parses the artwork JSON from a model reply.
pub(super) fn parse_artwork(content: &str) -> Result<Artwork, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid artwork: {error}"))
}
