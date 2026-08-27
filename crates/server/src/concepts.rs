//! Candidate concepts: one distinct design direction per candidate.
//!
//! Before the engine writes N > 1 candidates, it asks the model for N
//! concepts in one call. Each candidate is then written from its own
//! concept and told what the other concepts are, so candidates differ
//! by design rather than by chance.

use design_model::DesignBrief;
use serde::{Deserialize, Serialize};

/// One design direction for a candidate design.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Concept {
    /// Short name, for logs and the prompt.
    #[serde(default)]
    pub name: String,
    /// The angle on the subject this candidate takes.
    #[serde(default)]
    pub angle: String,
    /// Screen titles in order.
    #[serde(default)]
    pub outline: Vec<String>,
    /// The palette as `#rrggbb` values: background, text, accent, muted.
    #[serde(default)]
    pub palette: serde_json::Value,
    /// The font families: heading, body, mono.
    #[serde(default)]
    pub fonts: serde_json::Value,
    /// How the screens look: element arrangement, imagery, rhythm.
    #[serde(default)]
    pub visual_idea: String,
}

/// The shape of the concept reply.
#[derive(Debug, Deserialize)]
struct ConceptReply {
    #[serde(default)]
    concepts: Vec<Concept>,
}

/// What the concepts share at each variety level.
fn shared_note(variety: &str) -> &'static str {
    match variety {
        "low" => {
            "All concepts share the same angle, the same outline, and the same \
             visual idea. They differ only in palette and fonts."
        }
        "high" => {
            "Every concept has its own angle, its own outline and screen count, \
             its own palette and fonts, and its own visual idea. Make them as \
             different from each other as the brief allows."
        }
        _ => {
            "All concepts share the same outline. Each has its own palette, \
             fonts, and visual idea."
        }
    }
}

/// The system prompt for the concept call.
pub fn concept_prompt(count: usize) -> String {
    format!(
        "You plan {count} distinct candidate designs for one brief.\n\
         {shared}\n\
         Reply with only this JSON:\n\
         {{\"concepts\":[{{\"name\":\"...\",\"angle\":\"...\",\"outline\":[\"screen title\"],\
         \"palette\":{{\"background\":\"#rrggbb\",\"text\":\"#rrggbb\",\"accent\":\"#rrggbb\",\"muted\":\"#rrggbb\"}},\
         \"fonts\":{{\"heading\":\"family\",\"body\":\"family\",\"mono\":\"family\"}},\
         \"visual_idea\":\"how the screens look\"}}]}}\n\
         Give exactly {count} concepts. Use web-safe or Google font family names.\n\
         Keep every string short. Reply with only the JSON.",
        shared = shared_note("medium"),
    )
}

/// The user input for the concept call: the approved brief.
pub fn concept_input(brief: &DesignBrief) -> String {
    let mut input = format!("Request:\n{}\n", brief.request);
    input.push_str(&format!("Kind: {}\n", brief.artifact_kind.label()));
    if !brief.target_artifact.is_empty() {
        input.push_str(&format!("Artifact: {}\n", brief.target_artifact));
    }
    if !brief.audience.is_empty() {
        input.push_str(&format!("Audience: {}\n", brief.audience));
    }
    if !brief.visual_direction.is_empty() {
        input.push_str(&format!("Visual direction: {}\n", brief.visual_direction));
    }
    if !brief.information_architecture.is_empty() {
        input.push_str(&format!(
            "Information architecture: {}\n",
            brief.information_architecture.join("; ")
        ));
    }
    input.push_str("Reply with only the JSON.");
    input
}

/// Parses the concept reply. Bad JSON yields an empty list.
pub fn parse_concepts(content: &str) -> Vec<Concept> {
    let Some(start) = content.find('{') else {
        return Vec::new();
    };
    let Some(end) = content.rfind('}') else {
        return Vec::new();
    };
    if end < start {
        return Vec::new();
    }
    serde_json::from_str::<ConceptReply>(&content[start..=end])
        .map(|reply| reply.concepts)
        .unwrap_or_default()
}

/// The prompt lines for candidate `index` (zero-based): its own concept
/// in full, and the other concepts as a do-not-copy list. Empty when no
/// concept exists for the index.
pub fn concept_note(concepts: &[Concept], index: usize) -> String {
    let Some(own) = concepts.get(index) else {
        return String::new();
    };
    let mut note = format!(
        "Your concept, follow it exactly: {}\n\
         Use its palette and fonts for the theme, its outline for the screens in order, \
         and its visual idea for the element arrangement.\n",
        serde_json::to_string(own).unwrap_or_default()
    );
    let others: Vec<String> = concepts
        .iter()
        .enumerate()
        .filter(|(other_index, _)| *other_index != index)
        .map(|(_, concept)| {
            format!(
                "- {}: {} / {}",
                concept.name, concept.angle, concept.visual_idea
            )
        })
        .collect();
    if !others.is_empty() {
        note.push_str("Other candidates use these concepts. Do not copy them:\n");
        note.push_str(&others.join("\n"));
        note.push('\n');
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_state_the_count_and_the_variety_contract() {
        let prompt = concept_prompt(3);
        assert!(prompt.contains("plan 3 distinct"));
        let brief = DesignBrief {
            request: "A finance landing page.".to_owned(),
            audience: "retail investors".to_owned(),
            ..DesignBrief::default()
        };
        assert!(concept_input(&brief).contains("Audience: retail investors"));
        assert!(concept_input(&brief).contains("A finance landing page."));
    }

    #[test]
    fn concepts_parse_and_tolerate_prose_and_missing_fields() {
        let concepts = parse_concepts(
            "Here you go: {\"concepts\":[{\"name\":\"Bold\",\"angle\":\"why now\",\
             \"outline\":[\"Intro\"],\"visual_idea\":\"big numbers\"},{\"name\":\"Calm\"}]}",
        );
        assert_eq!(concepts.len(), 2);
        assert_eq!(concepts[0].outline, vec!["Intro"]);
        assert_eq!(concepts[1].angle, "");
        assert!(parse_concepts("no json here").is_empty());
        assert!(parse_concepts("{\"concepts\": 5}").is_empty());
    }

    #[test]
    fn notes_carry_the_own_concept_and_list_the_others() {
        let concepts = vec![
            Concept {
                name: "Bold".to_owned(),
                angle: "why now".to_owned(),
                visual_idea: "big numbers".to_owned(),
                ..Concept::default()
            },
            Concept {
                name: "Calm".to_owned(),
                angle: "the long view".to_owned(),
                visual_idea: "soft photos".to_owned(),
                ..Concept::default()
            },
        ];
        let note = concept_note(&concepts, 1);
        assert!(note.contains("\"name\":\"Calm\""));
        assert!(note.contains("- Bold: why now / big numbers"));
        assert!(!note.contains("- Calm:"));
        assert!(concept_note(&concepts, 5).is_empty());
    }
}
