//! The design brief: the canonical, versioned input to generation.
//!
//! A brief keeps three things apart: facts the user confirmed,
//! assumptions the app or the agent made, and questions still open.
//! The approved revision is the only content input a generation run
//! reads. Every edit makes a new revision; the history stays with the
//! brief.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact_kind::ArtifactKind;
use crate::question::QuestionAnswer;
use crate::viewport::Viewport;

/// Who or what made a brief revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevisionSource {
    /// The briefing agent drafted or updated it.
    Agent,
    /// The user edited it in the brief panel.
    UserEdit,
    /// A critique added generation instructions.
    Critique,
    /// The app moved open questions into assumptions.
    Assumptions,
}

impl RevisionSource {
    /// The snake_case name used in JSON and labels.
    pub fn as_str(self) -> &'static str {
        match self {
            RevisionSource::Agent => "agent",
            RevisionSource::UserEdit => "user_edit",
            RevisionSource::Critique => "critique",
            RevisionSource::Assumptions => "assumptions",
        }
    }
}

/// One entry in a brief's revision history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BriefRevision {
    /// The revision number, from 1.
    pub revision: u32,
    /// What made the revision.
    pub source: RevisionSource,
    /// One line that says what changed.
    pub summary: String,
    /// When it was made, as an RFC 3339 UTC string.
    pub at: String,
}

/// One required screen or section of the artifact.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BriefSection {
    /// The section name, such as `Hero` or `Pricing`.
    pub name: String,
    /// What the section must contain.
    #[serde(default)]
    pub content: String,
}

/// What a critique is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CritiqueCategory {
    /// Colors, type, imagery, mood.
    VisualDirection,
    /// Layout, hierarchy, information architecture.
    Structure,
    /// Contrast, text size, keyboard and screen-reader needs.
    Accessibility,
    /// Copy, data, and what the artifact says.
    Content,
    /// Anything else.
    FreeForm,
}

impl CritiqueCategory {
    /// Every category, in the order the UI shows them.
    pub const ALL: [CritiqueCategory; 5] = [
        CritiqueCategory::VisualDirection,
        CritiqueCategory::Structure,
        CritiqueCategory::Accessibility,
        CritiqueCategory::Content,
        CritiqueCategory::FreeForm,
    ];

    /// The snake_case name used in JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            CritiqueCategory::VisualDirection => "visual_direction",
            CritiqueCategory::Structure => "structure",
            CritiqueCategory::Accessibility => "accessibility",
            CritiqueCategory::Content => "content",
            CritiqueCategory::FreeForm => "free_form",
        }
    }

    /// The text the user sees.
    pub fn label(self) -> &'static str {
        match self {
            CritiqueCategory::VisualDirection => "Visual direction",
            CritiqueCategory::Structure => "Structure",
            CritiqueCategory::Accessibility => "Accessibility",
            CritiqueCategory::Content => "Content",
            CritiqueCategory::FreeForm => "Free-form",
        }
    }
}

/// One critique of a generated design.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Critique {
    /// What the critique is about.
    pub category: CritiqueCategory,
    /// What to change.
    pub text: String,
}

impl Critique {
    /// The generation instruction this critique adds to the brief, like
    /// `[Structure] Move pricing above the FAQ.`
    pub fn as_instruction(&self) -> String {
        format!("[{}] {}", self.category.label(), self.text.trim())
    }
}

/// One question the user answered, kept as text.
///
/// The brief must read on its own, so it carries the question wording
/// and the answer wording, not only the answer ids in `answers`. The
/// studio shows the two apart, so a reader can tell a question from an
/// answer at a glance.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnsweredQuestion {
    /// The question, as the user saw it.
    pub question: String,
    /// The answer, as the user gave it. Empty when the user skipped.
    pub answer: String,
    /// True when the user skipped and asked the agent to decide.
    #[serde(default)]
    pub is_assumed: bool,
}

