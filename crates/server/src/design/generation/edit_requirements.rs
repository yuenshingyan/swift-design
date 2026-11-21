//! Resolve demo edit scope and check structural requirements before saving.

use design_model::Design;
use serde::Deserialize;
use serde_json::json;

use crate::edit_focus::EditOrder;
use crate::generation::{Attachments, GenerationEngine};
use crate::instructions::{DEMO_FIDELITY_RULE, DEMO_PROTOTYPE_RULE, DEMO_SCREEN_RULE};

pub(super) const DEMO_ACCEPTANCE_CONTRACT: &str = "Demo acceptance contract: Safety, valid markup, protected-screen scope and core requested deliverables are mandatory. Minor spacing/alignment and other cosmetic audit findings are advisory, not reasons to reject an otherwise complete deliverable; unmeasurable layout groups are not defects. Do not inflate subjective polish preferences into missing core requirements. This artifact is a design prototype, not a production application. The renderer supports page links and built-in actions that change component state in place. It forbids scripts, event handlers, forms, buttons and editable text/date/time inputs. Do not change these restrictions. Distinguish rendered visual deliverables from implementation specifications in every checklist item. Require visible control designs, labelled input representations, responsive layouts and explicit state designs where requested; prose promising to design them later is insufficient. When the user permits designs or component specifications, accept complete, concrete specifications of those states and transitions, including persistent selection, automatic loading, date validation, duplicate prevention and clearing stale results; do not demand executable behavior the renderer forbids. Specifications must identify controls, state appearance, triggers, transitions and accessibility, not merely repeat the request. Do not replace requested high-fidelity desktop/mobile layouts with breakpoint prose alone. Preserve baseline navigation. When adding a page, link to its actual appended position by title and one-based screen number. Do not invent a required numeric position or move an existing page to satisfy an inferred position. Only classify a requirement as unsupported when the user explicitly requires executable behavior beyond this format and does not permit a design/specification alternative. Never silently drop such a requirement or claim backend integration.";

#[derive(Deserialize)]
pub(super) struct DemoEditRequirements {
    pub instruction: String,
    pub screens: Vec<usize>,
    pub minimum_new_screens: usize,
    requirements: Vec<String>,
}

impl DemoEditRequirements {
    pub(super) fn from_annotations(batch: &crate::design::annotations::AnnotationBatch) -> Self {
        let requirements = batch.requirements();
        Self {
            instruction: format!(
                "Update only the commented regions. Preserve every other element, screen order, link, and shared setting. Use the exact screen markup and the numbered boxes in the reference PNGs (not assets to embed). Coordinates use the rendered canvas with its top left as the origin.\n{}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}",
                requirements.join("\n")
            ),
            screens: batch.indexes(),
            minimum_new_screens: 0,
            requirements,
        }
    }

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

    pub(super) fn check_scope(
        &self,
        scope_baseline: &Design,
        after: &Design,
    ) -> Result<(), String> {
        if !self.screens.is_empty()
            && scope_baseline
                .screens
                .iter()
                .enumerate()
                .any(|(index, screen)| {
                    !self.screens.contains(&index) && after.screens.get(index) != Some(screen)
                })
        {
            return Err("Cannot save demo edit: an unselected screen changed.".into());
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
        review_input: (&Attachments, &str, Option<&[usize]>),
    ) -> Result<DemoEditRequirements, String> {
        let (attachments, effort, repair_indexes) = review_input;
        let repair_scope = repair_indexes.map_or_else(String::new, |indexes| format!(
            "\nRepair draft: only these existing zero-based screen indexes are editable: {indexes:?}. Keep shared settings and retained screen positions. Include in screens every existing screen the user's repair actually requires changing, even if protected, so the server can report a scope conflict before writing. Do not infer edits to other screens."
        ));
        let inventory: Vec<_> = design
            .screens
            .iter()
            .enumerate()
            .map(
                |(index, screen)| json!({"index": index, "number": index + 1, "name": screen.name}),
            )
            .collect();
        let messages = vec![
            json!({"role": "system", "content": format!(
            "Resolve the current demo edit requirements. Return JSON only with instruction (a self-contained actionable brief), screens (zero-based existing screen indexes explicitly restricted by the user, or [] for candidate-wide work), minimum_new_screens (minimum additions needed to give requested visual views their own screens, 0 if existing screens suffice), and requirements (a nonempty checklist of independently verifiable deliverables). Read the latest instruction together with the conversation: a clarification such as 'only candidate 4' retains the preceding feature request. Candidate numbers are NOT screen numbers. Ignore prior completed tasks and assistant-invented restrictions. Extending a demo can require BOTH modifying existing screens and adding connected screens; preserve both in the checklist. Include every requested feature, responsive variant, and relevant interaction/state requirement; do not substitute layout repairs. Give each separate requested page or independent visual deliverable its own checklist item, naming its intended screen, default content and navigation. Count only those views in minimum_new_screens. Require built-in actions for supported local interaction. Local states can share a screen. Hidden panels cannot replace separate requested pages. Do not turn permitted specification-only behavior into extra visual requirements. Distinguish proposed concepts from implemented features. A reference to an existing screen does not forbid requested additions. When additions are required, append them to preserve existing indexes. Name new pages by title. Do not assign a required screen number or literal link target unless the user explicitly fixed it; use links to the actual appended page. Inventory indexes are zero-based; visible screen numbers and #screen-N links are one-based. Treat an explicit destination that conflicts with preservation as a conflict, not permission to reassign an existing screen. Do not invent restrictions from worker assignments. Candidate write isolation is enforced by the server, not a visual deliverable; do not put it in the checklist. For restyling, preservation means retaining the existing content and behavior, not requiring identical HTML/CSS or implementing missing backend features. This is planning, not evidence of completion.\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}")}),
            self.user_message(&format!(
                "Conversation:\n{}\nLatest instruction:\n{}\nRegenerate selected screens: {}\nExisting screens:\n{}{repair_scope}",
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
        plan.instruction = format!(
            "{}\nBefore returning the patch, check it against these deliverables:\n{}\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}",
            plan.instruction,
            plan.requirements.join("\n")
        );
        Ok(plan)
    }
}
