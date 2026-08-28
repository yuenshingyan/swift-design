//! The planner: one turn of the conversation, as Swift Deck did it.
//! The model reads the request, the answers, and the conversation, and
//! replies with questions, a decision to write, a decision to edit the
//! open artifact, or plain text.

use design_model::{
    ArtifactKind, BriefQuestion, BriefQuestionSet, QUESTIONS_PER_TURN_LIMIT, QuestionKind,
    QuestionOption,
};

use crate::request::{SessionRequest, request_input};
use crate::sessions::ChatMessage;

/// After this many answered questions the planner asks no more.
pub(crate) const ANSWERED_QUESTION_LIMIT: usize = 5;

/// The title of a planner question set.
const QUESTION_SET_TITLE: &str = "A few questions first";

/// One planned turn, parsed from the model's reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Text for the user. Empty means nothing to say.
    pub(crate) reply: String,
    /// The questions to ask, at most `QUESTIONS_PER_TURN_LIMIT`.
    pub(crate) question_set: Option<BriefQuestionSet>,
    /// True when the model wants to write candidates now.
    pub(crate) should_generate: bool,
    /// True when the model wants to apply the request to the artifact
    /// open in the editor.
    pub(crate) should_edit: bool,
}

/// One question as the model writes it: a label, short options, and
/// whether more than one option may be picked.
#[derive(serde::Deserialize)]
struct PlannedQuestion {
    #[serde(default)]
    question: String,
    #[serde(default)]
    options: Vec<String>,
    /// True when the options are not exclusive. Absent means one pick.
    #[serde(default)]
    multi: bool,
}

/// Parses a planner reply. Prose that is not JSON becomes the reply
/// text, so the user still sees it.
pub(crate) fn parse_plan(content: &str) -> Plan {
    #[derive(serde::Deserialize)]
    struct PlanReply {
        #[serde(default)]
        reply: String,
        #[serde(default)]
        questions: Vec<PlannedQuestion>,
        #[serde(default)]
        generate: bool,
        #[serde(default)]
        edit: bool,
    }
    let parsed = content
        .find('{')
        .zip(content.rfind('}'))
        .filter(|(start, end)| end > start)
        .and_then(|(start, end)| serde_json::from_str::<PlanReply>(&content[start..=end]).ok());
    let Some(parsed) = parsed else {
        return Plan {
            reply: content.trim().to_owned(),
            ..Plan::default()
        };
    };
    let questions: Vec<BriefQuestion> = parsed
        .questions
        .into_iter()
        .filter(|question| !question.question.trim().is_empty())
        .take(QUESTIONS_PER_TURN_LIMIT)
        .enumerate()
        .map(|(index, question)| to_question(index, question))
        .collect();
    let question_set = (!questions.is_empty()).then(|| BriefQuestionSet {
        title: QUESTION_SET_TITLE.to_owned(),
        message: parsed.reply.trim().to_owned(),
        questions,
        can_proceed_with_assumptions: true,
    });
    Plan {
        reply: parsed.reply.trim().to_owned(),
        question_set,
        should_generate: parsed.generate,
        should_edit: parsed.edit,
    }
}

/// One planned question as a studio question: a choice among the
/// options, with an `Other` field, never required. The app adds the
/// `Use your best judgment` choice itself.
///
/// A question the model marks `multi` takes any number of options; the
/// rest take one.
fn to_question(index: usize, question: PlannedQuestion) -> BriefQuestion {
    let options: Vec<QuestionOption> = question
        .options
        .iter()
        .map(|option| option.trim())
        .filter(|option| !option.is_empty())
        .map(|option| QuestionOption {
            value: option.to_owned(),
            label: option.to_owned(),
        })
        .collect();
    let kind = match (options.is_empty(), question.multi) {
        (true, _) => QuestionKind::ShortText,
        (false, true) => QuestionKind::MultiSelect,
        (false, false) => QuestionKind::SingleSelect,
    };
    BriefQuestion {
        id: format!("q{}", index + 1),
        label: question.question.trim().to_owned(),
        rationale: None,
        kind,
        required: false,
        options,
        allow_other: kind.is_select(),
    }
}

/// The set that carries the app's own fixed questions when the planner
/// adds none of its own.
///
/// The app always asks its list first, so the first turn of a session
/// always ends in the question card. The set holds no question of its
/// own: the studio draws the app's cards in the same grid.
pub(crate) fn app_question_set() -> BriefQuestionSet {
    BriefQuestionSet {
        title: QUESTION_SET_TITLE.to_owned(),
        message: "Answer what matters here. Anything you leave is my best judgment.".to_owned(),
        questions: Vec::new(),
        can_proceed_with_assumptions: true,
    }
}

/// Makes a question set skippable the Swift Deck way: no question is
/// required, and the set allows generation without answers. An
/// external agent's own flags are not trusted.
pub(crate) fn relax_question_set(set: &mut BriefQuestionSet) {
    set.can_proceed_with_assumptions = true;
    for question in &mut set.questions {
        question.required = false;
    }
}

