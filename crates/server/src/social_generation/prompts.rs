use super::*;

/// The social system prompt: role, social rules, the social
/// schema, the clarification protocol, and one example social.
pub(super) fn social_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Social)).unwrap_or_default();
    format!(
        "You build social posts and carousels as JSON socials, for Instagram, LinkedIn, X, and Facebook. \
         Each frame is one HTML fragment plus its own CSS, for the px canvas of the social's format: \
         1080 by 1080 px for square, 1080 by 1350 px for portrait, 1080 by 1920 px for story, \
         1200 by 630 px for landscape. One frame is a single post. Two or more frames are a carousel.\n\
         Follow these rules:\n{rules}\n\
         The social must conform to this JSON Schema:\n{schema}\n\
         Example social:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one social JSON. No prose, no code fences.",
        rules = SOCIAL_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-social.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` frames and
/// the full outline.
pub(super) fn social_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} frames of the social, in order, starting with \
         the first frame. Put the frame titles of the complete social in `outline`, in order, \
         every frame title of the complete social. The app asks you for the remaining frames \
         later. Make these {count} frames show the theme, the layout language, and the text \
         density of the whole social.\n"
    )
}

/// The prompt line for the app's format choice. Empty when the agent
/// decides, or when the user typed a format the JSON does not carry.
pub(super) fn format_note(format: Option<&str>) -> String {
    match format.and_then(Format::from_name) {
        Some(format) => {
            let viewport = format.viewport();
            format!(
                "Lay the frames out on the {} format: {} by {} px. Set `format` to `{}`.\n",
                format.as_str(),
                viewport.width,
                viewport.height,
                format.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the social to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// frames than the length, so the count goes to the outline instead.
pub(super) fn frame_count_note(frame_count: Option<u32>, preview_frames: Option<usize>) -> String {
    let Some(count) = frame_count else {
        return String::new();
    };
    match preview_frames {
        Some(_) => {
            format!("The user asked for {count} frames. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} frames. Write exactly {count} frames.\n"),
    }
}

/// The user prompt for one social candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
pub(super) fn social_candidate_prompt(request: &SocialCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a social post or carousel for this request. The request and the answers are \
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
    if let Some(count) = request.preview_frames {
        prompt.push_str(&social_preview_note(count));
    }
    prompt.push_str(&frame_count_note(
        options.frame_count,
        request.preview_frames,
    ));
    prompt.push_str(&format_note(options.format.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("social", merge));
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
        "low" => prompt.push_str("Keep the social concise: fewer frames, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the social JSON.");
    prompt
}

/// The user prompt for a social edit: the social as it is, the
/// request, and the change the user asked for.
pub(super) fn social_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the social to change:\n{social_json}\n{note}\
         The social is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [frame 3, node 0/1 <h2.title>: What changed] names a frame \
         (1-based) and one element in that frame's html by its index path from the frame root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [frame 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one frame the same way, without their text. A reference like [frame 3] \
         names the frame alone: the change is about that frame. Change only what the critique asks for. Keep every other frame and \
         value as it is. Return every changed frame complete: html, css, and notes.\n{format}",
        social_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The social as a focused edit sees it: the title, the theme, the
/// format, the frame count, and only the frames at `indexes`, each with
/// its index.
pub(super) fn focused_social_json(
    social: &Social,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let frames: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            social.frames.get(*index).map(|frame| {
                let frame = if is_fresh {
                    fresh_frame(frame)
                } else {
                    frame.clone()
                };
                serde_json::json!({ "index": index, "frame": frame })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": social.title,
        "theme": social.theme,
        "format": social.format,
        "frame_count": social.frames.len(),
        "frames": frames,
    }))
}

/// The frame as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
pub(super) fn fresh_frame(frame: &Frame) -> Frame {
    Frame {
        html: String::new(),
        css: None,
        ..frame.clone()
    }
}

/// The user prompt for one social continuation chunk: the preview
/// social and the chunk's frames to add, as a patch of inserts.
pub(super) fn social_continue_prompt(
    request: &SessionRequest,
    social: &Social,
    social_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = social.frames.len();
    let planned = social.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = social
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a social in progress: its theme, its format, its first {written} frames, and \
         `outline`, the frame titles of the complete social:\n{social_json}\n\
         The social is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} frames: outline titles {} to {last} of {planned}, in order, one frame per \
         title:\n{}\n\
         Keep the theme. Match the existing frames in CSS style, font sizes, spacing, colors, \
         and visual language, so the social reads as one social. Do not change or repeat \
         the existing frames.\n",
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
        "Reply with only a JSON patch that appends the new frames, not the whole social:\n\
         {{\"frames\":[{{\"index\":{written},\"insert\":true,\"frame\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new frame index {written} and insert true, in reading order. Each frame \
         carries html, css, and notes. Omit title, theme, format, outline, and the existing frames."
    ));
    prompt
}

/// Extracts and parses the social JSON from a model reply.
pub(super) fn parse_social(content: &str) -> Result<Social, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid social: {error}"))
}
