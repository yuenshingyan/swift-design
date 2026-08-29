//! The app's own deck questions, as in Swift Deck: the scenario and
//! how different the candidates are. The app asks them next to the
//! agent's questions; the agent never does.

/// The scenario presets. The user picks one, or leaves it to the agent.
///
/// Ten presets, each a kind of deck rather than a field of work. Swift
/// Deck listed twenty fields (`Technology`, `Finance`, `Medical`, and
/// so on), and a field says less about the deck than its occasion
/// does: a finance pitch and a finance training deck share nothing but
/// the word. A stored value outside the list is the user's own words
/// and stays valid.
pub const DECK_SCENARIOS: [&str; 10] = [
    "Pitch or launch",
    "Business review",
    "Conference talk",
    "Teaching",
    "Research",
    "Technical deep dive",
    "Design review",
    "Policy or legal",
    "Internal update",
    "Personal",
];

/// The variety levels as (value, label). The value is the run option.
pub const DECK_VARIETY_LEVELS: [(&str, &str); 3] = [
    ("low", "Low: same structure, new colors and fonts"),
    (
        "medium",
        "Medium: new themes and arrangements, same outline",
    ),
    ("high", "High: new themes, structure, and angle"),
];

/// True when `name` is one of the scenario presets.
pub fn is_deck_scenario(name: &str) -> bool {
    DECK_SCENARIOS.contains(&name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_are_known_scenarios() {
        assert!(is_deck_scenario("Pitch or launch"));
        assert!(is_deck_scenario(" Teaching "));
        assert!(!is_deck_scenario("Cooking"));
        assert!(!is_deck_scenario(""));
    }

    #[test]
    fn the_variety_levels_use_the_run_option_values() {
        let values: Vec<&str> = DECK_VARIETY_LEVELS
            .iter()
            .map(|(value, _)| *value)
            .collect();
        assert_eq!(values, ["low", "medium", "high"]);
    }
}
