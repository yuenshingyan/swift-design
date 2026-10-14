use super::*;

/// The deck system prompt: role, deck rules, the deck schema, the
/// clarification protocol, and one example deck.
pub(super) fn deck_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Deck)).unwrap_or_default();
    format!(
        "You build slide decks as JSON documents. Each slide is one HTML fragment plus its own CSS, \
         for a 1920 by 1080 px canvas.\n\
         Follow these rules:\n{rules}\n\
         The deck must conform to this JSON Schema:\n{schema}\n\
         Example deck:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one deck JSON document. No prose, no code fences.",
        rules = DECK_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-deck.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` slides and
/// the full outline.
pub(super) fn deck_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} slides of the deck, in order, starting with the \
         title slide. Put the slide titles of the complete deck in `outline`, in order, every \
         slide title of the complete deck. The app asks you for the remaining slides later. \
         Make these {count} slides show the theme, the layout language, and the text density \
         of the whole deck.\n"
    )
}

/// The prompt line that holds the deck to the length the user asked
/// for. Empty when the user set no length. A preview writes fewer
/// slides than the length, so the count goes to the outline instead.
/// The prompt line for the app's scenario choice. Empty when the
/// agent decides.
pub(super) fn scenario_note(scenario: Option<&str>) -> String {
    match scenario.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!(
            "The deck is for this scenario: {name}. Use the vocabulary, the examples, and the tone that fit it.\n"
        ),
        None => String::new(),
    }
}

pub(super) fn slide_count_note(slide_count: Option<u32>, preview_slides: Option<usize>) -> String {
    let Some(count) = slide_count else {
        return String::new();
    };
    match preview_slides {
        Some(_) => {
            format!("The user asked for {count} slides. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} slides. Write exactly {count} slides.\n"),
    }
}

/// The user prompt for one deck candidate: the request and the answers
/// are authoritative, plus the template, preview, concept, and effort
/// notes.
pub(super) fn deck_candidate_prompt(request: &DeckCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a deck for this request. The request and the answers are authoritative; do not \
         override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are slides of another deck. Use them for the look only.\n",
        );
    }
    if let Some(count) = request.preview_slides {
        prompt.push_str(&deck_preview_note(count));
    }
    prompt.push_str(&slide_count_note(
        options.slide_count,
        request.preview_slides,
    ));
    prompt.push_str(&scenario_note(options.scenario.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("deck", merge));
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
        "low" => prompt.push_str("Keep the deck concise: fewer slides, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the deck JSON.");
    prompt
}

/// The user prompt for a deck edit: the deck as it is, the request, and
/// the change the user asked for.
pub(super) fn deck_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the deck to change:\n{deck_json}\n{note}\
         The deck is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [slide 3, node 0/1 <h2.title>: What Swift Design does] names a slide \
         (1-based) and one element in that slide's html by its index path from the slide root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [slide 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one slide the same way, without their text. A reference like [slide 3] \
         names the slide alone: the change is about that slide. Change only what the critique asks for. Keep every other slide and \
         value as it is. Return every changed slide complete: html, css, and notes.\n{format}",
        deck_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The deck as a focused edit sees it: the title, the theme, the slide
/// count, and only the slides at `indexes`, each with its index.
pub(super) fn focused_deck_json(
    deck: &Deck,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let slides: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            deck.slides.get(*index).map(|slide| {
                let slide = if is_fresh {
                    fresh_slide(slide)
                } else {
                    slide.clone()
                };
                serde_json::json!({ "index": index, "slide": slide })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": deck.title,
        "theme": deck.theme,
        "slide_count": deck.slides.len(),
        "slides": slides,
    }))
}

/// The slide as a regenerate shows it: its name and notes, without its
/// markup, so the model writes it anew instead of tweaking it.
pub(super) fn fresh_slide(slide: &Slide) -> Slide {
    Slide {
        html: String::new(),
        css: None,
        ..slide.clone()
    }
}

/// The user prompt for one deck continuation chunk: the preview deck and
/// the chunk's slides to add, as a patch of inserts.
pub(super) fn deck_continue_prompt(
    request: &SessionRequest,
    deck: &Deck,
    deck_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = deck.slides.len();
    let planned = deck.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = deck
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a deck in progress: its theme, its first {written} slides, and `outline`, the \
         slide titles of the complete deck:\n{deck_json}\n\
         The deck is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} slides: outline titles {} to {last} of {planned}, in order, one slide per \
         title:\n{}\n\
         Keep the theme. Match the existing slides in CSS style, font sizes, spacing, colors, \
         and visual language, so the deck reads as one deck. Do not change or repeat the \
         existing slides.\n",
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
        "Reply with only a JSON patch that appends the new slides, not the whole deck:\n\
         {{\"slides\":[{{\"index\":{written},\"insert\":true,\"slide\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new slide index {written} and insert true, in presentation order. Each slide \
         carries html, css, and notes. Omit title, theme, outline, and the existing slides."
    ));
    prompt
}

/// Extracts and parses the deck JSON from a model reply.
pub(super) fn parse_deck(content: &str) -> Result<Deck, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid deck: {error}"))
}
