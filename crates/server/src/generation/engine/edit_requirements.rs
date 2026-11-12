//! Resolve demo edit scope before dispatch and check requirements before saving.

use design_model::Design;
use serde::Deserialize;
use serde_json::json;

use super::super::{Attachments, EditOrder, GenerationEngine};

#[derive(Deserialize)]
struct DemoEditReview {
    coverage: Vec<RequirementReview>,
    reference_error: Option<String>,
}

#[derive(Deserialize)]
struct RequirementReview {
    complete: bool,
    reason: String,
}

#[derive(Deserialize)]
pub(super) struct DemoEditRequirements {
    pub instruction: String,
    pub screens: Vec<usize>,
    pub minimum_new_screens: usize,
    pub requirements: Vec<String>,
    #[serde(skip)]
    source: String,
}

pub(super) struct DemoEditEvidence<'a> {
    pub candidate_id: &'a str,
    pub before: &'a Design,
    pub after: &'a Design,
    pub attachments: &'a Attachments,
}

impl DemoEditRequirements {
    fn validate(&self, design: &Design) -> Result<(), String> {
        if self.instruction.trim().is_empty()
            || self.requirements.is_empty()
            || self.requirements.iter().any(|item| item.trim().is_empty())
            || self
                .screens
                .iter()
                .any(|index| *index >= design.screens.len())
        {
            return Err(
                "Cannot plan demo edit: incomplete requirements or invalid screen scope.".into(),
            );
        }
        Ok(())
    }

    pub(super) fn check_structure(&self, before: &Design, after: &Design) -> Result<(), String> {
        if after.screens.len().saturating_sub(before.screens.len()) < self.minimum_new_screens {
            return Err(format!(
                "Cannot save demo edit: at least {} new screens were required; the edit did not add them.",
                self.minimum_new_screens
            ));
        }
        if !self.screens.is_empty()
            && before.screens.iter().enumerate().any(|(index, screen)| {
                !self.screens.contains(&index) && after.screens.get(index) != Some(screen)
            })
        {
            return Err("Cannot save demo edit: an unselected screen changed.".into());
        }
        Ok(())
    }

    fn review_failures(&self, review: &DemoEditReview) -> Result<Vec<String>, String> {
        if let Some(reason) = &review.reference_error {
            if reason.trim().is_empty() {
                return Err(
                    "Cannot verify demo edit: the reference error has no explanation.".into(),
                );
            }
            return Err(format!("Cannot verify demo edit reference: {reason}"));
        }
        if review.coverage.len() != self.requirements.len() {
            return Err("Cannot verify demo edit: the review omitted requirements.".into());
        }
        let mut missing = Vec::new();
        for (item, result) in self.requirements.iter().zip(&review.coverage) {
            if !result.complete {
                if result.reason.trim().is_empty() {
                    return Err(
                        "Cannot verify demo edit: an incomplete requirement has no explanation."
                            .into(),
                    );
                }
                missing.push(format!("{item}: {}", result.reason));
            }
        }
        Ok(missing)
    }
}

impl GenerationEngine {
    pub(super) async fn plan_demo_edit(
        &self,
        client: &reqwest::Client,
        design: &Design,
        order: &EditOrder<'_>,
        review_input: (&Attachments, &str),
    ) -> Result<DemoEditRequirements, String> {
        let (attachments, effort) = review_input;
        let inventory: Vec<_> = design
            .screens
            .iter()
            .enumerate()
            .map(|(index, screen)| json!({"index": index, "name": screen.name}))
            .collect();
        let messages = vec![
            json!({"role": "system", "content":
            "Resolve the current demo edit requirements. Return JSON only with instruction (a self-contained actionable brief), screens (zero-based existing screen indexes explicitly restricted by the user, or [] for candidate-wide work), minimum_new_screens (minimum additions the user requires, 0 if none), and requirements (a nonempty checklist of independently verifiable deliverables). Read the latest instruction together with the conversation: a clarification such as 'only candidate 4' retains the preceding feature request. Candidate numbers are NOT screen numbers. Ignore prior completed tasks and assistant-invented restrictions. Extending a demo can require BOTH modifying existing screens and adding connected screens; preserve both in the checklist. Include every requested feature, responsive variant, and relevant interaction/state requirement; do not substitute layout repairs. Distinguish proposed concepts from implemented features. A reference to an existing screen does not forbid requested additions. When additions are required, append them to preserve existing indexes. Do not invent restrictions from worker assignments. Candidate write isolation is enforced by the server, not a visual deliverable; do not put it in the checklist. For restyling, preservation means retaining the existing content and behavior, not requiring identical HTML/CSS or implementing missing backend features. This is planning, not evidence of completion."}),
            self.user_message(&format!(
                "Conversation:\n{}\nLatest instruction:\n{}\nRegenerate selected screens: {}\nExisting screens:\n{}",
                order.conversation, order.instruction, order.is_fresh, json!(inventory)
            ), attachments),
        ];
        let reply = self.model.chat(client, &messages, effort).await?;
        let mut plan: DemoEditRequirements = serde_json::from_str(&reply).map_err(|_| {
            "Cannot plan demo edit: expected a JSON requirements object.".to_owned()
        })?;
        if order.is_fresh {
            plan.screens = crate::edit_focus::referenced_indexes(order.instruction, "screen");
            plan.minimum_new_screens = 0;
            plan.instruction = order.instruction.to_owned();
        }
        plan.validate(design)?;
        plan.source = format!(
            "{}\nLatest instruction: {}",
            order.conversation, order.instruction
        );
        Ok(plan)
    }

