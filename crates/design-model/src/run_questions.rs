//! The app's own questions that apply to both kinds: who the artifact
//! is for, what tone it takes, and how much of a demo to build.
//!
//! These recur in every session with the same answers, so the app asks
//! them from a fixed list instead of letting the agent invent options
//! that differ from run to run. The agent never asks them. The
//! request-specific questions stay with the agent.

/// Who the artifact is for, as (value, label). The value is the run
/// option; the label is the chip.
pub const AUDIENCES: [(&str, &str); 5] = [
    ("newcomers", "Newcomers to the subject"),
    ("practitioners", "Practitioners in the field"),
    ("decision_makers", "Decision makers"),
    ("customers", "Customers or users"),
    ("mixed", "Mixed audience"),
];

/// The tone the artifact takes, as (value, label).
///
/// Each value names a voice, not a purpose. An earlier list mixed the
/// two, so `Executive overview` and `Educational` overlapped and read
/// as a purpose the user had already stated in the request.
pub const TONES: [(&str, &str); 5] = [
    ("plain", "Plain and direct"),
    ("confident", "Confident and bold"),
    ("warm", "Warm and friendly"),
    ("technical", "Technical and precise"),
    ("playful", "Playful"),
];

/// How the colors read, as (value, label).
///
/// Each option is a complete look, not a single dial: it settles the
/// background and the color character together, so one pick is a full
/// answer. The values `light` and `dark` predate the longer labels and
/// stay stable for stored sessions. There is no `Either`: every card
/// already carries the judgment chip, which says the same thing.
pub const COLOR_MODES: [(&str, &str); 8] = [
    ("light", "Light and airy"),
    ("dark", "Dark and sleek"),
    ("colorful", "Bright and colorful"),
    ("pastel", "Soft pastels"),
    ("warm", "Warm and earthy"),
    ("corporate", "Cool and corporate"),
    ("high_contrast", "Stark, high contrast"),
    ("monochrome", "Black and white"),
];

/// What kind of product a demo shows, as (value, label). It decides the
/// layout vocabulary: a dashboard and a storefront share no furniture.
pub const PRODUCT_KINDS: [(&str, &str); 6] = [
    ("consumer_app", "Consumer mobile app"),
    ("business_app", "Business web app"),
    ("developer_tool", "Developer tool"),
    ("marketplace", "Marketplace or storefront"),
    ("content_site", "Content or media site"),
    ("dashboard", "Internal dashboard"),
];

/// What state a demo's screens are in, as (value, label). A demo that
/// shows empty screens reads as unfinished, so the app asks.
pub const DATA_STATES: [(&str, &str); 3] = [
    ("populated", "A full, realistic working state"),
    ("empty", "The first-run empty state"),
    ("mixed", "A mix across the screens"),
];

/// How much goes on one slide, as (value, label).
pub const SLIDE_DENSITIES: [(&str, &str); 3] = [
    ("sparse", "One idea in large type"),
    ("balanced", "A headline and a few points"),
    ("detailed", "Detailed, document-style"),
];

/// How much a deck leans on data, as (value, label). It decides how
/// many slides carry a chart.
pub const EVIDENCE_STYLES: [(&str, &str); 3] = [
    ("narrative", "Mostly narrative"),
    ("some_charts", "A few key charts"),
    ("data_heavy", "Data-heavy throughout"),
];

/// How much of a demo to build, as (value, label). A deck says its size
/// with the slide count instead.
pub const DEMO_SCOPES: [(&str, &str); 4] = [
    ("one_screen", "One polished screen"),
    ("short_flow", "A short flow of screens"),
    ("landing_page", "A landing page"),
    ("landing_and_app", "A landing page plus app screens"),
];

/// Longest answer a user may type into an app question.
pub const CUSTOM_ANSWER_LIMIT: usize = 120;

/// True when `value` is an answer the user typed instead of picking.
///
/// The fixed lists cover the common answers. They cannot cover every
/// answer, so every app question also takes text. The rule keeps a
/// typed answer short and printable: it goes straight into a prompt
/// line, where a control character or a run of text would break the
/// line the model reads.
pub fn is_custom_answer(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.chars().count() <= CUSTOM_ANSWER_LIMIT
        && !trimmed.chars().any(char::is_control)
}

/// The label for `value` in `choices`, or `None` when it is not one.
fn label_of(choices: &[(&'static str, &'static str)], value: &str) -> Option<&'static str> {
    choices
        .iter()
        .find(|(name, _)| *name == value.trim())
        .map(|(_, label)| *label)
}

/// The readable audience for a stored value, when it is a known one.
pub fn audience_label(value: &str) -> Option<&'static str> {
    label_of(&AUDIENCES, value)
}

/// The readable tone for a stored value, when it is a known one.
pub fn tone_label(value: &str) -> Option<&'static str> {
    label_of(&TONES, value)
}

