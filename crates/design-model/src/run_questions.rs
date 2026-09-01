//! The app's own questions: how the colors read for every kind, how
//! much of a demo to build and what it shows, who a deck, a document,
//! a social, a print, or a mailing is for and what tone it takes, what
//! a document is and what paper it takes, where a social is posted and
//! on what canvas, what a print is and on what paper size and
//! orientation, what an email is and on what canvas, and what an ad
//! sells and on what IAB unit.
//!
//! These recur in every session with the same answers, so the app asks
//! them from a fixed list instead of letting the agent invent options
//! that differ from run to run. The agent never asks them. The
//! request-specific questions stay with the agent.

use crate::ArtifactKind;

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
///
/// No label names a device. The canvas question owns the device, and
/// `Consumer mobile app` beside a phone canvas read as the same
/// question asked twice. The values predate the labels and stay.
pub const PRODUCT_KINDS: [(&str, &str); 6] = [
    ("consumer_app", "Consumer app"),
    ("business_app", "Business tool"),
    ("developer_tool", "Developer tool"),
    ("marketplace", "Marketplace or storefront"),
    ("content_site", "Content or media"),
    ("dashboard", "Internal dashboard"),
];

/// What state a demo's screens are in, as (value, label). A demo that
/// shows empty screens reads as unfinished, so the app asks.
pub const DATA_STATES: [(&str, &str); 3] = [
    ("populated", "Filled with realistic data"),
    ("empty", "The first-run empty state"),
    ("mixed", "A mix across the screens"),
];

/// How finished a demo looks, as (value, label). A wireframe is a
/// layout check: gray blocks for imagery, one neutral palette, real
/// copy. The default is the finished look.
pub const FIDELITIES: [(&str, &str); 2] = [
    ("high_fidelity", "Finished, high fidelity"),
    ("wireframe", "Wireframe, gray boxes"),
];

/// How much goes on one slide, as (value, label).
pub const SLIDE_DENSITIES: [(&str, &str); 3] = [
    ("sparse", "One idea in large type"),
    ("balanced", "A headline and a few points"),
    ("detailed", "Detailed, document-style"),
];

/// How much a deck or a document leans on data, as (value, label). It
/// decides how many slides or pages carry a chart or a table.
pub const EVIDENCE_STYLES: [(&str, &str); 3] = [
    ("narrative", "Mostly narrative"),
    ("some_charts", "A few key charts"),
    ("data_heavy", "Data-heavy throughout"),
];

/// What kind of document to write, as (value, label). It decides the
/// page vocabulary: a memo and a guide share no furniture.
pub const DOCUMENT_KINDS: [(&str, &str); 6] = [
    ("report", "Report"),
    ("memo", "Memo or brief"),
    ("proposal", "Proposal"),
    ("one_pager", "One-pager"),
    ("letter", "Letter"),
    ("guide", "Guide or manual"),
];

/// The paper a document is laid out on, as (value, label). The values
/// are the `paper` names in the document JSON.
pub const PAPERS: [(&str, &str); 2] = [("a4", "A4"), ("letter", "US Letter")];

/// The platform a social is posted on, as (value, label). It decides
/// the voice and the furniture: a LinkedIn carousel and an Instagram
/// story share no conventions.
pub const PLATFORMS: [(&str, &str); 4] = [
    ("instagram", "Instagram"),
    ("linkedin", "LinkedIn"),
    ("x", "X"),
    ("facebook", "Facebook"),
];

/// The canvas a social is laid out on, as (value, label). The values
/// are the `format` names in the social JSON.
pub const FORMATS: [(&str, &str); 4] = [
    ("square", "Square, 1:1"),
    ("portrait", "Portrait, 4:5"),
    ("story", "Story, 9:16"),
    ("landscape", "Landscape, 1.91:1"),
];

/// What a social is for, as (value, label). It decides the shape of
/// the copy: an announcement leads with the news, a lesson with the
/// claim.
pub const POST_GOALS: [(&str, &str); 4] = [
    ("announce", "Announce something"),
    ("educate", "Teach or explain"),
    ("promote", "Promote an offer"),
    ("recruit", "Recruit or invite"),
];

/// What kind of print piece to lay out, as (value, label). It decides
/// the sheet vocabulary: a poster and a menu share no furniture.
pub const PRINT_KINDS: [(&str, &str); 6] = [
    ("poster", "Poster"),
    ("flyer", "Flyer"),
    ("menu", "Menu"),
    ("program", "Program"),
    ("certificate", "Certificate"),
    ("sign", "Sign"),
];

