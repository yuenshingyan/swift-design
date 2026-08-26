//! The structured question protocol between the agent and the user.
//!
//! The agent asks at most `QUESTIONS_PER_TURN_LIMIT` questions per turn
//! as one `BriefQuestionSet`. The user answers each question with a
//! `QuestionAnswer`, or skips it, which the app records as an
//! assumption. `validate_question_set` and `validate_answers` return
//! every problem, not only the first, so the server can reject bad
//! structured output with a complete message.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Most questions one turn may ask.
pub const QUESTIONS_PER_TURN_LIMIT: usize = 3;

/// How the user answers a question.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// One choice from `options`.
    SingleSelect,
    /// Any number of choices from `options`.
    MultiSelect,
    /// One line of text.
    ShortText,
    /// A paragraph of text.
    LongText,
}

impl QuestionKind {
    /// True for the two kinds that carry `options`.
    pub fn is_select(self) -> bool {
        matches!(self, QuestionKind::SingleSelect | QuestionKind::MultiSelect)
    }
}

/// One preset answer for a select question.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionOption {
    /// The stored value, such as `mobile_app`.
    pub value: String,
    /// The text the user sees, such as `Mobile app`.
    pub label: String,
}

/// One question the agent asks the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BriefQuestion {
    /// A short id unique in the set, such as `platform`.
    pub id: String,
    /// The question text.
    pub label: String,
    /// Why the answer changes the design. Shown under the question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// How the user answers.
    pub kind: QuestionKind,
    /// True when the user must answer or skip. False when an empty
    /// answer is fine.
    #[serde(default)]
    pub required: bool,
    /// The choices for a select question. Empty for text questions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<QuestionOption>,
    /// True to offer an `Other` choice with a text field.
    #[serde(default)]
    pub allow_other: bool,
}

/// The questions of one turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BriefQuestionSet {
    /// A short title for the card, such as `Two things before I draft`.
    pub title: String,
    /// One or two sentences that introduce the questions.
    pub message: String,
    /// The questions, at most `QUESTIONS_PER_TURN_LIMIT`.
    pub questions: Vec<BriefQuestion>,
    /// True when the agent can draft a brief with assumptions instead
    /// of waiting for these answers.
    #[serde(default)]
    pub can_proceed_with_assumptions: bool,
}

/// The user's answer to one question.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuestionAnswer {
    /// The id of the question answered.
    pub question_id: String,
    /// The chosen option values, or the text for a text question.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// The text of the `Other` choice, when the user chose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_text: Option<String>,
    /// True when the user chose `Use your best judgment`. The app
    /// records the question as an assumption.
    #[serde(default)]
    pub skipped: bool,
}

impl QuestionAnswer {
    /// True when the answer carries no value, no other text, and no
    /// skip.
    pub fn is_empty(&self) -> bool {
        !self.skipped
            && self.values.iter().all(|value| value.trim().is_empty())
            && self
                .other_text
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
    }
}

/// A problem in a question set. Messages address the agent that wrote
/// it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuestionSetError {
    /// The set has no questions.
    #[error("the question set has no questions: ask one to {limit}")]
    NoQuestions {
        /// The allowed maximum.
        limit: usize,
    },
    /// The set has more questions than the limit.
    #[error("the question set has {count} questions: ask at most {limit} per turn")]
    TooManyQuestions {
        /// How many were sent.
        count: usize,
        /// The allowed maximum.
        limit: usize,
    },
    /// Two questions share an id.
    #[error("question id `{id}` is used twice: give every question a unique id")]
    DuplicateId {
        /// The repeated id.
        id: String,
    },
    /// A question has a blank id.
    #[error("questions[{index}].id is empty: set a short unique id")]
    EmptyId {
        /// Zero-based question index.
        index: usize,
    },
    /// A question has a blank label.
    #[error("questions[{index}].label is empty: write the question text")]
    EmptyLabel {
        /// Zero-based question index.
        index: usize,
    },
    /// A select question has no options.
    #[error("question `{id}` is a select question with no options: add at least two options")]
    MissingOptions {
        /// The question id.
        id: String,
    },
    /// An option has a blank value or label.
    #[error("question `{id}` option {index} has an empty value or label: set both")]
    EmptyOption {
        /// The question id.
        id: String,
        /// Zero-based option index.
        index: usize,
    },
    /// A text question carries options.
    #[error(
        "question `{id}` is a text question with options: remove the options or make it a select"
    )]
    OptionsOnTextQuestion {
        /// The question id.
        id: String,
    },
    /// The set has a blank title.
    #[error("the question set title is empty: write a short title")]
    EmptyTitle,
}

