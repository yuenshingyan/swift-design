//! Resolve demo edit scope before dispatch and check requirements before saving.

use design_model::Design;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::instructions::{DEMO_FIDELITY_RULE, DEMO_PROTOTYPE_RULE, DEMO_SCREEN_RULE};

use crate::edit_focus::EditOrder;
use crate::generation::{Attachments, GenerationEngine};

// This contract is included in planning, writing, correction, and review so
// the reviewer cannot silently raise the implementation fidelity after writing.
pub(super) const DEMO_ACCEPTANCE_CONTRACT: &str = "Demo acceptance contract: Safety, valid markup, protected-screen scope and core requested deliverables are mandatory. Minor spacing/alignment and other cosmetic audit findings are advisory, not reasons to reject an otherwise complete deliverable; unmeasurable layout groups are not defects. Do not inflate subjective polish preferences into missing core requirements. This artifact is a design prototype, not a production application. The renderer supports page links and built-in actions that change component state in place. It forbids scripts, event handlers, forms, buttons and editable text/date/time inputs. Do not change these restrictions. Distinguish rendered visual deliverables from implementation specifications in every checklist item. Require visible control designs, labelled input representations, responsive layouts and explicit state designs where requested; prose promising to design them later is insufficient. When the user permits designs or component specifications, accept complete, concrete specifications of those states and transitions, including persistent selection, automatic loading, date validation, duplicate prevention and clearing stale results; do not demand executable behavior the renderer forbids. Specifications must identify controls, state appearance, triggers, transitions and accessibility, not merely repeat the request. Do not replace requested high-fidelity desktop/mobile layouts with breakpoint prose alone. Preserve baseline navigation. When adding a page, link to its actual appended position by title and one-based screen number. Do not invent a required numeric position or move an existing page to satisfy an inferred position. Only classify a requirement as unsupported when the user explicitly requires executable behavior beyond this format and does not permit a design/specification alternative. Never silently drop such a requirement or claim backend integration.";

#[derive(Deserialize)]
struct DemoEditReview {
    coverage: Vec<RequirementReview>,
    reference_error: Option<String>,
    #[serde(default)]
    unsupported_requirements: Vec<String>,
}

#[derive(Deserialize)]
struct RequirementReview {
    complete: bool,
    #[serde(default)]
    findings: Vec<ReviewFinding>,
}

