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
    #[serde(default)]
    workflow_groups: Vec<WorkflowGroup>,
}

#[derive(Deserialize, serde::Serialize)]
struct WorkflowGroup {
    name: String,
    #[serde(default)]
    screens: Vec<usize>,
    #[serde(default)]
    include_new_screens: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenRename {
    index: usize,
    name: String,
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
            workflow_groups: Vec::new(),
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

    fn check_group_scope(&self, repair_indexes: Option<&[usize]>) -> Result<(), String> {
        let mut assigned = std::collections::HashSet::new();
        let mut assigns_new = false;
        for group in &self.workflow_groups {
            if group.name.trim().is_empty()
                || group.name.trim() != group.name
                || group.name.contains(['·', '\n', '\r'])
                || (group.include_new_screens && std::mem::replace(&mut assigns_new, true))
                || group.screens.iter().any(|index| {
                    !assigned.insert(index)
                        || (!self.screens.is_empty() && !self.screens.contains(index))
                        || repair_indexes.is_some_and(|allowed| !allowed.contains(index))
                })
            {
                return Err("Cannot plan workflow groups: use one exact group per editable screen and at most one group for new screens.".into());
            }
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

fn has_group(name: &str, group: &str) -> bool {
    name.split_once('·')
        .is_some_and(|(prefix, title)| prefix.trim() == group && !title.trim().is_empty())
}

impl GenerationEngine {
    pub(super) async fn correct_workflow_names(
        &self,
        plan: &DemoEditRequirements,
        design: &mut Design,
        input: (&reqwest::Client, &str, usize),
        rounds_left: usize,
    ) -> Result<(), String> {
        let (client, effort, existing_count) = input;
        let pending: Vec<_> = plan
            .workflow_groups
            .iter()
            .flat_map(|group| {
                let additions = if group.include_new_screens {
                    existing_count..design.screens.len()
                } else {
                    0..0
                };
                group
                    .screens
                    .iter()
                    .copied()
                    .chain(additions)
                    .filter_map(|index| {
                        let screen = design.screens.get(index)?;
                        (!has_group(&screen.name, &group.name)).then_some((
                            index,
                            group.name.as_str(),
                            screen.name.clone(),
                        ))
                    })
            })
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let mut feedback = String::new();
        for _ in 0..rounds_left {
            let messages = [
                json!({"role": "system", "content": "Return only a JSON array of {index,name} objects, exactly one per supplied screen. Use the assigned workflow group verbatim, followed by ` · ` and a unique descriptive title that preserves the original screen's meaning and device qualifiers. Do not change screen indexes. Do not return HTML, CSS, or any other field."}),
                json!({"role": "user", "content": format!("Rename only these screens (zero-based index, assigned workflow, current name):\n{}\n{feedback}", json!(pending))}),
            ];
            let reply = self.model.chat(client, &messages, effort).await?;
            match apply_workflow_names(design, &pending, &reply) {
                Ok(()) => return Ok(()),
                Err(error) => feedback = format!("Previous naming patch rejected: {error}"),
            }
        }
        let screens = pending
            .iter()
            .map(|(index, group, _)| format!("{} ({group})", index + 1))
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "Workflow names still need correction on screens {screens}. {feedback}"
        ))
    }
}

fn apply_workflow_names(
    design: &mut Design,
    pending: &[(usize, &str, String)],
    reply: &str,
) -> Result<(), String> {
    let names = serde_json::from_str::<Vec<ScreenRename>>(reply)
        .map_err(|_| "Expected a JSON array of screen renames.".to_owned())?;
    let mut seen = std::collections::HashSet::new();
    if names.len() != pending.len()
        || names.iter().any(|rename| {
            !seen.insert(rename.index)
                || !pending.iter().any(|(index, group, _)| {
                    if *index != rename.index || !has_group(&rename.name, group) {
                        return false;
                    }
                    let mut screen = design.screens[*index].clone();
                    screen.name.clone_from(&rename.name);
                    screen.is_mobile(design.viewport)
                        == design.screens[*index].is_mobile(design.viewport)
                })
        })
    {
        return Err("The naming patch omitted a screen, changed its viewport tab, or used an unassigned screen or group.".into());
    }
    for rename in names {
        design.screens[rename.index].name = rename.name;
    }
    Ok(())
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
            "Resolve the current demo edit requirements. Return JSON only with instruction (a self-contained actionable brief), screens (zero-based existing screen indexes explicitly restricted by the user, or [] for candidate-wide work), minimum_new_screens (minimum additions needed to give requested visual views their own screens, 0 if existing screens suffice), requirements (a nonempty checklist of independently verifiable deliverables), and workflow_groups (an array of objects with name, screens, and include_new_screens). For a multi-screen workflow edit or retained draft repair, choose one short group name for the workflow, not for each screen or state. List the zero-based indexes of existing or retained companion screens to group, including the entry screen. Set include_new_screens to true if all new screens belong to that workflow; for unrelated new workflows, tell the writer their respective prefixes in the brief instead. Include only editable indexes and reuse the workflow’s existing prefix where possible. Return [] when no grouping is needed. Read the latest instruction together with the conversation: a clarification such as 'only candidate 4' retains the preceding feature request. Candidate numbers are NOT screen numbers. Ignore prior completed tasks and assistant-invented restrictions. Extending a demo can require BOTH modifying existing screens and adding connected screens; preserve both in the checklist. Include every requested feature, responsive variant, and relevant interaction/state requirement; do not substitute layout repairs. Give each separate requested page or independent visual deliverable its own checklist item, naming its intended screen, default content and navigation. Count only those views in minimum_new_screens. Require built-in actions for supported local interaction. Local states can share a screen. Hidden panels cannot replace separate requested pages. Do not turn permitted specification-only behavior into extra visual requirements. Distinguish proposed concepts from implemented features. A reference to an existing screen does not forbid requested additions. When additions are required, append them to preserve existing indexes. Name new pages by title. Do not assign a required screen number or literal link target unless the user explicitly fixed it; use links to the actual appended page. Inventory indexes are zero-based; visible screen numbers and #screen-N links are one-based. Treat an explicit destination that conflicts with preservation as a conflict, not permission to reassign an existing screen. Do not invent restrictions from worker assignments. Candidate write isolation is enforced by the server, not a visual deliverable; do not put it in the checklist. For restyling, preservation means retaining the existing content and behavior, not requiring identical HTML/CSS or implementing missing backend features. This is planning, not evidence of completion.\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}")}),
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
        if plan
            .workflow_groups
            .iter()
            .flat_map(|group| &group.screens)
            .any(|&index| index >= design.screens.len())
        {
            return Err("Cannot plan workflow groups: a screen index is out of range.".into());
        }
        plan.check_group_scope(repair_indexes)?;
        if !plan.workflow_groups.is_empty() {
            plan.instruction.push_str(&format!(
                "\nUse these exact workflow group assignments for screen names: {}",
                json!(plan.workflow_groups)
            ));
        }
        plan.instruction = format!(
            "{}\nBefore returning the patch, check it against these deliverables:\n{}\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}",
            plan.instruction,
            plan.requirements.join("\n")
        );
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::SAMPLE_DESIGN;

    use super::*;

    #[test]
    fn workflow_rename_rejects_unassigned_indexes_and_viewport_changes_atomically() {
        let mut design: Design =
            serde_json::from_str(SAMPLE_DESIGN).expect("sample design should deserialize");
        design.screens[0].name = "Overview · 390px mobile".into();
        let original = design.clone();
        let pending = [(0, "Task Details", design.screens[0].name.clone())];
        for reply in [
            r#"[{"index":1,"name":"Task Details · Overview mobile"}]"#,
            r#"[{"index":0,"name":"Task Details · Overview"}]"#,
            r#"[{"index":0,"name":"Overview · mobile"}]"#,
            r#"[{"index":0,"name":"Task Details · "}]"#,
        ] {
            apply_workflow_names(&mut design, &pending, reply)
                .expect_err("invalid rename must be rejected");
            assert_eq!(design, original);
        }
        apply_workflow_names(
            &mut design,
            &pending,
            r#"[{"index":0,"name":"Task Details · Overview mobile"}]"#,
        )
        .expect("valid name should be applied");
        assert_eq!(design.screens[0].name, "Task Details · Overview mobile");
        design.screens[0].name.clone_from(&original.screens[0].name);
        assert_eq!(design, original);
    }

    #[test]
    fn workflow_plan_cannot_assign_protected_screens() {
        let plan: DemoEditRequirements = serde_json::from_value(json!({
            "instruction": "Group task screens", "screens": [0], "minimum_new_screens": 0,
            "requirements": ["Group the task workflow"],
            "workflow_groups": [{"name": "Task Details", "screens": [0, 1]}]
        }))
        .expect("workflow plan should deserialize");
        plan.check_group_scope(None)
            .expect_err("screen 1 was not selected");
        let mut plan = plan;
        plan.screens.clear();
        plan.check_group_scope(Some(&[0]))
            .expect_err("screen 1 is protected in the retained draft");
        plan.check_group_scope(Some(&[0, 1]))
            .expect("both screens are editable");
    }
}