/// The planner's system prompt: one turn of the conversation.
pub(crate) fn planner_prompt(kind: ArtifactKind) -> String {
    let (subject, owned) = match kind {
        ArtifactKind::Demo => (
            "You plan software demos with the user: landing pages, app screens, and similar layouts on a device canvas.",
            "the audience, the tone, light or dark, how much of the demo to build, what kind of product it is, what state the screens show, the canvases to build for, and the number of candidates",
        ),
        ArtifactKind::Deck => (
            "You plan slide decks with the user.",
            "the audience, the tone, light or dark, the scenario, the deck length in slides, how much goes on a slide, how much the deck leans on data, the number of candidates, and how different the candidates are",
        ),
    };
    format!(
        "{subject}\n\
         Read the request, the answers, and the conversation. Reply with only this JSON:\n\
         {{\"reply\":\"text for the user\",\"questions\":[{{\"question\":\"...\",\"options\":[\"...\"],\"multi\":false}}],\"generate\":false,\"edit\":false}}\n\
         The app always asks the user its own fixed questions first. Your questions are added to that card.\n\
         Ask 0 to {per_turn} extra questions. Ask none when the request and the source files already say enough. Give 2 to 4 short options for each question you do ask.\n\
         Set multi to true when the user can pick more than one option at once, such as the topics to cover or the sections to include.\n\
         Set multi to false when the options rule each other out, such as the audience or the tone.\n\
         The app asks the user for {owned} itself. Never ask these, and never ask them in other words. The input shows their answers, or `not chosen yet`.\n\
         Ask only what this request raises and the list above does not cover, such as the features to show, the steps of a flow, or the data on a screen.\n\
         Read the user's source files before you ask. Never ask what a source file already answers.\n\
         After {answered} answered questions, do not ask more.\n\
         Set generate to true when you know enough to write. Then say in reply what you will write.\n\
         When the input names an artifact open in the editor and the user asks for a change, set edit to true and generate to false. Say in reply what you will change. The app applies the change to that artifact.\n\
         When no artifact is open and candidates exist and the user asks for changes, set generate to true to write new candidates.\n\
         When the user only chats, set generate to false and answer in reply.\n\
         Keep reply to 1 to 3 sentences. Reply with only the JSON.",
        per_turn = QUESTIONS_PER_TURN_LIMIT,
        answered = ANSWERED_QUESTION_LIMIT,
    )
}

