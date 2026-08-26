//! The app's own questions before it writes candidates: what scenario
//! the design is for, how long the design is, how many candidates, and how
//! different.
//!
//! None is a composer setting. The app asks them in the conversation,
//! like any other clarification, before it writes candidates. The
//! answers are stored in the brief as `scenario`, `length`,
//! `variations`, and `variety`.

use crate::candidates::CANDIDATE_LIMIT;
use crate::questions::Question;

/// The question text for the design's scenario.
pub const SCENARIO_QUESTION: &str = "What scenario is the design for?";

/// The scenario options. The user may pick several.
pub const SCENARIO_OPTIONS: [&str; 20] = [
    "Technology",
    "Academia",
    "Business",
    "Finance",
    "Medical",
    "Science",
    "Engineering",
    "Education",
    "Marketing",
    "Sales",
    "Design",
    "Legal",
    "Government",
    "Nonprofit",
    "Startup pitch",
    "Product launch",
    "Training",
    "Conference talk",
    "Internal update",
    "Personal",
];

/// The scenario stored when the user says "You decide".
pub const DEFAULT_SCENARIO: &str = "general";

/// The question text for the design length.
pub const LENGTH_QUESTION: &str = "How long should the design be?";

/// The length options. Each names a screen range.
pub const LENGTH_OPTIONS: [&str; 3] = [
    "Short: 5 to 8 screens",
    "Standard: 10 to 15 screens",
    "Long: 20 to 30 screens",
];

/// The length stored when the user says "You decide": no target.
pub const DEFAULT_LENGTH: &str = "any";

/// The question text for the candidate count.
pub const VARIATION_QUESTION: &str = "How many candidates should I write?";

/// The count used when the user says "You decide".
pub const DEFAULT_VARIATIONS: usize = 3;

/// The question text for the variety level.
pub const VARIETY_QUESTION: &str = "How different should the candidates be?";

/// The variety options, one per level. The level is the first word.
pub const VARIETY_OPTIONS: [&str; 3] = [
    "Low: same structure, new colors and fonts",
    "Medium: new themes and arrangements, same outline",
    "High: new themes, structure, and angle",
];

/// The level used when the user says "You decide".
pub const DEFAULT_VARIETY: &str = "medium";

/// The scenario question with its preset options.
pub fn scenario_question() -> Question {
    Question {
        question: SCENARIO_QUESTION.to_owned(),
        options: SCENARIO_OPTIONS
            .iter()
            .map(|option| (*option).to_owned())
            .collect(),
    }
}

/// Is `question` the scenario question?
pub fn is_scenario_question(question: &str) -> bool {
    question.trim() == SCENARIO_QUESTION
}