/// The paper size a print is laid out on, as (value, label). The
/// values are the `size` names in the print JSON.
pub const PRINT_SIZES: [(&str, &str); 5] = [
    ("a5", "A5"),
    ("a4", "A4"),
    ("a3", "A3"),
    ("letter", "US Letter"),
    ("tabloid", "Tabloid"),
];

/// How a print's sheets are turned, as (value, label). The values are
/// the `orientation` names in the print JSON.
pub const ORIENTATIONS: [(&str, &str); 2] = [("portrait", "Portrait"), ("landscape", "Landscape")];

/// What kind of email to write, as (value, label). It decides the
/// email vocabulary: a newsletter and a welcome email share no
/// furniture.
pub const EMAIL_KINDS: [(&str, &str); 6] = [
    ("newsletter", "Newsletter"),
    ("announcement", "Announcement"),
    ("promotion", "Promotion or offer"),
    ("welcome", "Welcome email"),
    ("digest", "Digest"),
    ("invitation", "Invitation"),
];

/// The canvas an email is laid out on, as (value, label). The values
/// are the `format` names in the mailing JSON. Every format is 600 px
/// wide; the formats differ in height.
pub const EMAIL_FORMATS: [(&str, &str); 3] = [
    ("short", "Short, one glance"),
    ("standard", "Standard, one scroll"),
    ("long", "Long, a full read"),
];

/// What kind of ad to write, as (value, label). It decides the copy
/// vocabulary: a launch ad and a retargeting ad share no furniture.
pub const AD_KINDS: [(&str, &str); 6] = [
    ("product_launch", "Product launch"),
    ("sale", "Sale or offer"),
    ("brand_awareness", "Brand awareness"),
    ("event", "Event"),
    ("retargeting", "Retargeting"),
    ("app_install", "App install"),
];

/// The canvas an ad is laid out on, as (value, label). The values are
/// the `size` names in the campaign JSON, the standard IAB units.
pub const AD_SIZES: [(&str, &str); 5] = [
    ("medium_rectangle", "Medium rectangle, 300 by 250"),
    ("leaderboard", "Leaderboard, 728 by 90"),
    ("half_page", "Half page, 300 by 600"),
    ("skyscraper", "Skyscraper, 160 by 600"),
    ("mobile_banner", "Mobile banner, 320 by 100"),
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

/// One app-owned question: the option it writes, the name the prompt
/// prints, and the fixed choices as (value, label).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppAxis {
    /// The run option the pick lands in, such as `product_kind`.
    pub key: &'static str,
    /// The name the prompt prints, such as `Product kind`.
    pub name: &'static str,
    /// The fixed choices, as (value, label).
    pub choices: &'static [(&'static str, &'static str)],
}

/// Every app-owned axis every kind asks.
pub const SHARED_AXES: [AppAxis; 1] = [AppAxis {
    key: "color_mode",
    name: "Color mode",
    choices: &COLOR_MODES,
}];

/// Every app-owned axis only a demo asks.
pub const DEMO_AXES: [AppAxis; 4] = [
    AppAxis {
        key: "scope",
        name: "Scope",
        choices: &DEMO_SCOPES,
    },
    AppAxis {
        key: "product_kind",
        name: "Product kind",
        choices: &PRODUCT_KINDS,
    },
    AppAxis {
        key: "data_state",
        name: "Screen data",
        choices: &DATA_STATES,
    },
    AppAxis {
        key: "fidelity",
        name: "Fidelity",
        choices: &FIDELITIES,
    },
];

/// Every app-owned axis a deck and a document ask, and a demo does not.
///
/// The audience, the tone, and the evidence live here: a deck speaks to
/// a room and a document to a reader, and the copy changes with who
/// reads it. A demo's screens show a product, and the request already
/// says what it is.
pub const SPEECH_AXES: [AppAxis; 3] = [
    AppAxis {
        key: "audience",
        name: "Audience",
        choices: &AUDIENCES,
    },
    AppAxis {
        key: "tone",
        name: "Tone",
        choices: &TONES,
    },
    AppAxis {
        key: "evidence_style",
        name: "Evidence",
        choices: &EVIDENCE_STYLES,
    },
];

/// Every app-owned axis only a deck asks.
pub const DECK_AXES: [AppAxis; 1] = [AppAxis {
    key: "slide_density",
    name: "Slide density",
    choices: &SLIDE_DENSITIES,
}];

