//! What a run works from: the request, the answers so far, and the
//! app's own choices. There is no brief: the model reads the request
//! and the answers, as Swift Deck did.

use design_model::{
    AnsweredQuestion, ArtifactKind, BriefQuestion, QuestionAnswer, Viewport, axis_label,
};

use crate::sessions::RunOptions;

/// The input of one run.
#[derive(Clone, Debug)]
pub(crate) struct SessionRequest {
    /// The user's request, in their words.
    pub(crate) request: String,
    /// `demo` or `deck`.
    pub(crate) kind: ArtifactKind,
    /// Every answered question so far, oldest first.
    pub(crate) answers: Vec<AnsweredQuestion>,
    /// The app's choices: canvases, variations, slides, scenario.
    pub(crate) options: RunOptions,
}

impl SessionRequest {
    /// Every platform a demo run builds for, never empty. An empty name
    /// stands for the default desktop canvas.
    pub(crate) fn platforms(&self) -> Vec<String> {
        let listed: Vec<String> = self
            .options
            .platforms
            .iter()
            .filter(|platform| !platform.trim().is_empty())
            .cloned()
            .collect();
        if listed.is_empty() {
            return vec![String::new()];
        }
        listed
    }

    /// The canvas of every platform the run builds for, in order.
    pub(crate) fn viewports(&self) -> Vec<Viewport> {
        self.platforms()
            .iter()
            .map(|platform| Viewport::for_platform(platform))
            .collect()
    }
}

/// The request, the app's choices, and the answers, rendered as
/// labelled lines for a generation prompt.
/// What a wireframe is, for the prompt. The label alone reads as a
/// style; this line says what to draw and what to leave out.
const WIREFRAME_NOTE: &str = "A wireframe shows the layout only. Use one neutral gray palette. \
     Draw a gray block with a label in place of every image, logo, and chart. Use one \
     system font. Keep the real copy and the real labels. Add no decoration.\n";

pub(crate) fn request_input(request: &SessionRequest) -> String {
    let mut input = format!("Request:\n{}\n", request.request.trim());
    input.push_str(&format!("Kind: {}\n", request.kind.label()));
    match request.kind {
        ArtifactKind::Demo => {
            let canvases: Vec<String> = request
                .viewports()
                .iter()
                .map(|viewport| format!("{} by {} px", viewport.width, viewport.height))
                .collect();
            input.push_str(&format!("Canvases: {}\n", canvases.join(", ")));
        }
        ArtifactKind::Deck => {
            if let Some(scenario) = &request.options.scenario {
                input.push_str(&format!("Scenario: {scenario}\n"));
            }
            if let Some(count) = request.options.slide_count {
                input.push_str(&format!("Slide count the user asked for: {count}\n"));
            }
        }
    }
    // The app's own answers, which recur in every session. An axis
    // the user has not picked is absent, so the agent decides it. A
    // picked value is a slug that the prompt reads as its label; an
    // answer the user typed has no label and is printed as it was
    // typed.
    for (name, value) in request.options.axes(request.kind) {
        let text = axis_label(name, value).unwrap_or(value);
        input.push_str(&format!("{name}: {text}\n"));
    }
    if request.options.fidelity.as_deref() == Some("wireframe") {
        input.push_str(WIREFRAME_NOTE);
    }
    if !request.answers.is_empty() {
        input.push_str("Answers from the user:\n");
        for entry in &request.answers {
            let answer = if entry.is_assumed {
                "use your best judgment"
            } else {
                entry.answer.as_str()
            };
            input.push_str(&format!("- {} -> {answer}\n", entry.question));
        }
    }
    input
}

/// The answered questions as text entries: the question wording and
/// the answer wording, kept apart.
pub(crate) fn answered_questions_from_answers(
    answered: &[(BriefQuestion, QuestionAnswer)],
) -> Vec<AnsweredQuestion> {
    answered
        .iter()
        .map(|(question, answer)| AnsweredQuestion {
            question: question.label.clone(),
            answer: if answer.skipped {
                String::new()
            } else {
                answer_text(question, answer)
            },
            is_assumed: answer.skipped,
        })
        .collect()
}

