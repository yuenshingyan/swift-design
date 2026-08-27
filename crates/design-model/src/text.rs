//! Text comparison for the brief, shared by the server and the studio.
//!
//! A brief states the same thing several ways: an answer, a confirmed
//! fact, and a field can all say `desktop web app`. Both the server,
//! which drops repeated lines before it stores a brief, and the studio,
//! which hides a field an answer already states, need one rule for
//! "this text says nothing new". The rule lives here so the two cannot
//! drift apart.

/// True when `container` already says `value`.
///
/// The match is lexical: letters and digits are compared in lowercase,
/// every other character counts as a break, and `value` must appear as
/// whole words. A one-word `value` must have at least five characters,
/// so a short answer such as `All` cannot strike out every line that
/// holds the letters `all`, `small` included.
pub fn repeats(container: &str, value: &str) -> bool {
    let value_normalized = normalized(value);
    let trimmed = value_normalized.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.split_whitespace().count() == 1 && trimmed.chars().count() < 5 {
        return false;
    }
    normalized(container).contains(&value_normalized)
}

/// Share of words two texts must have in common, both ways, to count
/// as saying the same thing. Seven in ten leaves room for a rewording.
const SAME_MEANING_SHARE: usize = 7;

/// True when two texts say the same thing in different words.
///
/// [`repeats`] needs the words in the same order, which misses the way
/// a model rewrites an answer: `Small app flow: list, task detail` and
/// `TODO app small flow: list, task detail` share every word but not
/// the order. This compares the two word sets instead, in both
/// directions, so a long line that merely mentions a short answer is
/// not mistaken for that answer: `Developers can quickly manage tasks`
/// keeps its place next to the answer `Developers`.
pub fn mostly_repeats(left: &str, right: &str) -> bool {
    let left_words = words(left);
    let right_words = words(right);
    if left_words.is_empty() || right_words.is_empty() {
        return false;
    }
    // One short word is too weak a signal either way.
    if right_words.len() == 1 && right_words[0].chars().count() < 5 {
        return false;
    }
    shared_share(&left_words, &right_words) >= SAME_MEANING_SHARE
        && shared_share(&right_words, &left_words) >= SAME_MEANING_SHARE
}

/// How many of `these` words appear in `those`, out of ten.
fn shared_share(these: &[String], those: &[String]) -> usize {
    let shared = these.iter().filter(|word| those.contains(word)).count();
    shared * 10 / these.len()
}

/// The distinct words of `text`, normalized.
fn words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in normalized(text).split_whitespace() {
        if !words.iter().any(|kept| kept == word) {
            words.push(word.to_owned());
        }
    }
    words
}

/// The text as lowercase words between single spaces, with a space at
/// each end, so `contains` always matches whole words.
pub fn normalized(text: &str) -> String {
    let mut normalized = String::from(" ");
    for character in text.chars() {
        if character.is_alphanumeric() {
            normalized.extend(character.to_lowercase());
        } else if !normalized.ends_with(' ') {
            normalized.push(' ');
        }
    }
    if !normalized.ends_with(' ') {
        normalized.push(' ');
    }
    normalized
}

#[cfg(test)]
mod tests {
    use crate::text::{mostly_repeats, normalized, repeats};

    #[test]
    fn punctuation_and_case_do_not_change_a_match() {
        assert!(repeats(
            "The target platform is desktop web app.",
            "Desktop web app"
        ));
        assert!(repeats("The audience is developers.", "developers"));
        assert!(repeats("Ship by March 2026.", "march 2026"));
    }

    #[test]
    fn a_line_that_adds_something_is_not_a_repeat() {
        assert!(!repeats("The deadline is March.", "Desktop web app"));
        assert!(!repeats("The audience is developers.", ""));
        assert!(!repeats("", "developers"));
    }

    #[test]
    fn a_short_one_word_value_never_matches() {
        // `All` inside `small` must not count as a repeat.
        assert!(!repeats("Use a small app flow.", "All"));
        assert!(!repeats("The primary action covers all tasks.", "all"));
    }

    #[test]
    fn a_value_matches_only_as_whole_words() {
        assert!(!repeats("Developersaurus rex.", "developers"));
        assert!(repeats("For developers, mostly.", "developers"));
    }

    #[test]
    fn mostly_repeats_needs_the_value_to_cover_the_line() {
        // Same words, different order: the model reworded the answer.
        assert!(mostly_repeats(
            "TODO app small flow: list, task detail, add/edit task",
            "Small app flow: list, task detail, add/edit task"
        ));
        assert!(mostly_repeats("developers", "Developers"));
        // The field only mentions the answer; it says much more.
        assert!(!mostly_repeats(
            "Developers can quickly manage coding tasks across list, detail, and add/edit states",
            "Developers"
        ));
        assert!(!mostly_repeats("The deadline is March.", "developers"));
        // A long field that merely restates a short answer keeps its place.
        assert!(!mostly_repeats(
            "Organize project or coding work by viewing tasks, opening details, and editing tasks",
            "Organize project or coding work"
        ));
        assert!(!mostly_repeats("", "developers"));
        assert!(!mostly_repeats("developers", ""));
        // The rule reads the same from either side.
        assert!(mostly_repeats("Desktop web app", "desktop web app!"));
        assert!(mostly_repeats("desktop web app!", "Desktop web app"));
    }

    #[test]
    fn normalized_text_is_padded_lowercase_words() {
        assert_eq!(normalized("Desktop  web-app!"), " desktop web app ");
        assert_eq!(normalized(""), " ");
    }
}