/// The design brief. Every field has a default, so a partial draft from
/// the agent loads and the user fills the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DesignBrief {
    /// The user's request, in their words.
    pub request: String,
    /// The answers the user gave, newest last.
    pub answers: Vec<QuestionAnswer>,
    /// The answered questions as text, newest last. One entry per
    /// question the user answered or skipped.
    pub answered_questions: Vec<AnsweredQuestion>,
    /// Facts the user stated or confirmed. Never guessed.
    pub confirmed_facts: Vec<String>,
    /// Choices the app or the agent made without the user. Use only
    /// where a confirmed fact does not cover the need.
    pub assumptions: Vec<String>,
    /// Questions still without an answer.
    pub open_questions: Vec<String>,
    /// `demo` or `deck`. The session sets it. A demo run writes a
    /// design; a deck run writes a deck.
    pub artifact_kind: ArtifactKind,
    /// How many slides the user asked for. The app asks; the agent does
    /// not. `None` lets the agent choose. Only for a deck.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide_count: Option<u32>,
    /// What to make, such as `landing page` or `onboarding flow`.
    pub target_artifact: String,
    /// Where it runs, such as `desktop web` or `iOS app`. The first
    /// entry of `target_platforms` when the app set several.
    pub target_platform: String,
    /// Every platform the run builds for. The app asks; the agent does
    /// not. One design is written per platform, each on that
    /// platform's canvas. Empty means the one in `target_platform`.
    pub target_platforms: Vec<String>,
    /// Who it is for.
    pub audience: String,
    /// The problem the audience has.
    pub user_problem: String,
    /// The one thing a user must be able to do.
    pub primary_job: String,
    /// How the team knows the design worked, such as `sign-up rate`.
    pub success_criterion: String,
    /// The screens or sections in order, one line each.
    pub information_architecture: Vec<String>,
    /// The screens or sections that must exist, with their content.
    pub required_sections: Vec<BriefSection>,
    /// Mood, references, and style words.
    pub visual_direction: String,
    /// Logos, colors, fonts, and files the design must use.
    pub brand_assets: Vec<String>,
    /// Accessibility needs, such as `WCAG AA contrast`.
    pub accessibility_constraints: Vec<String>,
    /// Technical limits, such as `no external fonts`.
    pub technical_constraints: Vec<String>,
    /// Instructions for the generation run, newest last. Critiques
    /// append here.
    pub generation_instructions: Vec<String>,
    /// The revision number of this brief, from 1.
    pub revision: u32,
    /// Every revision so far, oldest first.
    pub revision_history: Vec<BriefRevision>,
}

impl DesignBrief {
    /// The viewport the brief's target platform implies.
    pub fn viewport(&self) -> Viewport {
        Viewport::for_platform(&self.target_platform)
    }

    /// Every platform the run builds for, never empty.
    ///
    /// Falls back to `target_platform` for a brief written before the
    /// app asked for more than one. An empty name is still a platform:
    /// it stands for the default desktop canvas.
    pub fn platforms(&self) -> Vec<String> {
        let listed: Vec<String> = self
            .target_platforms
            .iter()
            .filter(|platform| !platform.trim().is_empty())
            .cloned()
            .collect();
        if listed.is_empty() {
            return vec![self.target_platform.clone()];
        }
        listed
    }

    /// The canvas of every platform the run builds for, in order.
    pub fn viewports(&self) -> Vec<Viewport> {
        self.platforms()
            .iter()
            .map(|platform| Viewport::for_platform(platform))
            .collect()
    }

    /// Moves every open question into `assumptions` as `Assumed: …`
    /// and appends `extra`, without duplicates. Used when the user
    /// generates with assumptions.
    pub fn with_assumed_open_questions(mut self, extra: Vec<String>) -> DesignBrief {
        let open = std::mem::take(&mut self.open_questions);
        for question in open {
            push_unique(&mut self.assumptions, format!("Assumed: {question}"));
        }
        for assumption in extra {
            push_unique(&mut self.assumptions, assumption);
        }
        self
    }

    /// Adds `instruction` to the generation instructions, without
    /// duplicates.
    pub fn with_instruction(mut self, instruction: String) -> DesignBrief {
        push_unique(&mut self.generation_instructions, instruction);
        self
    }

    /// True when the brief names what to make and who it is for: the
    /// two facts generation cannot guess.
    pub fn has_core_facts(&self) -> bool {
        !self.target_artifact.trim().is_empty() && !self.audience.trim().is_empty()
    }
}