/// The readable text of one answer: option labels joined, plus any
/// other text.
fn answer_text(question: &BriefQuestion, answer: &QuestionAnswer) -> String {
    let mut parts: Vec<String> = answer
        .values
        .iter()
        .map(|value| {
            question
                .options
                .iter()
                .find(|option| &option.value == value)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| value.clone())
        })
        .collect();
    if let Some(other) = &answer.other_text
        && !other.trim().is_empty()
    {
        parts.push(other.trim().to_owned());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use design_model::{QuestionKind, QuestionOption};

    use super::*;

    fn request(kind: ArtifactKind) -> SessionRequest {
        SessionRequest {
            request: "Intro for Swift Design.".to_owned(),
            kind,
            answers: vec![AnsweredQuestion {
                question: "Who is the audience?".to_owned(),
                answer: "New users".to_owned(),
                is_assumed: false,
            }],
            options: RunOptions::default(),
        }
    }

    #[test]
    fn a_demo_input_names_the_canvases_and_the_answers() {
        let input = request_input(&request(ArtifactKind::Demo));
        assert!(input.contains("Request:\nIntro for Swift Design."));
        assert!(input.contains("Canvases: 1440 by 900 px"));
        assert!(input.contains("- Who is the audience? -> New users"));
    }

    #[test]
    fn a_deck_input_names_the_scenario_and_the_slide_count() {
        let mut deck = request(ArtifactKind::Deck);
        deck.options.scenario = Some("Training".to_owned());
        deck.options.slide_count = Some(8);
        let input = request_input(&deck);
        assert!(input.contains("Scenario: Training"));
        assert!(input.contains("Slide count the user asked for: 8"));
        assert!(!input.contains("Canvases"));
    }

    #[test]
    fn the_apps_own_answers_reach_the_prompt() {
        let mut demo = request(ArtifactKind::Demo);
        demo.options.scope = Some("short_flow".to_owned());
        demo.options.color_mode = Some("dark".to_owned());
        demo.options.product_kind = Some("developer_tool".to_owned());
        demo.options.data_state = Some("populated".to_owned());
        let input = request_input(&demo);
        // The stored value is a slug; the prompt reads the label.
        assert!(input.contains("Color mode: Dark and sleek"));
        assert!(input.contains("Scope: A short flow of screens"));
        assert!(input.contains("Product kind: Developer tool"));
        assert!(input.contains("Screen data: Filled with realistic data"));
        assert!(!input.contains("Fidelity"));
        assert!(!input.contains("A wireframe shows"));
        let mut deck = request(ArtifactKind::Deck);
        deck.options.audience = Some("practitioners".to_owned());
        deck.options.tone = Some("technical".to_owned());
        let input = request_input(&deck);
        assert!(input.contains("Audience: Practitioners in the field"));
        assert!(input.contains("Tone: Technical and precise"));
    }

    #[test]
    fn a_wireframe_pick_says_what_a_wireframe_is() {
        let mut demo = request(ArtifactKind::Demo);
        demo.options.fidelity = Some("wireframe".to_owned());
        let input = request_input(&demo);
        assert!(input.contains("Fidelity: Wireframe, gray boxes"));
        assert!(input.contains("A wireframe shows the layout only."));
        // The finished look is the default and needs no note.
        demo.options.fidelity = Some("high_fidelity".to_owned());
        let input = request_input(&demo);
        assert!(input.contains("Fidelity: Finished, high fidelity"));
        assert!(!input.contains("A wireframe shows"));
    }

    #[test]
    fn a_demo_prints_no_audience_and_no_tone() {
        // A demo does not ask them, so an old record's value stays out
        // of the prompt.
        let mut demo = request(ArtifactKind::Demo);
        demo.options.audience = Some("practitioners".to_owned());
        demo.options.tone = Some("technical".to_owned());
        let input = request_input(&demo);
        assert!(!input.contains("Audience:"));
        assert!(!input.contains("Tone:"));
    }

    #[test]
    fn a_deck_gets_the_deck_axes_and_a_demo_gets_the_demo_ones() {
        let mut deck = request(ArtifactKind::Deck);
        deck.options.slide_density = Some("sparse".to_owned());
        deck.options.evidence_style = Some("data_heavy".to_owned());
        // A demo-only value on a deck reaches no prompt line.
        deck.options.product_kind = Some("developer_tool".to_owned());
        let input = request_input(&deck);
        assert!(input.contains("Slide density: One idea in large type"));
        assert!(input.contains("Evidence: Data-heavy throughout"));
        assert!(!input.contains("Product kind"));
    }

    #[test]
    fn an_unpicked_axis_is_left_out_so_the_agent_decides_it() {
        let input = request_input(&request(ArtifactKind::Demo));
        for axis in [
            "Audience:",
            "Tone:",
            "Color mode:",
            "Scope:",
            "Product kind:",
        ] {
            assert!(!input.contains(axis), "{axis}");
        }
    }

    #[test]
    fn a_deck_says_its_size_with_slides_not_scope() {
        let mut deck = request(ArtifactKind::Deck);
        deck.options.audience = Some("newcomers".to_owned());
        // Scope is a demo question; a deck must not print it even when
        // an old record carries one.
        deck.options.scope = Some("short_flow".to_owned());
        let input = request_input(&deck);
        assert!(input.contains("Audience: Newcomers to the subject"));
        assert!(!input.contains("Scope:"));
    }

    #[test]
    fn an_answer_the_user_typed_reaches_the_prompt_as_typed() {
        let mut deck = request(ArtifactKind::Deck);
        // No list carries this, so there is no label to read: the
        // user's own words are the answer.
        deck.options.audience = Some("astronauts".to_owned());
        assert!(request_input(&deck).contains("Audience: astronauts"));
    }

    #[test]
    fn platforms_fall_back_to_the_default_canvas() {
        let mut demo = request(ArtifactKind::Demo);
        assert_eq!(demo.platforms(), vec![String::new()]);
        demo.options.platforms = vec!["phone".to_owned(), " ".to_owned()];
        assert_eq!(demo.platforms(), vec!["phone".to_owned()]);
        assert_eq!(demo.viewports()[0], Viewport::for_platform("phone"));
    }

    #[test]
    fn answers_read_as_option_labels_and_other_text() {
        let question = BriefQuestion {
            id: "audience".to_owned(),
            label: "Who?".to_owned(),
            rationale: None,
            kind: QuestionKind::SingleSelect,
            required: false,
            options: vec![QuestionOption {
                value: "new".to_owned(),
                label: "New users".to_owned(),
            }],
            allow_other: true,
        };
        let answer = QuestionAnswer {
            question_id: "audience".to_owned(),
            values: vec!["new".to_owned()],
            other_text: Some("and partners".to_owned()),
            skipped: false,
        };
        let entries = answered_questions_from_answers(&[(question.clone(), answer)]);
        assert_eq!(entries[0].answer, "New users, and partners");
        let skipped = QuestionAnswer {
            question_id: "audience".to_owned(),
            skipped: true,
            ..QuestionAnswer::default()
        };
        let entries = answered_questions_from_answers(&[(question, skipped)]);
        assert!(entries[0].is_assumed);
        assert_eq!(entries[0].answer, "");
    }
}