/// Checks a question set and returns every problem found.
pub fn validate_question_set(set: &BriefQuestionSet) -> Vec<QuestionSetError> {
    let mut errors = Vec::new();
    if set.title.trim().is_empty() {
        errors.push(QuestionSetError::EmptyTitle);
    }
    if set.questions.is_empty() {
        errors.push(QuestionSetError::NoQuestions {
            limit: QUESTIONS_PER_TURN_LIMIT,
        });
    }
    if set.questions.len() > QUESTIONS_PER_TURN_LIMIT {
        errors.push(QuestionSetError::TooManyQuestions {
            count: set.questions.len(),
            limit: QUESTIONS_PER_TURN_LIMIT,
        });
    }
    let mut seen = HashSet::new();
    for (index, question) in set.questions.iter().enumerate() {
        let id = question.id.trim();
        if id.is_empty() {
            errors.push(QuestionSetError::EmptyId { index });
        } else if !seen.insert(id) {
            errors.push(QuestionSetError::DuplicateId { id: id.to_owned() });
        }
        if question.label.trim().is_empty() {
            errors.push(QuestionSetError::EmptyLabel { index });
        }
        validate_options(question, &mut errors);
    }
    errors
}

/// Checks that select questions have usable options and text questions
/// have none.
fn validate_options(question: &BriefQuestion, errors: &mut Vec<QuestionSetError>) {
    let id = question.id.trim().to_owned();
    if question.kind.is_select() {
        if question.options.is_empty() {
            errors.push(QuestionSetError::MissingOptions { id: id.clone() });
        }
        for (index, option) in question.options.iter().enumerate() {
            if option.value.trim().is_empty() || option.label.trim().is_empty() {
                errors.push(QuestionSetError::EmptyOption {
                    id: id.clone(),
                    index,
                });
            }
        }
    } else if !question.options.is_empty() {
        errors.push(QuestionSetError::OptionsOnTextQuestion { id });
    }
}

/// A problem in a set of answers. Messages address the client that sent
/// them.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnswerError {
    /// An answer names a question the set does not have.
    #[error("no question `{id}` in this set: answer only the questions asked")]
    UnknownQuestion {
        /// The unknown id.
        id: String,
    },
    /// Two answers name the same question.
    #[error("question `{id}` is answered twice: send one answer per question")]
    DuplicateAnswer {
        /// The repeated id.
        id: String,
    },
    /// A required question has neither an answer nor a skip.
    #[error("question `{id}` is required: answer it or skip it with best judgment")]
    RequiredUnanswered {
        /// The question id.
        id: String,
    },
    /// A single-select answer has more than one value.
    #[error("question `{id}` takes one value: send one")]
    TooManyValues {
        /// The question id.
        id: String,
    },
    /// A select answer names a value that is not an option.
    #[error("question `{id}` has no option `{value}`: choose an option, or use other_text")]
    UnknownOption {
        /// The question id.
        id: String,
        /// The rejected value.
        value: String,
    },
    /// `other_text` was sent for a question that does not allow it.
    #[error("question `{id}` does not take other_text: choose an option")]
    OtherNotAllowed {
        /// The question id.
        id: String,
    },
}

/// Checks `answers` against `set` and returns every problem found.
pub fn validate_answers(set: &BriefQuestionSet, answers: &[QuestionAnswer]) -> Vec<AnswerError> {
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    for answer in answers {
        let id = answer.question_id.as_str();
        let Some(question) = set.questions.iter().find(|question| question.id == id) else {
            errors.push(AnswerError::UnknownQuestion { id: id.to_owned() });
            continue;
        };
        if !seen.insert(id) {
            errors.push(AnswerError::DuplicateAnswer { id: id.to_owned() });
            continue;
        }
        if answer.skipped {
            continue;
        }
        validate_answer_values(question, answer, &mut errors);
    }
    for question in &set.questions {
        let answer = answers
            .iter()
            .find(|answer| answer.question_id == question.id);
        let is_unanswered = answer.is_none_or(QuestionAnswer::is_empty);
        if question.required && is_unanswered {
            errors.push(AnswerError::RequiredUnanswered {
                id: question.id.clone(),
            });
        }
    }
    errors
}

