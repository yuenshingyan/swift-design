//! Resolve demo edit scope before dispatch and check requirements before saving.

use design_model::Design;
use serde::Deserialize;
use serde_json::json;

use super::super::{EditOrder, GenerationEngine};

#[derive(Deserialize)]
pub(super) struct DemoEditRequirements {
    pub instruction: String,
    pub screens: Vec<usize>,
    pub minimum_new_screens: usize,
    pub requirements: Vec<String>,
    #[serde(skip)]
    source: String,
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

    fn check_coverage(&self, coverage: &[bool]) -> Result<(), String> {
        if coverage.len() != self.requirements.len() {
            return Err("Cannot verify demo edit: the review omitted requirements.".into());
        }
        let missing: Vec<&str> = self
            .requirements
            .iter()
            .zip(coverage)
            .filter(|(_, complete)| !**complete)
            .map(|(item, _)| item.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "Cannot save incomplete demo edit: {}",
                missing.join("; ")
            ));
        }
        Ok(())
    }
}

impl GenerationEngine {
    pub(super) async fn plan_demo_edit(
        &self,
        client: &reqwest::Client,
        design: &Design,
        order: &EditOrder<'_>,
        effort: &str,
    ) -> Result<DemoEditRequirements, String> {
        let inventory: Vec<_> = design
            .screens
            .iter()
            .enumerate()
            .map(|(index, screen)| json!({"index": index, "name": screen.name}))
            .collect();
        let messages = vec![
            json!({"role": "system", "content":
            "Resolve the current demo edit requirements. Return JSON only with instruction (a self-contained actionable brief), screens (zero-based existing screen indexes explicitly restricted by the user, or [] for candidate-wide work), minimum_new_screens (minimum additions the user requires, 0 if none), and requirements (a nonempty checklist of independently verifiable deliverables). Read the latest instruction together with the conversation: a clarification such as 'only candidate 4' retains the preceding feature request. Candidate numbers are NOT screen numbers. Ignore prior completed tasks and assistant-invented restrictions. Extending a demo can require BOTH modifying existing screens and adding connected screens; preserve both in the checklist. Include every requested feature, responsive variant, and relevant interaction/state requirement; do not substitute layout repairs. Distinguish proposed concepts from implemented features. A reference to an existing screen does not forbid requested additions. When additions are required, append them to preserve existing indexes. Do not invent restrictions from worker assignments. This is planning, not evidence of completion."}),
            json!({"role": "user", "content": format!(
                "Conversation:\n{}\nLatest instruction:\n{}\nRegenerate selected screens: {}\nExisting screens:\n{}",
                order.conversation, order.instruction, order.is_fresh, json!(inventory)
            )}),
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
        design: &Design,
        effort: &str,
    ) -> Result<(), String> {
        let messages = vec![
            json!({"role": "system", "content":
            "Review demo requirement coverage before saving. Return only a JSON array of booleans, one per checklist item in order. Judge actual HTML/CSS, navigation destinations and interaction states, not notes or promises. A layout-only patch does not implement feature demos. Mark missing, partial, disconnected, or unverifiable requirements false. Compare the checklist with the original conversation and latest request; if the plan omitted requested work, mark all items false rather than accepting a narrowed brief. Evaluate only the current task and its clarifications, not earlier completed tasks or assistant-invented restrictions; the latest user requirements resolve conflicts. Proposed demo interactions need not have a backend, but must be present in the artifact. This review is not a browser or backend test."}),
            json!({"role": "user", "content": format!(
                "Original request:\n{}\nEdit brief:\n{}\nChecklist:\n{}\nFinal artifact:\n{}",
                plan.source, plan.instruction, json!(plan.requirements), json!(design)
            )}),
        ];
        let reply = self.model.chat(client, &messages, effort).await?;
        let coverage: Vec<bool> = serde_json::from_str(&reply)
            .map_err(|_| "Cannot verify demo edit: expected a JSON coverage array.".to_owned())?;
        plan.check_coverage(&coverage)
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
        assert!(plan.check_coverage(&[true, true]).is_ok());
        assert!(plan.check_coverage(&[true]).is_err());
        assert!(
            plan.check_coverage(&[true, false])
                .expect_err("missing feature must fail")
                .contains("Notification page")
        );
        plan.screens = vec![before.screens.len()];
        assert!(plan.validate(&before).is_err());
        plan.screens.clear();
        plan.requirements.clear();
        assert!(plan.validate(&before).is_err());
    }
}