/// The readable demo scope for a stored value, when it is a known one.
pub fn demo_scope_label(value: &str) -> Option<&'static str> {
    label_of(&DEMO_SCOPES, value)
}

/// Every app-owned axis both kinds ask, as (prompt name, choices).
pub const SHARED_AXES: [(&str, &[(&str, &str)]); 3] = [
    ("Audience", &AUDIENCES),
    ("Tone", &TONES),
    ("Color mode", &COLOR_MODES),
];

/// Every app-owned axis only a demo asks.
pub const DEMO_AXES: [(&str, &[(&str, &str)]); 3] = [
    ("Scope", &DEMO_SCOPES),
    ("Product kind", &PRODUCT_KINDS),
    ("Screen state", &DATA_STATES),
];

/// Every app-owned axis only a deck asks.
pub const DECK_AXES: [(&str, &[(&str, &str)]); 2] = [
    ("Slide density", &SLIDE_DENSITIES),
    ("Evidence", &EVIDENCE_STYLES),
];

/// The label for `value` on the axis named `name`, when both are known.
pub fn axis_label(name: &str, value: &str) -> Option<&'static str> {
    SHARED_AXES
        .iter()
        .chain(DEMO_AXES.iter())
        .chain(DECK_AXES.iter())
        .find(|(axis, _)| *axis == name)
        .and_then(|(_, choices)| label_of(choices, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_value_reads_back_as_its_label() {
        assert_eq!(
            audience_label("practitioners"),
            Some("Practitioners in the field")
        );
        assert_eq!(tone_label(" technical "), Some("Technical and precise"));
        assert_eq!(
            demo_scope_label("short_flow"),
            Some("A short flow of screens")
        );
    }

    #[test]
    fn a_typed_answer_is_short_and_printable() {
        assert!(is_custom_answer("Wry, like a good changelog"));
        assert!(is_custom_answer("  spaced  "));
        assert!(!is_custom_answer(""));
        assert!(!is_custom_answer("   "));
        assert!(!is_custom_answer(&"a".repeat(CUSTOM_ANSWER_LIMIT + 1)));
        assert!(is_custom_answer(&"a".repeat(CUSTOM_ANSWER_LIMIT)));
        // A newline would break the prompt line the value lands in.
        assert!(!is_custom_answer("two\nlines"));
    }

    #[test]
    fn an_unknown_value_has_no_label() {
        assert_eq!(audience_label("astronauts"), None);
        assert_eq!(tone_label(""), None);
        assert_eq!(demo_scope_label("whole_app"), None);
    }

    #[test]
    fn each_kind_asks_its_own_axes_and_the_shared_ones() {
        let demo: Vec<&str> = SHARED_AXES
            .iter()
            .chain(DEMO_AXES.iter())
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            demo,
            [
                "Audience",
                "Tone",
                "Color mode",
                "Scope",
                "Product kind",
                "Screen state"
            ]
        );
        let deck: Vec<&str> = SHARED_AXES
            .iter()
            .chain(DECK_AXES.iter())
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(
            deck,
            [
                "Audience",
                "Tone",
                "Color mode",
                "Slide density",
                "Evidence"
            ]
        );
    }

    #[test]
    fn an_axis_reads_back_by_name_and_value() {
        assert_eq!(
            axis_label("Product kind", "developer_tool"),
            Some("Developer tool")
        );
        assert_eq!(
            axis_label("Evidence", "data_heavy"),
            Some("Data-heavy throughout")
        );
        // An unknown axis or value has no label, so it reaches no prompt.
        assert_eq!(axis_label("Vibe", "loud"), None);
        assert_eq!(axis_label("Product kind", "spaceship"), None);
    }

    #[test]
    fn no_axis_name_is_used_twice() {
        let names: Vec<&str> = SHARED_AXES
            .iter()
            .chain(DEMO_AXES.iter())
            .chain(DECK_AXES.iter())
            .map(|(name, _)| *name)
            .collect();
        for name in &names {
            assert_eq!(
                names.iter().filter(|other| *other == name).count(),
                1,
                "{name}"
            );
        }
    }

    #[test]
    fn every_value_is_snake_case_and_unique() {
        let banks = [
            &AUDIENCES[..],
            &TONES[..],
            &COLOR_MODES[..],
            &DEMO_SCOPES[..],
            &PRODUCT_KINDS[..],
            &DATA_STATES[..],
            &SLIDE_DENSITIES[..],
            &EVIDENCE_STYLES[..],
        ];
        for choices in banks {
            let values: Vec<&str> = choices.iter().map(|(value, _)| *value).collect();
            for value in &values {
                assert!(!value.is_empty());
                assert!(
                    value.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                    "{value}"
                );
                assert_eq!(values.iter().filter(|other| *other == value).count(), 1);
            }
        }
    }
}