/// Every app-owned axis only a document asks.
pub const DOCUMENT_AXES: [AppAxis; 3] = [
    AppAxis {
        key: "document_kind",
        name: "Document kind",
        choices: &DOCUMENT_KINDS,
    },
    AppAxis {
        key: "paper",
        name: "Paper",
        choices: &PAPERS,
    },
    AppAxis {
        key: "page_density",
        name: "Page density",
        choices: &SLIDE_DENSITIES,
    },
];

/// Every app-owned axis only a social asks.
pub const SOCIAL_AXES: [AppAxis; 3] = [
    AppAxis {
        key: "platform",
        name: "Platform",
        choices: &PLATFORMS,
    },
    AppAxis {
        key: "format",
        name: "Format",
        choices: &FORMATS,
    },
    AppAxis {
        key: "post_goal",
        name: "Post goal",
        choices: &POST_GOALS,
    },
];

/// Every app-owned axis only a print asks.
pub const PRINT_AXES: [AppAxis; 3] = [
    AppAxis {
        key: "print_kind",
        name: "Print kind",
        choices: &PRINT_KINDS,
    },
    AppAxis {
        key: "print_size",
        name: "Print size",
        choices: &PRINT_SIZES,
    },
    AppAxis {
        key: "orientation",
        name: "Orientation",
        choices: &ORIENTATIONS,
    },
];

/// Every app-owned axis only a mailing asks.
pub const MAILING_AXES: [AppAxis; 2] = [
    AppAxis {
        key: "email_kind",
        name: "Email kind",
        choices: &EMAIL_KINDS,
    },
    AppAxis {
        key: "email_format",
        name: "Email format",
        choices: &EMAIL_FORMATS,
    },
];

/// Every app-owned axis only a campaign asks.
pub const CAMPAIGN_AXES: [AppAxis; 2] = [
    AppAxis {
        key: "ad_kind",
        name: "Ad kind",
        choices: &AD_KINDS,
    },
    AppAxis {
        key: "ad_size",
        name: "Ad size",
        choices: &AD_SIZES,
    },
];

/// Every app-owned axis of every kind.
fn every_axis() -> impl Iterator<Item = &'static AppAxis> {
    SHARED_AXES
        .iter()
        .chain(DEMO_AXES.iter())
        .chain(SPEECH_AXES.iter())
        .chain(DECK_AXES.iter())
        .chain(DOCUMENT_AXES.iter())
        .chain(SOCIAL_AXES.iter())
        .chain(PRINT_AXES.iter())
        .chain(MAILING_AXES.iter())
        .chain(CAMPAIGN_AXES.iter())
}

/// The app-owned axes `kind` asks, shared ones first.
pub fn app_axes(kind: ArtifactKind) -> impl Iterator<Item = &'static AppAxis> {
    let (spoken, own): (&'static [AppAxis], &'static [AppAxis]) = match kind {
        ArtifactKind::Demo => (&[], &DEMO_AXES),
        ArtifactKind::Deck => (&SPEECH_AXES, &DECK_AXES),
        ArtifactKind::Document => (&SPEECH_AXES, &DOCUMENT_AXES),
        ArtifactKind::Social => (&SPEECH_AXES, &SOCIAL_AXES),
        ArtifactKind::Print => (&SPEECH_AXES, &PRINT_AXES),
        ArtifactKind::Mailing => (&SPEECH_AXES, &MAILING_AXES),
        ArtifactKind::Campaign => (&SPEECH_AXES, &CAMPAIGN_AXES),
    };
    SHARED_AXES.iter().chain(spoken.iter()).chain(own.iter())
}

/// The axis whose option is `key`, when there is one.
pub fn axis_by_key(key: &str) -> Option<&'static AppAxis> {
    every_axis().find(|axis| axis.key == key)
}