/// What the planner sees: the request and the answers, the app's
/// choices, the canvas state, and the conversation.
pub(crate) fn planner_input(
    request: &SessionRequest,
    messages: &[ChatMessage],
    candidate_count: usize,
    open_artifact: Option<&str>,
) -> String {
    let options = &request.options;
    let mut input = request_input(request);
    let candidates = options
        .variations
        .map_or("not chosen yet".to_owned(), |count| count.to_string());
    match request.kind {
        ArtifactKind::Demo => {
            input.push_str(&format!("Candidates requested: {candidates}\n"));
        }
        ArtifactKind::Deck => {
            input.push_str(&format!(
                "Scenario: {}\nLength in slides: {}\nCandidates requested: {candidates}\nVariety: {}\n",
                options.scenario.as_deref().unwrap_or("not chosen yet"),
                options
                    .slide_count
                    .map_or("not chosen yet".to_owned(), |count| count.to_string()),
                options.variety
            ));
        }
    }
    input.push_str(&format!("Effort: {}\n", options.effort));
    input.push_str(&format!("Candidates on the canvas: {candidate_count}\n"));
    input.push_str(&format!(
        "Artifact open in the editor: {}\n",
        open_artifact.unwrap_or("none")
    ));
    input.push_str(&format!(
        "Questions answered so far: {}\n",
        request.answers.len()
    ));
    if messages.is_empty() {
        input.push_str("Conversation: none yet.\n");
    } else {
        input.push_str("Conversation, oldest first:\n");
        for message in messages {
            match &message.design {
                Some(artifact) => input.push_str(&format!(
                    "{} (editing {artifact}): {}\n",
                    message.role, message.content
                )),
                None => input.push_str(&format!("{}: {}\n", message.role, message.content)),
            }
        }
    }
    input.push_str("Reply with only the JSON.");
    input
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::sessions::RunOptions;

    #[test]
    fn a_json_reply_becomes_a_plan_with_a_question_set() {
        let plan = parse_plan(
            r#"Sure. {"reply":"Two things.","questions":[{"question":"Who is it for?","options":["Developers","Buyers"]},{"question":"","options":[]}],"generate":false,"edit":false}"#,
        );
        assert_eq!(plan.reply, "Two things.");
        let set = plan.question_set.unwrap();
        assert_eq!(set.questions.len(), 1);
        assert_eq!(set.questions[0].id, "q1");
        assert_eq!(set.questions[0].label, "Who is it for?");
        assert_eq!(set.questions[0].kind, QuestionKind::SingleSelect);
        assert!(!set.questions[0].required);
        assert!(set.questions[0].allow_other);
        assert_eq!(set.questions[0].options[1].label, "Buyers");
        assert!(set.can_proceed_with_assumptions);
        assert!(!plan.should_generate);
    }

    #[test]
    fn a_question_marked_multi_takes_several_options() {
        let plan = parse_plan(
            r#"{"questions":[{"question":"What should it cover?","options":["HIG","SwiftUI"],"multi":true}]}"#,
        );
        let question = &plan.question_set.unwrap().questions[0];
        assert_eq!(question.kind, QuestionKind::MultiSelect);
        // `Other` belongs to every select, not only the single-pick one.
        assert!(question.allow_other);
    }

    #[test]
    fn a_question_without_multi_still_takes_one_option() {
        let plan = parse_plan(r#"{"questions":[{"question":"Who for?","options":["A","B"]}]}"#);
        let question = &plan.question_set.unwrap().questions[0];
        assert_eq!(question.kind, QuestionKind::SingleSelect);
        assert!(question.allow_other);
    }

    #[test]
    fn the_prompt_tells_the_model_when_to_allow_several_picks() {
        let prompt = planner_prompt(ArtifactKind::Demo);
        assert!(prompt.contains(r#""multi":false"#));
        assert!(prompt.contains("Set multi to true when the user can pick more than one option"));
        assert!(prompt.contains("Set multi to false when the options rule each other out"));
    }

    #[test]
    fn a_question_without_options_asks_for_text() {
        let plan = parse_plan(r#"{"questions":[{"question":"What is the product name?"}]}"#);
        let set = plan.question_set.unwrap();
        assert_eq!(set.questions[0].kind, QuestionKind::ShortText);
        assert!(!set.questions[0].allow_other);
    }

    #[test]
    fn prose_becomes_the_reply_and_flags_are_kept() {
        let plan = parse_plan("I need more detail before I write.");
        assert_eq!(plan.reply, "I need more detail before I write.");
        assert!(plan.question_set.is_none());
        let plan = parse_plan(r#"{"reply":"Writing now.","generate":true}"#);
        assert!(plan.should_generate);
        let plan = parse_plan(r#"{"reply":"Changing the title.","edit":true}"#);
        assert!(plan.should_edit);
    }

    #[test]
    fn more_than_the_limit_is_cut() {
        let plan = parse_plan(
            r#"{"questions":[{"question":"a"},{"question":"b"},{"question":"c"},{"question":"d"}]}"#,
        );
        assert_eq!(
            plan.question_set.unwrap().questions.len(),
            QUESTIONS_PER_TURN_LIMIT
        );
    }

    #[test]
    fn relaxing_a_set_makes_it_skippable() {
        let mut set = parse_plan(r#"{"questions":[{"question":"Who?","options":["A"]}]}"#)
            .question_set
            .unwrap();
        set.can_proceed_with_assumptions = false;
        set.questions[0].required = true;
        relax_question_set(&mut set);
        assert!(set.can_proceed_with_assumptions);
        assert!(!set.questions[0].required);
    }

    #[test]
    fn the_prompts_name_what_the_app_asks() {
        let demo = planner_prompt(ArtifactKind::Demo);
        assert!(demo.contains("the canvases to build for, and the number of candidates"));
        assert!(demo.contains("what kind of product it is"));
        let deck = planner_prompt(ArtifactKind::Deck);
        assert!(deck.contains("how much goes on a slide"));
        // The recurring axes are the app's, for both kinds. The app
        // asks them itself, and asking nothing on top of them is a
        // valid turn.
        for prompt in [&demo, &deck] {
            assert!(prompt.contains("the audience, the tone, light or dark"));
            assert!(prompt.contains("never ask them in other words"));
            assert!(prompt.contains("the list above does not cover"));
            assert!(prompt.contains("Ask 0 to 3 extra questions"));
            assert!(prompt.contains("asks the user its own fixed questions first"));
            assert!(prompt.contains("Never ask what a source file already answers"));
        }
        assert!(demo.contains("After 5 answered questions, do not ask more."));
        assert!(deck.contains("the scenario, the deck length in slides"));
    }

    #[test]
    fn the_apps_own_set_holds_no_question_and_can_be_skipped() {
        let set = app_question_set();
        assert!(set.questions.is_empty());
        assert!(set.can_proceed_with_assumptions);
        assert_eq!(set.title, QUESTION_SET_TITLE);
        assert!(set.message.contains("best judgment"));
    }

    #[test]
    fn the_input_shows_the_canvas_state_and_the_conversation() {
        let request = SessionRequest {
            request: "A todo app.".to_owned(),
            kind: ArtifactKind::Deck,
            answers: Vec::new(),
            options: RunOptions::default(),
        };
        let messages = vec![
            ChatMessage::user("Make it bolder.", Some("todo-candidate-1")),
            ChatMessage::assistant("Done."),
        ];
        let input = planner_input(&request, &messages, 2, Some("todo-candidate-1"));
        assert!(input.contains("Scenario: not chosen yet"));
        assert!(input.contains("Candidates on the canvas: 2"));
        assert!(input.contains("Artifact open in the editor: todo-candidate-1"));
        assert!(input.contains("user (editing todo-candidate-1): Make it bolder."));
        assert!(input.contains("Questions answered so far: 0"));
        assert!(input.ends_with("Reply with only the JSON."));
    }
}