    pub(super) async fn verify_demo_edit(
        &self,
        client: &reqwest::Client,
        plan: &DemoEditRequirements,
        evidence: DemoEditEvidence<'_>,
        effort: &str,
    ) -> Result<Vec<String>, String> {
        let messages = vec![
            json!({"role": "system", "content":
            "Review demo requirement coverage before saving. Return only a JSON object with coverage (one object per checklist item in order, each with complete: boolean and reason: string) and reference_error (null unless a required reference cannot be inspected, then a specific explanation and recovery action). Explain each incomplete item with concrete evidence and an actionable correction. Use the attached references when judging theme matching; do not invent reference colors or fonts. If a required reference is missing or unreadable, report reference_error rather than treating it as an incomplete edit. Judge actual HTML/CSS, navigation destinations and interaction states, not notes or promises. For a styling-only request, theme/CSS-only changes are valid. Require new features or screens only when requested by the current task; a layout-only patch is insufficient only when feature work was requested. Mark missing, partial, disconnected, or unverifiable requirements false. Compare the checklist with the original conversation and latest request; if the plan omitted requested work, mark all items false rather than accepting a narrowed brief. Evaluate only the current task and its clarifications, not earlier completed tasks or assistant-invented restrictions; the latest user requirements resolve conflicts. Proposed demo interactions need not have a backend, but must be present in the artifact. For preservation requirements, compare the before and after artifacts; do not demand identical markup for a styling change, new backend functionality, or responsive behavior absent from the baseline. Candidate isolation is a server-enforced fact: the pending write can only save the identified candidate, not other candidates. Do not reject a candidate-scope checklist item because other candidates are not supplied. This review is not a browser or backend test."}),
            self.user_message(&format!(
                "Pending save target (server-enforced): {}\nOriginal request:\n{}\nEdit brief:\n{}\nChecklist:\n{}\nBefore artifact:\n{}\nAfter artifact:\n{}",
                evidence.candidate_id, plan.source, plan.instruction, json!(plan.requirements),
                json!(evidence.before), json!(evidence.after)
            ), evidence.attachments),
        ];
        let reply = self.model.chat(client, &messages, effort).await?;
        let review: DemoEditReview = serde_json::from_str(&reply)
            .map_err(|_| "Cannot verify demo edit: expected a JSON coverage review.".to_owned())?;
        plan.review_failures(&review)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_or_out_of_scope_edits_fail_closed() {
        let before: Design =
            serde_json::from_str(crate::test_support::SAMPLE_DESIGN).expect("valid fixture");
        let mut plan = DemoEditRequirements {
            source: String::new(),
            instruction: "Extend candidate 4, not screen 4".into(),
            screens: vec![0],
            minimum_new_screens: 1,
            requirements: vec!["Comments".into(), "Notification page".into()],
        };
        assert!(plan.validate(&before).is_ok());
        assert!(plan.check_structure(&before, &before).is_err());
        let mut after = before.clone();
        after.screens.push(before.screens[0].clone());
        assert!(plan.check_structure(&before, &after).is_ok());
        after.screens[1].notes = Some("Unrelated change".into());
        assert!(plan.check_structure(&before, &after).is_err());
        let mut review: DemoEditReview = serde_json::from_value(json!({
            "coverage": [
                {"complete": true, "reason": "Comments present"},
                {"complete": true, "reason": "Notification page present"}
            ],
            "reference_error": null
        }))
        .expect("valid review");
        assert!(
            plan.review_failures(&review)
                .expect("valid coverage")
                .is_empty()
        );
        review.coverage[1].complete = false;
        review.coverage[1].reason = "Add the missing notification page".into();
        let failures = plan.review_failures(&review).expect("actionable coverage");
        assert!(failures[0].contains("Notification page: Add the missing"));
        review.coverage[1].reason.clear();
        assert!(plan.review_failures(&review).is_err());
        review.coverage.pop();
        assert!(plan.review_failures(&review).is_err());
        review.reference_error = Some("Upload the missing theme screenshot".into());
        assert!(
            plan.review_failures(&review)
                .expect_err("missing reference must fail")
                .contains("reference: Upload")
        );
        plan.screens = vec![before.screens.len()];
        assert!(plan.validate(&before).is_err());
        plan.screens.clear();
        plan.requirements.clear();
        assert!(plan.validate(&before).is_err());
    }
}