/// Checks the values of one non-skipped answer against its question.
fn validate_answer_values(
    question: &BriefQuestion,
    answer: &QuestionAnswer,
    errors: &mut Vec<AnswerError>,
) {
    let id = question.id.clone();
    let has_other = answer
        .other_text
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty());
    if has_other && !question.allow_other && question.kind.is_select() {
        errors.push(AnswerError::OtherNotAllowed { id: id.clone() });
    }
    if !question.kind.is_select() {
        return;
    }
    if question.kind == QuestionKind::SingleSelect && answer.values.len() > 1 {
        errors.push(AnswerError::TooManyValues { id: id.clone() });
    }
    for value in &answer.values {
        if !question.options.iter().any(|option| &option.value == value) {
            errors.push(AnswerError::UnknownOption {
                id: id.clone(),
                value: value.clone(),
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{
        AnswerError, BriefQuestion, BriefQuestionSet, QUESTIONS_PER_TURN_LIMIT, QuestionAnswer,
        QuestionKind, QuestionOption, QuestionSetError, validate_answers, validate_question_set,
    };

    fn option(value: &str) -> QuestionOption {
        QuestionOption {
            value: value.to_owned(),
            label: value.to_owned(),
        }
    }

    fn select(id: &str, required: bool) -> BriefQuestion {
        BriefQuestion {
            id: id.to_owned(),
            label: format!("Which {id}?"),
            rationale: Some("It changes the layout.".to_owned()),
            kind: QuestionKind::SingleSelect,
            required,
            options: vec![option("web"), option("app")],
            allow_other: true,
        }
    }

    fn text(id: &str) -> BriefQuestion {
        BriefQuestion {
            id: id.to_owned(),
            label: format!("Describe the {id}."),
            rationale: None,
            kind: QuestionKind::ShortText,
            required: false,
            options: Vec::new(),
            allow_other: false,
        }
    }

    fn set(questions: Vec<BriefQuestion>) -> BriefQuestionSet {
        BriefQuestionSet {
            title: "Before I draft".to_owned(),
            message: "Two things.".to_owned(),
            questions,
            can_proceed_with_assumptions: true,
        }
    }

    fn answer(id: &str, values: &[&str]) -> QuestionAnswer {
        QuestionAnswer {
            question_id: id.to_owned(),
            values: values.iter().map(|value| (*value).to_owned()).collect(),
            other_text: None,
            skipped: false,
        }
    }

    #[test]
    fn a_question_set_with_one_to_three_questions_is_valid() {
        assert_eq!(QUESTIONS_PER_TURN_LIMIT, 3);
        assert_eq!(
            validate_question_set(&set(vec![select("platform", true)])),
            Vec::new()
        );
        let three = set(vec![select("a", true), select("b", false), text("c")]);
        assert_eq!(validate_question_set(&three), Vec::new());
    }

    #[test]
    fn a_fourth_question_is_rejected() {
        let four = set(vec![
            select("a", true),
            select("b", false),
            text("c"),
            text("d"),
        ]);
        assert_eq!(
            validate_question_set(&four),
            vec![QuestionSetError::TooManyQuestions { count: 4, limit: 3 }]
        );
        assert_eq!(
            validate_question_set(&set(Vec::new())),
            vec![QuestionSetError::NoQuestions { limit: 3 }]
        );
    }

    #[test]
    fn every_question_set_problem_is_reported_not_only_the_first() {
        let mut broken = set(vec![select("a", true), select("a", true), text("b")]);
        broken.title = " ".to_owned();
        broken.questions[1].options.clear();
        broken.questions[2].options.push(option("x"));
        broken.questions[2].label = String::new();
        let errors = validate_question_set(&broken);
        assert!(errors.contains(&QuestionSetError::EmptyTitle));
        assert!(errors.contains(&QuestionSetError::DuplicateId { id: "a".to_owned() }));
        assert!(errors.contains(&QuestionSetError::MissingOptions { id: "a".to_owned() }));
        assert!(errors.contains(&QuestionSetError::EmptyLabel { index: 2 }));
        assert!(errors.contains(&QuestionSetError::OptionsOnTextQuestion { id: "b".to_owned() }));
        assert_eq!(errors.len(), 5);
        let mut blank = set(vec![select("", true)]);
        blank.questions[0].options[0].value = String::new();
        assert_eq!(
            validate_question_set(&blank),
            vec![
                QuestionSetError::EmptyId { index: 0 },
                QuestionSetError::EmptyOption {
                    id: String::new(),
                    index: 0
                },
            ]
        );
    }

    #[test]
    fn unknown_question_kinds_fail_to_deserialize() {
        let raw = r#"{"id":"a","label":"A?","kind":"slider","required":true}"#;
        assert!(serde_json::from_str::<BriefQuestion>(raw).is_err());
        let raw = r#"{"id":"a","label":"A?","kind":"short_text"}"#;
        let question: BriefQuestion = serde_json::from_str(raw).unwrap();
        assert_eq!(question.kind, QuestionKind::ShortText);
        assert!(!question.required);
    }

    #[test]
    fn question_set_json_matches_the_documented_shape() {
        let json = serde_json::to_value(set(vec![select("platform", true)])).unwrap();
        assert_eq!(json["can_proceed_with_assumptions"], true);
        let question = &json["questions"][0];
        assert_eq!(question["kind"], "single_select");
        assert_eq!(question["allow_other"], true);
        assert_eq!(question["required"], true);
        assert_eq!(question["options"][0]["value"], "web");
        assert_eq!(question["rationale"], "It changes the layout.");
        let restored: BriefQuestionSet = serde_json::from_value(json).unwrap();
        assert_eq!(restored, set(vec![select("platform", true)]));
    }

    #[test]
    fn answers_to_required_questions_must_have_values_unless_skipped() {
        let questions = set(vec![select("platform", true), text("goal")]);
        assert_eq!(
            validate_answers(&questions, &[]),
            vec![AnswerError::RequiredUnanswered {
                id: "platform".to_owned()
            }]
        );
        assert_eq!(
            validate_answers(&questions, &[answer("platform", &["web"])]),
            Vec::new()
        );
        let skipped = QuestionAnswer {
            question_id: "platform".to_owned(),
            skipped: true,
            ..QuestionAnswer::default()
        };
        assert_eq!(validate_answers(&questions, &[skipped]), Vec::new());
        let other = QuestionAnswer {
            question_id: "platform".to_owned(),
            other_text: Some("kiosk".to_owned()),
            ..QuestionAnswer::default()
        };
        assert_eq!(validate_answers(&questions, &[other]), Vec::new());
    }

    #[test]
    fn answer_values_are_checked_against_the_question() {
        let questions = set(vec![select("platform", false), text("goal")]);
        let errors = validate_answers(
            &questions,
            &[
                answer("platform", &["web", "tv"]),
                answer("platform", &["app"]),
                answer("tone", &["warm"]),
            ],
        );
        assert!(errors.contains(&AnswerError::TooManyValues {
            id: "platform".to_owned()
        }));
        assert!(errors.contains(&AnswerError::UnknownOption {
            id: "platform".to_owned(),
            value: "tv".to_owned()
        }));
        assert!(errors.contains(&AnswerError::DuplicateAnswer {
            id: "platform".to_owned()
        }));
        assert!(errors.contains(&AnswerError::UnknownQuestion {
            id: "tone".to_owned()
        }));
        assert_eq!(errors.len(), 4);
        let mut strict = questions.clone();
        strict.questions[0].allow_other = false;
        let other = QuestionAnswer {
            question_id: "platform".to_owned(),
            other_text: Some("kiosk".to_owned()),
            ..QuestionAnswer::default()
        };
        assert_eq!(
            validate_answers(&strict, &[other]),
            vec![AnswerError::OtherNotAllowed {
                id: "platform".to_owned()
            }]
        );
        assert_eq!(
            validate_answers(&questions, &[answer("goal", &["sign-ups"])]),
            Vec::new()
        );
    }
}
