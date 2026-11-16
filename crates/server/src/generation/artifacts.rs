use super::*;

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
pub(crate) fn wrote_summary(design_ids: &[String]) -> String {
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