/// The scenario named by an answer, as typed. "You decide" or an empty
/// answer maps to the default.
pub fn scenario_from_answer(answer: &str) -> String {
    let trimmed = answer.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("you decide") {
        DEFAULT_SCENARIO.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The prompt line for a chosen scenario. Empty for the default.
pub fn scenario_note(scenario: &str) -> String {
    if scenario == DEFAULT_SCENARIO {
        return String::new();
    }
    format!(
        "The design is for this scenario: {scenario}. \
         Use the vocabulary, the examples, and the tone that fit it.\n"
    )
}

/// The length question with its preset screen ranges.
pub fn length_question() -> Question {
    Question {
        question: LENGTH_QUESTION.to_owned(),
        options: LENGTH_OPTIONS
            .iter()
            .map(|option| (*option).to_owned())
            .collect(),
    }
}

/// Is `question` the length question?
pub fn is_length_question(question: &str) -> bool {
    question.trim() == LENGTH_QUESTION
}

/// The screen range named by an answer, as `min-max`: the first and the
/// last number in the answer. One number stands for itself. An answer
/// with no number, like "You decide", maps to the default.
pub fn length_from_answer(answer: &str) -> String {
    let numbers: Vec<usize> = answer
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|digits| digits.parse::<usize>().ok())
        .filter(|number| *number >= 1)
        .collect();
    match (numbers.first(), numbers.last()) {
        (Some(first), Some(last)) => format!("{}-{}", first.min(last), first.max(last)),
        _ => DEFAULT_LENGTH.to_owned(),
    }
}

/// The screen range behind a stored length: `(min, max)`. `None` for the
/// default or a malformed value.
pub fn length_bounds(length: &str) -> Option<(usize, usize)> {
    let (min, max) = length.trim().split_once('-')?;
    let min = min.trim().parse::<usize>().ok()?;
    let max = max.trim().parse::<usize>().ok()?;
    (min >= 1 && min <= max).then_some((min, max))
}

/// The prompt line for a chosen length. Empty for the default.
pub fn length_note(length: &str) -> String {
    let Some((min, max)) = length_bounds(length) else {
        return String::new();
    };
    if min == max {
        return format!(
            "Write {min} screens, counting the title screen. One screen more or fewer is \
             acceptable when the content needs it.\n"
        );
    }
    format!(
        "Write between {min} and {max} screens, counting the title screen. Stay in this \
         range. One screen outside it is acceptable when the content needs it.\n"
    )
}

/// The candidate-count question: one option per count up to the limit.
pub fn variation_question() -> Question {
    Question {
        question: VARIATION_QUESTION.to_owned(),
        options: (1..=CANDIDATE_LIMIT)
            .map(|count| {
                if count == 1 {
                    "1 candidate".to_owned()
                } else {
                    format!("{count} candidates")
                }
            })
            .collect(),
    }
}

/// Is `question` the candidate-count question?
pub fn is_variation_question(question: &str) -> bool {
    question.trim() == VARIATION_QUESTION
}

/// The count named by an answer, clamped to the limit. An answer with
/// no leading number, like "You decide", maps to the default.
pub fn count_from_answer(answer: &str) -> usize {
    let digits: String = answer
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse::<usize>() {
        Ok(count) if count >= 1 => count.min(CANDIDATE_LIMIT),
        _ => DEFAULT_VARIATIONS,
    }
}

/// The variety question with its preset options.
pub fn variety_question() -> Question {
    Question {
        question: VARIETY_QUESTION.to_owned(),
        options: VARIETY_OPTIONS
            .iter()
            .map(|option| (*option).to_owned())
            .collect(),
    }
}

/// Is `question` the variety question?
pub fn is_variety_question(question: &str) -> bool {
    question.trim() == VARIETY_QUESTION
}

/// The level named by an answer: `low`, `medium`, or `high`. An answer
/// that names no level, like "You decide", maps to the default.
pub fn level_from_answer(answer: &str) -> String {
    let lowered = answer.trim().to_ascii_lowercase();
    for level in ["low", "medium", "high"] {
        if lowered.starts_with(level) {
            return level.to_owned();
        }
    }
    DEFAULT_VARIETY.to_owned()
}

/// What must differ between candidates at `level`.
pub fn variety_note(level: &str) -> &'static str {
    match level {
        "low" => {
            "Keep the same outline, screen count, and element arrangement across \
             candidates. Change only the color palette and the fonts."
        }
        "high" => {
            "Give each candidate a different theme, a different outline and screen \
             count, a different arrangement of elements on every screen, and a different \
             angle on the subject."
        }
        _ => {
            "Keep the same outline across candidates. Give each candidate a \
             different theme and a different arrangement of elements on every screen."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_answers_are_kept_as_typed_or_defaulted() {
        assert_eq!(
            scenario_from_answer("Finance, Business"),
            "Finance, Business"
        );
        assert_eq!(scenario_from_answer("You decide"), DEFAULT_SCENARIO);
        assert_eq!(scenario_from_answer("  "), DEFAULT_SCENARIO);
        assert!(scenario_note(DEFAULT_SCENARIO).is_empty());
        assert!(scenario_note("Medical").contains("Medical"));
    }

    #[test]
    fn the_scenario_question_offers_the_preset_list() {
        let question = scenario_question();
        assert!(is_scenario_question(&question.question));
        assert_eq!(question.options.len(), SCENARIO_OPTIONS.len());
        assert!(question.options.iter().any(|option| option == "Technology"));
    }

    #[test]
    fn length_answers_map_to_a_screen_range() {
        assert_eq!(length_from_answer(LENGTH_OPTIONS[0]), "5-8");
        assert_eq!(length_from_answer("Standard: 10 to 15 screens"), "10-15");
        assert_eq!(
            length_from_answer("Short: 5 to 8 screens, Long: 20 to 30 screens"),
            "5-30"
        );
        assert_eq!(length_from_answer("12"), "12-12");
        assert_eq!(length_from_answer("You decide"), DEFAULT_LENGTH);
        assert_eq!(length_from_answer("0"), DEFAULT_LENGTH);
        assert_eq!(length_bounds("10-15"), Some((10, 15)));
        assert_eq!(length_bounds(DEFAULT_LENGTH), None);
        assert_eq!(length_bounds("15-10"), None);
        assert_eq!(length_bounds("0-3"), None);
    }

    #[test]
    fn length_notes_state_the_range_or_stay_empty() {
        assert!(length_note("10-15").contains("between 10 and 15 screens"));
        assert!(length_note("12-12").contains("Write 12 screens"));
        assert!(length_note(DEFAULT_LENGTH).is_empty());
        assert!(length_note("bad").is_empty());
    }

    #[test]
    fn the_length_question_offers_the_preset_ranges() {
        let question = length_question();
        assert!(is_length_question(&question.question));
        assert_eq!(question.options.len(), LENGTH_OPTIONS.len());
        assert!(!is_length_question(VARIATION_QUESTION));
    }

    #[test]
    fn count_answers_parse_their_leading_number() {
        assert_eq!(count_from_answer("1 candidate"), 1);
        assert_eq!(count_from_answer(" 4 candidates"), 4);
        assert_eq!(count_from_answer("9"), CANDIDATE_LIMIT);
        assert_eq!(count_from_answer("0"), DEFAULT_VARIATIONS);
        assert_eq!(count_from_answer("You decide"), DEFAULT_VARIATIONS);
    }

    #[test]
    fn the_count_question_offers_every_count_up_to_the_limit() {
        let question = variation_question();
        assert!(is_variation_question(&question.question));
        assert_eq!(question.options.len(), CANDIDATE_LIMIT);
        assert_eq!(question.options[0], "1 candidate");
        assert_eq!(question.options[1], "2 candidates");
    }

    #[test]
    fn variety_answers_map_to_levels_by_their_first_word() {
        assert_eq!(level_from_answer(VARIETY_OPTIONS[0]), "low");
        assert_eq!(level_from_answer("  high "), "high");
        assert_eq!(level_from_answer("You decide"), "medium");
    }

    #[test]
    fn the_variety_question_carries_one_option_per_level() {
        let question = variety_question();
        assert!(is_variety_question(&question.question));
        assert_eq!(question.options.len(), 3);
        assert!(!is_variety_question("How long?"));
    }

    #[test]
    fn variety_notes_differ_by_level() {
        assert!(variety_note("low").contains("only the color palette"));
        assert!(variety_note("high").contains("different outline"));
        assert!(variety_note("medium").contains("same outline"));
    }
}