#[derive(Deserialize)]
struct ReviewFinding {
    id: String,
    reason: String,
    screens: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub(super) struct DemoEditFailure {
    pub requirement: usize,
    pub id: String,
    pub detail: String,
    pub screens: Vec<usize>,
}

// Compare defect identities and known affected screens, not changing review wording.
pub(super) fn review_improved(previous: &[DemoEditFailure], next: &[DemoEditFailure]) -> bool {
    let mut improved = next.len() < previous.len();
    for current in next {
        let Some(before) = previous
            .iter()
            .find(|old| old.requirement == current.requirement && old.id == current.id)
        else {
            return false;
        };
        if !before.screens.is_empty() {
            if current.screens.is_empty()
                || !current
                    .screens
                    .iter()
                    .all(|screen| before.screens.contains(screen))
            {
                return false;
            }
            improved |= current.screens.len() < before.screens.len();
        }
    }
    improved
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
    pub browser_evidence: &'a str,
    pub previous_failures: &'a [DemoEditFailure],
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

    pub(super) fn check_structure(
        &self,
        accepted: &Design,
        scope_baseline: &Design,
        after: &Design,
    ) -> Result<(), String> {
        if after.screens.len().saturating_sub(accepted.screens.len()) < self.minimum_new_screens {
            return Err(format!(
                "Cannot save demo edit: at least {} new screens were required; the edit did not add them.",
                self.minimum_new_screens
            ));
        }
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

    fn review_failures(
        &self,
        review: &DemoEditReview,
        screen_count: usize,
    ) -> Result<Vec<DemoEditFailure>, String> {
        if let Some(reason) = &review.reference_error {
            if reason.trim().is_empty() {
                return Err(
                    "Cannot verify demo edit: the reference error has no explanation.".into(),
                );
            }
            return Err(format!("Cannot verify demo edit reference: {reason}"));
        }
        if !review.unsupported_requirements.is_empty() {
            return Err(format!(
                "Cannot complete demo edit: unsupported executable requirements: {}; approve a design/specification alternative or a separate renderer change.",
                review.unsupported_requirements.join("; ")
            ));
        }
        if review.coverage.len() != self.requirements.len() {
            return Err("Cannot verify demo edit: the review omitted requirements.".into());
        }
        let mut missing = Vec::new();
        for (requirement, (item, result)) in
            self.requirements.iter().zip(&review.coverage).enumerate()
        {
            if result.complete != result.findings.is_empty() {
                return Err(
                    "Cannot verify demo edit: requirement status disagrees with its findings."
                        .into(),
                );
            }
            let mut ids = std::collections::HashSet::new();
            for finding in &result.findings {
                if finding.id.is_empty()
                    || finding.id.len() > 64
                    || !finding.id.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
                    || !ids.insert(&finding.id)
                    || finding.reason.trim().is_empty()
                {
                    return Err("Cannot verify demo edit: each defect needs a unique valid ID and an explanation.".into());
                }
                let mut screens = finding.screens.clone();
                if screens.iter().any(|&screen| screen >= screen_count) {
                    return Err("Cannot verify demo edit: a review finding names a screen outside the draft.".into());
                }
                screens.sort_unstable();
                screens.dedup();
                missing.push(DemoEditFailure {
                    requirement,
                    id: finding.id.clone(),
                    detail: format!("{item} [{}]: {}", finding.id, finding.reason),
                    screens,
                });
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
            .map(
                |(index, screen)| json!({"index": index, "number": index + 1, "name": screen.name}),
            )
            .collect();
        let messages = vec![
            json!({"role": "system", "content":
            "Resolve the current demo edit requirements. Return JSON only with instruction (a self-contained actionable brief), screens (zero-based existing screen indexes explicitly restricted by the user, or [] for candidate-wide work), minimum_new_screens (minimum additions needed to give requested visual views their own screens, 0 if existing screens suffice), and requirements (a nonempty checklist of independently verifiable deliverables). Read the latest instruction together with the conversation: a clarification such as 'only candidate 4' retains the preceding feature request. Candidate numbers are NOT screen numbers. Ignore prior completed tasks and assistant-invented restrictions. Extending a demo can require BOTH modifying existing screens and adding connected screens; preserve both in the checklist. Include every requested feature, responsive variant, and relevant interaction/state requirement; do not substitute layout repairs. Give each separate requested page or independent visual deliverable its own checklist item, naming its intended screen, default content and navigation. Count only those views in minimum_new_screens. Require built-in actions for supported local interaction. Local states can share a screen. Hidden panels cannot replace separate requested pages. Do not turn permitted specification-only behavior into extra visual requirements. Distinguish proposed concepts from implemented features. A reference to an existing screen does not forbid requested additions. When additions are required, append them to preserve existing indexes. Name new pages by title. Do not assign a required screen number or literal link target unless the user explicitly fixed it; use links to the actual appended page. Inventory indexes are zero-based; visible screen numbers and #screen-N links are one-based. Treat an explicit destination that conflicts with preservation as a conflict, not permission to reassign an existing screen. Do not invent restrictions from worker assignments. Candidate write isolation is enforced by the server, not a visual deliverable; do not put it in the checklist. For restyling, preservation means retaining the existing content and behavior, not requiring identical HTML/CSS or implementing missing backend features. This is planning, not evidence of completion."}),
            self.user_message(&format!(
                "Conversation:\n{}\nLatest instruction:\n{}\nRegenerate selected screens: {}\nExisting screens:\n{}",
                order.conversation, order.instruction, order.is_fresh, json!(inventory)
            ), attachments),
        ];
        let mut messages = messages;
        messages[0]["content"] = json!(format!(
            "{}\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}",
            messages[0]["content"].as_str().unwrap_or_default()
        ));
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
            "{}\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}",
            plan.instruction
        );
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
    ) -> Result<Vec<DemoEditFailure>, String> {
        let messages = vec![
            json!({"role": "system", "content":
            "Review demo requirement coverage before saving. Return only a JSON object with coverage (one object per checklist item in order, each with complete: boolean and findings: array of objects with id, reason, and screens) and reference_error (null unless a required reference cannot be inspected, then a specific explanation and recovery action), plus unsupported_requirements (an array of explicitly requested executable capabilities the renderer cannot support, empty otherwise). Report each independently fixable defect as a separate finding; for example, a missing empty state and stale selection are two findings, even under the same checklist item on the same screen. Give each finding a lowercase kebab-case id of at most 64 characters, a reason with concrete evidence and an actionable correction, and the zero-based draft indexes of screens needing that fix. Use an empty screens array when that fix needs a new screen, shared settings, or has no reliable screen scope. Do not list unaffected screens. Recheck the previous findings independently and reuse the same id under the same checklist item for the same unresolved defect, even if wording or affected screens change. Omit fixed defects, and use a new id for a genuinely new defect. Mark complete exactly when findings is empty. Use the attached references when judging theme matching; do not invent reference colors or fonts. If a required reference is missing or unreadable, report reference_error rather than treating it as an incomplete edit. Judge rendered visual requirements from HTML/CSS and navigation; judge specification-based requirements from complete concrete component/state specifications when the user permits them, not promises of future work. For a styling-only request, theme/CSS-only changes are valid. Require new features or screens only when requested by the current task; a layout-only patch is insufficient only when feature work was requested. Mark missing, partial, disconnected, or unverifiable requirements false. For each requested page or independent visual deliverable, identify its separate screen by number/name and check its default content. Check that page links lead to the intended screen by name and actual position. A number or literal target introduced only by the plan is not a requirement: do not report it as a defect if the link reaches the intended page and existing destinations are preserved. Require a specific number only if the user explicitly fixed it in the original request. Do not move an existing page to satisfy an inferred number. Local selections, tabs, and overlays may use built-in prototype actions on one screen. Check that each action points to the correct local state. Do not require a separate screen for each local state unless the user requests separate screens or independent exports. Hidden panels cannot replace separate requested pages. Give an actionable correction to append the missing independent screens and connect them, preserving unselected existing screens. Ordinary local menu expansion and incidental toggles are allowed; do not reject unrelated legacy patterns or invent extra states during a restyle. Compare the checklist with the original conversation and latest request; if the plan omitted requested work, mark all items false rather than accepting a narrowed brief. Evaluate only the current task and its clarifications, not earlier completed tasks or assistant-invented restrictions; the latest user requirements resolve conflicts. Requested supported demo interactions must be present in the artifact; renderer-forbidden behavior may instead have a concrete implementation specification when the task permits it. For preservation requirements, compare the before and after artifacts; do not demand identical markup for a styling change, new backend functionality, or responsive behavior absent from the baseline. Candidate isolation is a server-enforced fact: the pending write can only save the identified candidate, not other candidates. Do not reject a candidate-scope checklist item because other candidates are not supplied. Use the supplied browser results for supported prototype actions. Do not claim a physical Tab or Enter key test from a programmatic probe. Do not reject an otherwise complete requirement only because you cannot rerun the browser yourself. Missing evidence is not a measured failure. Report concrete defects from the artifact or a reported failed check. Do not require a save receipt: this review runs before saving. The server owns browser verification and saving. This review does not test a backend."}),
            self.user_message(&format!(
                "Pending save target (server-enforced): {}\nOriginal request:\n{}\nEdit brief:\n{}\nChecklist:\n{}\nPrevious individual findings (recheck each; reuse IDs for unresolved defects):\n{}\nBrowser prototype results (empty means no probes recorded):\n{}\nBefore artifact:\n{}\nAfter artifact:\n{}",
                evidence.candidate_id, plan.source, plan.instruction, json!(plan.requirements),
                json!(evidence.previous_failures), evidence.browser_evidence, json!(evidence.before), json!(evidence.after)
            ), evidence.attachments),
        ];
        let mut messages = messages;
        messages[0]["content"] = json!(format!(
            "{}\n{DEMO_FIDELITY_RULE}\n{DEMO_ACCEPTANCE_CONTRACT}\n{DEMO_SCREEN_RULE}\n{DEMO_PROTOTYPE_RULE}",
            messages[0]["content"].as_str().unwrap_or_default()
        ));
        let reply = self.model.chat(client, &messages, effort).await?;
        let review: DemoEditReview = serde_json::from_str(&reply)
            .map_err(|_| "Cannot verify demo edit: expected a JSON coverage review.".to_owned())?;
        plan.review_failures(&review, evidence.after.screens.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_tracks_independent_defects_on_the_same_screen_not_wording() {
        let finding = |id: &str, screens: &[usize], detail: &str| DemoEditFailure {
            requirement: 0,
            id: id.into(),
            screens: screens.to_vec(),
            detail: detail.into(),
        };
        let before = [
            finding("stale-detail", &[1, 2], "Two affected screens"),
            finding("empty-state", &[1], "Missing empty state"),
        ];
        assert!(review_improved(
            &before,
            &[
                finding("stale-detail", &[1], "One screen left"),
                finding("empty-state", &[1], "Still missing")
            ]
        ));
        assert!(review_improved(
            &before,
            &[finding(
                "stale-detail",
                &[1, 2],
                "Empty state fixed; stale detail remains"
            )]
        ));
        assert!(!review_improved(
            &before,
            &[
                finding("stale-detail", &[1, 2], "Same defect, new wording"),
                finding("empty-state", &[1], "Still missing")
            ]
        ));
        assert!(!review_improved(
            &before,
            &[finding("keyboard", &[1], "New defect on the same screen")]
        ));
        assert!(!review_improved(
            &before,
            &[finding("stale-detail", &[3], "New affected screen")]
        ));
        assert!(!review_improved(
            &before,
            &[finding("stale-detail", &[], "Scope became unknown")]
        ));
    }

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
        assert!(plan.check_structure(&before, &before, &before).is_err());
        let mut after = before.clone();
        after.screens.push(before.screens[0].clone());
        assert!(plan.check_structure(&before, &before, &after).is_ok());
        after.screens[1].notes = Some("Unrelated change".into());
        assert!(plan.check_structure(&before, &before, &after).is_err());
        let mut review: DemoEditReview = serde_json::from_value(json!({
            "coverage": [
                {"complete": true, "reason": "Comments present"},
                {"complete": true, "reason": "Notification page present"}
            ],
            "reference_error": null
        }))
        .expect("valid review");
        assert!(
            plan.review_failures(&review, after.screens.len())
                .expect("valid coverage")
                .is_empty()
        );
        review.coverage[1].complete = false;
        assert!(plan.review_failures(&review, after.screens.len()).is_err());
        review.coverage[1].findings = vec![ReviewFinding {
            id: "missing-page".into(),
            reason: "Add the missing notification page".into(),
            screens: vec![2, 0, 2],
        }];
        let failures = plan
            .review_failures(&review, after.screens.len())
            .expect("actionable coverage");
        assert!(
            failures[0]
                .detail
                .contains("Notification page [missing-page]: Add the missing")
        );
        assert_eq!(failures[0].screens, [0, 2]);
        review.coverage[1].findings[0].screens = vec![after.screens.len()];
        assert!(
            plan.review_failures(&review, after.screens.len())
                .expect_err("invalid review index should fail")
                .contains("outside the draft")
        );
        review.coverage[1].findings[0].screens.clear();
        review.coverage[1].findings.push(ReviewFinding {
            id: "missing-page".into(),
            reason: "Do not count the same defect twice".into(),
            screens: vec![],
        });
        assert!(plan.review_failures(&review, after.screens.len()).is_err());
        review.coverage[1].findings.pop();
        review.coverage[1].findings[0].reason.clear();
        assert!(plan.review_failures(&review, after.screens.len()).is_err());
        review.coverage.pop();
        assert!(plan.review_failures(&review, after.screens.len()).is_err());
        review.reference_error = Some("Upload the missing theme screenshot".into());
        assert!(
            plan.review_failures(&review, after.screens.len())
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
