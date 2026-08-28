//! The app's own deck questions, as in Swift Deck: the scenario and
//! how different the candidates are. The app asks them next to the
//! agent's questions; the agent never does.

/// The scenario presets. The user picks one, or leaves it to the agent.
pub const DECK_SCENARIOS: [&str; 20] = [
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
        assert!(is_deck_scenario("Startup pitch"));
        assert!(is_deck_scenario(" Design "));
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