/// The label for `value` on the axis named `name`, when both are known.
pub fn axis_label(name: &str, value: &str) -> Option<&'static str> {
    every_axis()
        .find(|axis| axis.name == name)
        .and_then(|axis| label_of(axis.choices, value))
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
        let demo: Vec<&str> = app_axes(ArtifactKind::Demo).map(|axis| axis.name).collect();
        assert_eq!(
            demo,
            [
                "Color mode",
                "Scope",
                "Product kind",
                "Screen data",
                "Fidelity"
            ]
        );
        let deck: Vec<&str> = app_axes(ArtifactKind::Deck).map(|axis| axis.name).collect();
        assert_eq!(
            deck,
            [
                "Color mode",
                "Audience",
                "Tone",
                "Evidence",
                "Slide density"
            ]
        );
        let document: Vec<&str> = app_axes(ArtifactKind::Document)
            .map(|axis| axis.name)
            .collect();
        assert_eq!(
            document,
            [
                "Color mode",
                "Audience",
                "Tone",
                "Evidence",
                "Document kind",
                "Paper",
                "Page density"
            ]
        );
        let social: Vec<&str> = app_axes(ArtifactKind::Social)
            .map(|axis| axis.name)
            .collect();
        assert_eq!(
            social,
            [
                "Color mode",
                "Audience",
                "Tone",
                "Evidence",
                "Platform",
                "Format",
                "Post goal"
            ]
        );
        let print: Vec<&str> = app_axes(ArtifactKind::Print)
            .map(|axis| axis.name)
            .collect();
        assert_eq!(
            print,
            [
                "Color mode",
                "Audience",
                "Tone",
                "Evidence",
                "Print kind",
                "Print size",
                "Orientation"
            ]
        );
        let mailing: Vec<&str> = app_axes(ArtifactKind::Mailing)
            .map(|axis| axis.name)
            .collect();
        assert_eq!(
            mailing,
            [
                "Color mode",
                "Audience",
                "Tone",
                "Evidence",
                "Email kind",
                "Email format"
            ]
        );
        let campaign: Vec<&str> = app_axes(ArtifactKind::Campaign)
            .map(|axis| axis.name)
            .collect();
        assert_eq!(
            campaign,
            [
                "Color mode",
                "Audience",
                "Tone",
                "Evidence",
                "Ad kind",
                "Ad size"
            ]
        );
    }

    #[test]
    fn the_paper_values_are_the_paper_names() {
        for (value, _) in PAPERS {
            assert!(crate::Paper::from_name(value).is_some(), "{value}");
        }
    }

    #[test]
    fn the_format_values_are_the_format_names() {
        for (value, _) in FORMATS {
            assert!(crate::Format::from_name(value).is_some(), "{value}");
        }
        assert_eq!(FORMATS.len(), crate::Format::ALL.len());
    }

    #[test]
    fn the_print_size_values_are_the_print_size_names() {
        for (value, _) in PRINT_SIZES {
            assert!(crate::PrintSize::from_name(value).is_some(), "{value}");
        }
        assert_eq!(PRINT_SIZES.len(), crate::PrintSize::ALL.len());
    }

    #[test]
    fn the_orientation_values_are_the_orientation_names() {
        for (value, _) in ORIENTATIONS {
            assert!(crate::Orientation::from_name(value).is_some(), "{value}");
        }
        assert_eq!(ORIENTATIONS.len(), crate::Orientation::ALL.len());
    }

    #[test]
    fn the_email_format_values_are_the_email_format_names() {
        for (value, _) in EMAIL_FORMATS {
            assert!(crate::EmailFormat::from_name(value).is_some(), "{value}");
        }
        assert_eq!(EMAIL_FORMATS.len(), crate::EmailFormat::ALL.len());
    }

    #[test]
    fn the_ad_size_values_are_the_ad_size_names() {
        for (value, _) in AD_SIZES {
            assert!(crate::AdSize::from_name(value).is_some(), "{value}");
        }
        assert_eq!(AD_SIZES.len(), crate::AdSize::ALL.len());
    }

    #[test]
    fn no_product_kind_names_a_device() {
        for (_, label) in PRODUCT_KINDS {
            let lower = label.to_lowercase();
            for device in ["mobile", "web", "phone", "desktop", "site"] {
                assert!(!lower.contains(device), "{label} names a device");
            }
        }
    }

    #[test]
    fn an_axis_is_found_by_its_option_key() {
        assert_eq!(
            axis_by_key("product_kind").map(|axis| axis.name),
            Some("Product kind")
        );
        assert_eq!(axis_by_key("vibe"), None);
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
        let names: Vec<&str> = every_axis().map(|axis| axis.name).collect();
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
            &FIDELITIES[..],
            &SLIDE_DENSITIES[..],
            &EVIDENCE_STYLES[..],
            &DOCUMENT_KINDS[..],
            &PAPERS[..],
            &PLATFORMS[..],
            &FORMATS[..],
            &POST_GOALS[..],
            &PRINT_KINDS[..],
            &PRINT_SIZES[..],
            &ORIENTATIONS[..],
            &EMAIL_KINDS[..],
            &EMAIL_FORMATS[..],
            &AD_KINDS[..],
            &AD_SIZES[..],
        ];
        for choices in banks {
            let values: Vec<&str> = choices.iter().map(|(value, _)| *value).collect();
            for value in &values {
                assert!(!value.is_empty());
                assert!(
                    value
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                    "{value}"
                );
                assert_eq!(values.iter().filter(|other| *other == value).count(), 1);
            }
        }
    }
}
