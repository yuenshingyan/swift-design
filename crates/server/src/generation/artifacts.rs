use super::*;

/// The system prompt: role, rules, schema, the clarification protocol,
/// and one example design.
pub(super) fn system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Design)).unwrap_or_default();
    format!(
        "You build screen designs as JSON documents. Each screen is one HTML fragment plus its own CSS, \
         for the px canvas the design's `viewport` names.\n\
         Follow these rules:\n{rules}\n\
         The design must conform to this JSON Schema:\n{schema}\n\
         Example design:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one design JSON document. No prose, no code fences.",
        rules = DEMO_RULES.join("\n"),
        example = include_str!("../../../../fixtures/sample-design.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// A brief question set the model returned instead of a design, when
/// the reply carries a `needs_clarification` object. `Some(Err)` when
/// the object is present but invalid.
pub(crate) fn clarification_request(
    content: &str,
) -> Option<Result<BriefQuestionSet, Vec<QuestionSetError>>> {
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    if end <= start {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Wrapper {
        needs_clarification: BriefQuestionSet,
    }
    let wrapper: Wrapper = serde_json::from_str(&content[start..=end]).ok()?;
    let set = wrapper.needs_clarification;
    let problems = validate_question_set(&set);
    Some(if problems.is_empty() {
        Ok(set)
    } else {
        Err(problems)
    })
}

/// Turns a generation stop into a plain message, for callers that do not
/// carry a clarification.
pub(crate) fn stop_to_string(stop: GenerationStop) -> String {
    match stop {
        GenerationStop::Failed(message) => message,
        GenerationStop::NeedsClarification(_) => {
            "the model asked for clarification mid-continuation".to_owned()
        }
    }
}

/// The assistant summary after a run wrote designs.
pub(super) fn wrote_summary(design_ids: &[String]) -> String {
    match design_ids.len() {
        0 => "The run wrote no designs.".to_owned(),
        1 => format!(
            "I wrote `{}`. Open it, then ask for a change in the chat.",
            design_ids[0]
        ),
        count => format!(
            "I wrote {count} candidates to the canvas. Pick one, then ask for a change in the chat."
        ),
    }
}

/// Turns a model reply into an artifact: a whole design or deck, or a
/// patch applied to an existing one.
pub(crate) type ReplyParser<'request, T> =
    Box<dyn Fn(&str) -> Result<T, String> + Send + Sync + 'request>;

/// An artifact the fix-round loop can validate: a design or a deck.
pub(crate) trait Validated {
    /// Every validation problem, empty when the artifact is ready.
    fn problems(&self) -> Vec<design_model::ValidationError>;
}

impl Validated for Design {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Deck {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Document {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Social {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Print {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Mailing {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Campaign {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

impl Validated for design_model::Artwork {
    fn problems(&self) -> Vec<design_model::ValidationError> {
        self.validate()
    }
}

/// Effort, log label, and reply parser for one artifact request: a
/// candidate (the reply is the artifact) or an edit (the reply is a
/// patch).
pub(crate) struct ArtifactRequest<'request, T> {
    pub(crate) effort: String,
    pub(crate) label: String,
    pub(crate) parse: ReplyParser<'request, T>,
    /// Where this request reports its share of the turn: `DRAFT_SHARE`
    /// once the draft validates, then up to 1.0 over the polish rounds.
    pub(crate) progress: Option<ShareSink>,
    /// Gets the accumulated reply text while it streams, so the caller
    /// can show partial results.
    pub(crate) live: Option<Arc<TextSink>>,
}

impl<T> ArtifactRequest<'_, T> {
    /// Reports this request's share, when it has a sink.
    pub(crate) fn report(&self, fraction: f32) {
        if let Some(progress) = &self.progress {
            progress(fraction);
        }
    }
}

/// The user prompt for an edit: the design as it is, the request, and
/// the change the user asked for.
pub(super) fn edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the design to change:\n{design_json}\n{note}\
         The design is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [screen 3, node 0/1 <h2.title>: What Swift Design does] names a screen \
         (1-based) and one element in that screen's html by its index path from the screen root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [screen 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one screen the same way, without their text. A reference like [screen 3] \
         names the screen alone: the change is about that screen. Change only what the critique asks for. Keep every other screen and \
         value as it is. Preserve the candidate's shared app_window bounds and every shell marker. \
         Do not resize the window to fit one screen's content. Keep window chrome, header height, \
         and sidebar width consistent across screens. Return every changed screen complete: html, css, and notes.\n{format}",
        design_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = PATCH_FORMAT
    )
}

/// The design as a focused edit sees it: the title, the theme, the
/// viewport, the screen count, and only the screens at `indexes`, each
/// with its index.
pub(super) fn focused_design_json(
    design: &Design,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let screens: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            design.screens.get(*index).map(|screen| {
                let screen = if is_fresh {
                    fresh_screen(screen)
                } else {
                    screen.clone()
                };
                serde_json::json!({ "index": index, "screen": screen })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": design.title,
        "theme": design.theme,
        "viewport": design.viewport,
        "app_window": design.app_window,
        "screen_count": design.screens.len(),
        "screens": screens,
    }))
}

/// The screen as a regenerate shows it: its name and notes, without
/// its markup, so the model writes it anew instead of tweaking it.
pub(super) fn fresh_screen(screen: &design_model::Screen) -> design_model::Screen {
    design_model::Screen {
        html: String::new(),
        css: None,
        ..screen.clone()
    }
}

/// What one candidate call needs: the run context, the candidate number,
/// every concept, so the prompt can name its own and the others, and
/// the preview length when the candidate is a preview.
pub(super) struct CandidateRequest<'request> {
    pub(super) context: &'request GenerationContext,
    pub(super) candidate_number: usize,
    /// The canvas this candidate is written for. One platform the user
    /// picked, not always the first.
    pub(super) viewport: design_model::Viewport,
    pub(super) concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` screens plus the outline.
    pub(super) preview_screens: Option<usize>,
    /// The id the candidate is saved under.
    pub(super) design_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    pub(super) template: Option<&'request Template>,
    /// The candidates to combine, when this candidate is a merge.
    pub(super) merge: Option<&'request MergeInput>,
}

/// The prompt lines for a preview candidate: write `count` screens and
/// the full outline.
pub(super) fn preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} screens of the design, in order, starting with the \
         title screen. Put the screen titles of the complete design in `outline`, in order, \
         every screen title of the complete design. The app asks you for the remaining screens \
         later. Make these {count} screens show the theme, the layout language, and the text \
         density of the whole design.\n"
    )
}

/// The user prompt for one continuation chunk: the preview design and
/// the chunk's screens to add, as a patch of inserts.
pub(super) fn continue_prompt(
    request: &SessionRequest,
    design: &Design,
    design_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = design.screens.len();
    let planned = design.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = design
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a design in progress: its theme, its first {written} screens, and `outline`, the \
         screen titles of the complete design:\n{design_json}\n\
         The design is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} screens: outline titles {} to {last} of {planned}, in order, one screen per \
         title:\n{}\n\
         Keep the theme and app_window. When app_window is set, mark exactly one top-level \
         shell element with data-swift-design-window on every new screen. Match window chrome, \
         header height, and sidebar width. Match the existing screens in CSS style, font sizes, spacing, colors, \
         and visual language, so the design reads as one design. Do not change or repeat the \
         existing screens.\n",
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
        "Reply with only a JSON patch that appends the new screens, not the whole design:\n\
         {{\"screens\":[{{\"index\":{written},\"insert\":true,\"screen\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new screen index {written} and insert true, in presentation order. Each screen \
         carries html, css, and notes. Omit title, theme, outline, and the existing screens."
    ));
    prompt
}

/// The new screens in a continuation reply, in order. Accepts a patch
/// (the screens of its operations at or past the existing screens) and,
/// as a fallback, a whole design (its screens past the existing ones).
/// Existing screens are never touched.
pub(super) fn continuation_screens(
    written: usize,
    content: &str,
) -> Result<Vec<design_model::Screen>, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    let value: serde_json::Value = serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid JSON: {error}"))?;
    let items = value
        .get("screens")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no screens array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("screen").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("screen"))
            .filter(|screen| screen.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, screen)| {
            serde_json::from_value::<design_model::Screen>(screen.clone()).map_err(|error| {
                format!(
                    "new screen {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new screens to the design in progress. The outline
/// stays until every title has a screen, so a short reply leaves the
/// design continuable.
pub(super) fn apply_continuation(original: &Design, content: &str) -> Result<Design, String> {
    let new_screens = continuation_screens(original.screens.len(), content)?;
    if new_screens.is_empty() {
        return Err(
            "the reply adds no screens: reply with a patch of inserts, one per new screen"
                .to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.screens.extend(new_screens);
    if continued.screens.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The message for a run that produced nothing.
///
/// `failures` is empty when every task reported success and the work
/// still is not there, which is a bug in this engine rather than a
/// model failure. An empty message told the user nothing, so each site
/// names itself instead.
pub(crate) fn failure_message(failures: &[String], fallback: &str) -> String {
    if failures.is_empty() {
        return fallback.to_owned();
    }
    failures.join("; ")
}

/// One candidate to write: which variation, which canvas, which id.
///
/// A run writes one design per platform per variation. The variation
/// number picks the concept and the template; the viewport picks the
/// canvas. The id numbers every candidate straight through, so the
/// existing `{base}-candidate-{n}` naming still holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CandidatePlan {
    /// The id the candidate is saved under.
    pub design_id: String,
    /// Position in the whole run, from one.
    pub candidate_number: usize,
    /// Which variation this is, from one.
    pub variation: usize,
    /// The canvas to write for.
    pub viewport: design_model::Viewport,
}

/// Every candidate a run writes: `variations` concepts on each canvas,
/// canvas by canvas, numbered from `first_number`. A run numbers after
/// the candidates the session has, so it adds to them instead of
/// overwriting them.
pub(crate) fn candidate_plans(
    base: &str,
    platforms: &[String],
    variations: usize,
    first_number: usize,
) -> Vec<CandidatePlan> {
    let mut plans = Vec::new();
    for viewport in platforms
        .iter()
        .map(|platform| design_model::Viewport::for_platform(platform))
    {
        for variation in 1..=variations.max(1) {
            let candidate_number = first_number.max(1) + plans.len();
            plans.push(CandidatePlan {
                design_id: candidate_id(base, candidate_number),
                candidate_number,
                variation,
                viewport,
            });
        }
    }
    plans
}

/// The user prompt for one candidate: the request and the answers are
/// authoritative, plus the template, preview, concept, and effort notes.
pub(super) fn candidate_prompt(request: &CandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a design for this request. The request and the answers are authoritative; do not \
         override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
    }
    if let Some(count) = request.preview_screens {
        prompt.push_str(&preview_note(count));
    }
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("design", merge));
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
        "low" => prompt.push_str("Keep the design concise: fewer screens, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    // A run writes one design per platform, and each is drawn for its
    // own canvas.
    let viewport = request.viewport;
    prompt.push_str(&format!(
        "Set `viewport` to {} by {} px. Design every screen for that canvas. \
         For app screens, choose one shared app_window rectangle for this candidate. \
         Mark one top-level shell per screen with data-swift-design-window. \
         Reuse the same window chrome, header height, and sidebar width. \
         Different candidates may choose different window bounds.\n",
        viewport.width, viewport.height
    ));
    prompt.push_str("Reply with only the design JSON.");
    prompt
}

/// The template candidate `candidate_number` takes its look from.
/// Candidates are numbered from one and wrap around the chosen
/// templates, so three looks across five candidates run 1, 2, 3, 1, 2.
pub(crate) fn candidate_template(
    templates: &[Template],
    candidate_number: usize,
) -> Option<Template> {
    if templates.is_empty() {
        return None;
    }
    templates
        .get((candidate_number - 1) % templates.len())
        .cloned()
}

/// The prompt lines for a template: the theme to copy and the screens to
/// match. The template gives the look. The brief gives the content.
pub(crate) fn template_note(template: &Template) -> String {
    let mut note = format!(
        "Use the saved template `{name}` for the look of this design.\n\
         Copy this theme into the design exactly. Use it instead of any palette or fonts \
         named elsewhere in this prompt:\n{theme}\n",
        name = template.name,
        theme = serde_json::to_string(&template.theme).unwrap_or_default(),
    );
    if let Some(style_note) = template
        .note
        .as_deref()
        .filter(|note| !note.trim().is_empty())
    {
        note.push_str(&format!(
            "The template's style note, from its brand material. Follow it:\n{style_note}\n"
        ));
    }
    if !template.screens.is_empty() {
        note.push_str(
            "These screens show the template style. Match their CSS: the same font sizes, \
             spacing, alignment, colors, and layout language. Write new content for the \
             request. Do not copy their text.\n",
        );
    }
    for (index, screen) in template.screens.iter().enumerate() {
        let Ok(json) = serde_json::to_string(screen) else {
            continue;
        };
        note.push_str(&format!("Template screen {}: {json}\n", index + 1));
    }
    note
}

/// Extracts and parses the design JSON from a model reply.
pub(super) fn parse_design(content: &str) -> Result<Design, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid design: {error}"))
}