/// Pushes `item` unless an equal item is already in `items`.
fn push_unique(items: &mut Vec<String>, item: String) {
    let item = item.trim().to_owned();
    if item.is_empty() || items.iter().any(|existing| existing.trim() == item) {
        return;
    }
    items.push(item);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{
        AnsweredQuestion, BriefRevision, BriefSection, Critique, CritiqueCategory, DesignBrief,
        RevisionSource,
    };
    use crate::Viewport;

    fn brief() -> DesignBrief {
        DesignBrief {
            request: "Design a landing page for my finance app.".to_owned(),
            confirmed_facts: vec!["Platform: desktop web".to_owned()],
            assumptions: vec!["Audience: retail investors".to_owned()],
            open_questions: vec!["Which brand colors?".to_owned()],
            target_artifact: "landing page".to_owned(),
            target_platform: "desktop web".to_owned(),
            audience: "retail investors".to_owned(),
            required_sections: vec![BriefSection {
                name: "Hero".to_owned(),
                content: "Value proposition and sign-up button".to_owned(),
            }],
            revision: 1,
            revision_history: vec![BriefRevision {
                revision: 1,
                source: RevisionSource::Agent,
                summary: "Drafted from the conversation".to_owned(),
                at: "2026-08-26T10:00:00Z".to_owned(),
            }],
            ..DesignBrief::default()
        }
    }

    #[test]
    fn brief_round_trips_through_json() {
        let brief = brief();
        let json = serde_json::to_string(&brief).unwrap();
        assert!(json.contains("\"confirmed_facts\""));
        assert!(json.contains("\"source\":\"agent\""));
        let restored: DesignBrief = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, brief);
    }

    #[test]
    fn a_brief_without_a_kind_is_a_demo() {
        let brief: DesignBrief = serde_json::from_str(r#"{"request":"x"}"#).unwrap();
        assert_eq!(brief.artifact_kind, crate::ArtifactKind::Demo);
        let deck: DesignBrief =
            serde_json::from_str(r#"{"request":"x","artifact_kind":"deck"}"#).unwrap();
        assert_eq!(deck.artifact_kind, crate::ArtifactKind::Deck);
        assert!(
            serde_json::to_string(&deck)
                .unwrap()
                .contains("\"artifact_kind\":\"deck\"")
        );
    }

    #[test]
    fn answered_questions_keep_the_question_and_the_answer_apart() {
        let brief = DesignBrief {
            answered_questions: vec![
                AnsweredQuestion {
                    question: "Which platform?".to_owned(),
                    answer: "Desktop web".to_owned(),
                    is_assumed: false,
                },
                AnsweredQuestion {
                    question: "Which tone?".to_owned(),
                    answer: String::new(),
                    is_assumed: true,
                },
            ],
            ..DesignBrief::default()
        };
        let json = serde_json::to_string(&brief).unwrap();
        let restored: DesignBrief = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.answered_questions, brief.answered_questions);
        assert_eq!(restored.answered_questions[0].question, "Which platform?");
        assert_eq!(restored.answered_questions[0].answer, "Desktop web");
        assert!(restored.answered_questions[1].is_assumed);
    }

    #[test]
    fn the_platforms_fall_back_to_the_single_one() {
        let mut brief = DesignBrief {
            target_platform: "desktop web".to_owned(),
            ..DesignBrief::default()
        };
        assert_eq!(brief.platforms(), vec!["desktop web"]);
        assert_eq!(brief.viewports(), vec![Viewport::default()]);

        brief.target_platforms = vec![
            "desktop web".to_owned(),
            "  ".to_owned(),
            "phone".to_owned(),
        ];
        assert_eq!(brief.platforms(), vec!["desktop web", "phone"]);
        let viewports = brief.viewports();
        assert_eq!(viewports.len(), 2);
        assert_eq!(viewports[0], Viewport::default());
        assert_eq!(viewports[1].width, 390);
        // The first platform still decides the single-viewport reading.
        assert_eq!(brief.viewport(), Viewport::default());
    }

    #[test]
    fn a_slide_count_is_absent_until_the_app_sets_one() {
        let brief: DesignBrief = serde_json::from_str(r#"{"request":"x"}"#).unwrap();
        assert_eq!(brief.slide_count, None);
        assert!(
            !serde_json::to_string(&brief)
                .unwrap()
                .contains("slide_count")
        );
        let counted: DesignBrief =
            serde_json::from_str(r#"{"request":"x","slide_count":12}"#).unwrap();
        assert_eq!(counted.slide_count, Some(12));
        assert!(
            serde_json::to_string(&counted)
                .unwrap()
                .contains("\"slide_count\":12")
        );
    }

    #[test]
    fn a_brief_with_missing_fields_loads_with_defaults() {
        let brief: DesignBrief = serde_json::from_str(r#"{"request":"x"}"#).unwrap();
        assert_eq!(brief.request, "x");
        assert_eq!(brief.revision, 0);
        assert!(brief.assumptions.is_empty());
        assert!(!brief.has_core_facts());
        assert!(DesignBrief::default().audience.is_empty());
    }

    #[test]
    fn assuming_open_questions_moves_them_to_assumptions() {
        let brief = brief().with_assumed_open_questions(vec![
            "Assumed for `Tone`: best judgment".to_owned(),
            "Audience: retail investors".to_owned(),
        ]);
        assert!(brief.open_questions.is_empty());
        assert_eq!(
            brief.assumptions,
            vec![
                "Audience: retail investors",
                "Assumed: Which brand colors?",
                "Assumed for `Tone`: best judgment",
            ]
        );
        assert_eq!(brief.confirmed_facts, vec!["Platform: desktop web"]);
    }

    #[test]
    fn critiques_become_labelled_instructions() {
        let critique = Critique {
            category: CritiqueCategory::Structure,
            text: " Move pricing above the FAQ. ".to_owned(),
        };
        assert_eq!(
            critique.as_instruction(),
            "[Structure] Move pricing above the FAQ."
        );
        let brief = brief()
            .with_instruction(critique.as_instruction())
            .with_instruction(critique.as_instruction());
        assert_eq!(brief.generation_instructions.len(), 1);
        assert_eq!(
            serde_json::to_string(&CritiqueCategory::VisualDirection).unwrap(),
            "\"visual_direction\""
        );
        assert_eq!(CritiqueCategory::ALL.len(), 5);
        assert_eq!(CritiqueCategory::FreeForm.label(), "Free-form");
    }

    #[test]
    fn the_viewport_follows_the_target_platform() {
        let mut brief = brief();
        assert_eq!(brief.viewport().width, 1440);
        brief.target_platform = "iOS app".to_owned();
        assert_eq!(brief.viewport().width, 390);
    }
}
