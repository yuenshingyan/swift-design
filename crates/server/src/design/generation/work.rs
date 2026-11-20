use std::collections::HashSet;

use super::edit_requirements::{DemoEditEvidence, DemoEditFailure, review_improved};
use super::*;

const MAX_DEMO_CORRECTIONS: usize = 10;

fn require_repair_scope(
    before: &Design,
    retained: &Design,
    edited: &Design,
) -> Result<(), GenerationStop> {
    let problem = if edited.title != retained.title {
        Some("the title changed".to_owned())
    } else if edited.theme != retained.theme {
        Some("the theme changed".to_owned())
    } else if edited.app_window != retained.app_window {
        Some("the shared app window changed".to_owned())
    } else if edited.screens.len() < retained.screens.len() {
        Some("a retained screen was removed".to_owned())
    } else {
        before
            .screens
            .iter()
            .zip(&retained.screens)
            .zip(&edited.screens)
            .enumerate()
            .find(|(_, ((saved, retained), edited))| saved == retained && edited != saved)
            .map(|(index, ((_, screen), _))| {
                format!("protected screen {} ({}) changed", index + 1, screen.name)
            })
    };
    if let Some(problem) = problem {
        return Err(GenerationStop::Failed(format!(
            "Cannot repair draft: {problem}."
        )));
    }
    Ok(())
}

impl GenerationEngine {
    /// Writes one design per requested variation. Returns the ids.
    pub(crate) async fn generate_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let count = context.options.variation_count();
        let attachments = Arc::new(self.load_attachments(&context.session_id, log).await);
        let concepts = if count > 1 {
            self.plan_concepts(client, context, count, &attachments, log)
                .await?
        } else {
            Vec::new()
        };
        self.report_progress(10);
        let base = context.session_id.clone();
        // One design per platform per variation: the same concept drawn
        // on each canvas the user picked. The studio groups them by
        // canvas, so the numbering runs straight through.
        // The canvases are the app's answer, held on the session. The
        // brief mirrors them, but only from the revision after the pick,
        // and a user who picks a canvas at approval writes no revision.
        let platforms = context.request.platforms();
        // A later run numbers after the candidates the session has, so
        // it adds to them instead of overwriting them.
        let first_number = match self.designs.list().await {
            Ok(rows) => next_candidate_number(&base, rows.iter().map(|row| row.id.as_str())),
            Err(_) => 1,
        };
        let plans = candidate_plans(&base, &platforms, count, first_number);
        let ids: Vec<String> = plans.iter().map(|plan| plan.design_id.clone()).collect();
        if plans.len() > count {
            log(&format!(
                "writing {} candidates: {count} variations across {} canvases",
                plans.len(),
                platforms.len()
            ));
        }
        let shares = self.shared_progress(&ids, 10, 90);
        let templates = self.brief_templates(&context.options, log).await;
        // Every candidate runs at the same time; each saves itself as
        // soon as it is ready.
        let mut tasks = tokio::task::JoinSet::new();
        for (index, plan) in plans.iter().enumerate() {
            let engine = self.clone();
            let client = client.clone();
            let context = context.clone();
            let concepts = concepts.clone();
            let candidate_number = plan.candidate_number;
            let variation = plan.variation;
            let viewport = plan.viewport;
            // One look per variation, wrapping when the user picked
            // fewer templates than variations.
            let template = candidate_template(&templates, variation);
            let attachments = Arc::clone(&attachments);
            let share = Arc::clone(&shares[index]);
            let log = Arc::clone(log);
            let id = plan.design_id.clone();
            // The card appears at once, as a placeholder with its bar.
            share(0.0);
            tasks.spawn(async move {
                let request = CandidateRequest {
                    context: &context,
                    candidate_number: variation,
                    viewport,
                    concepts: &concepts,
                    preview_screens: context.preview_screens(),
                    design_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_candidate(&client, &request, &attachments, &share, &log)
                    .await?;
                log(&format!("candidate {candidate_number}: saved as {id}"));
                Ok::<String, GenerationStop>(id)
            });
        }
        let mut saved = Vec::new();
        let mut failures = Vec::new();
        while let Some(outcome) = tasks.join_next().await {
            match outcome {
                Ok(Ok(id)) => saved.push(id),
                Ok(Err(GenerationStop::NeedsClarification(set))) => {
                    tasks.shutdown().await;
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Ok(Err(GenerationStop::Failed(error))) => failures.push(error),
                Err(error) => failures.push(format!("candidate task failed: {error}")),
            }
        }
        // Presence on disk is not completion: failed polish may retain an
        // explicitly unverified review draft under its reserved id.
        let saved: Vec<_> = ids.into_iter().filter(|id| saved.contains(id)).collect();
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no candidate passed verification",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Combines parts of `sources` into one new candidate, as
    /// `instruction` asks, and returns its id. The sources must share a
    /// canvas: a phone screen and a desktop screen do not merge. The
    /// new candidate takes the next free number and goes through the
    /// same fix and polish rounds as a fresh candidate.
    pub(crate) async fn merge_designs(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let mut loaded = Vec::new();
        for id in sources {
            let design = self
                .designs
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("design `{id}` does not exist")))?;
            loaded.push((id.as_str(), design));
        }
        let Some((first_id, first)) = loaded.first() else {
            return Err(GenerationStop::Failed(
                "a merge needs two candidates: pin them with @".to_owned(),
            ));
        };
        let viewport = first.viewport;
        if let Some((other_id, _)) = loaded
            .iter()
            .find(|(_, design)| design.viewport != viewport)
        {
            return Err(GenerationStop::Failed(format!(
                "`{first_id}` and `{other_id}` are on different canvases: merge candidates of one \
                 canvas"
            )));
        }
        let merge = MergeInput {
            sources: merge_sources(&loaded).map_err(GenerationStop::Failed)?,
            instruction: instruction.to_owned(),
        };
        let base = context.session_id.as_str();
        let rows = self
            .designs
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let design_id = candidate_id(base, number);
        log(&format!("merging {} into {design_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&design_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = CandidateRequest {
            context,
            candidate_number: number,
            viewport,
            concepts: &[],
            preview_screens: None,
            design_id: design_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {design_id}"));
        Ok(design_id)
    }

    /// Applies a critique to one chosen design: the model rewrites the
    /// design against the brief and the critique, the result is
    /// validated, polished, and saved under the same id.
    /// Applies `instruction` to each design in turn and returns the ones
    /// it saved. One failure is logged and the rest still run; the turn
    /// fails only when every edit failed.
    pub(crate) async fn edit_designs(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        edit: (&EditOrder<'_>, bool),
        log: &LogSink,
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let (order, _) = edit;
        let mut saved = Vec::new();
        let mut last_error = None;
        for design_id in order.artifact_ids {
            match self
                .edit_design(client, context, design_id, edit, log)
                .await
            {
                Ok(report) => saved.push((design_id.clone(), report)),
                // A question from the model halts the whole turn: the
                // user answers it before any other edit.
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {design_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    pub(crate) async fn save_review_draft(
        &self,
        source_id: &str,
        design: &Design,
        issue: &str,
        is_new: bool,
    ) -> Result<String, String> {
        if !design.validate().is_empty() {
            return Err("Cannot preserve review draft: the artifact failed validation.".into());
        }
        if is_new {
            let mut draft = design.clone();
            draft.title = format!("Needs review — {}", draft.title);
            self.designs
                .save(source_id, &draft)
                .await
                .map_err(|error| error.to_string())?;
        } else {
            self.designs
                .keep_review_draft(source_id, design, None)
                .await
                .map_err(|error| error.to_string())?;
        }
        self.review_draft_notice(source_id, issue, is_new).await
    }

    async fn review_draft_notice(
        &self,
        source_id: &str,
        issue: &str,
        is_new: bool,
    ) -> Result<String, String> {
        self.notifier.notify();
        let preservation = if is_new {
            "This is a draft, not a completed candidate."
        } else {
            "The saved version is unchanged. Open the same candidate to see the review draft."
        };
        let message = format!(
            "Needs review: preserved draft for `{source_id}`. {preservation} Layout and requirement completion are not verified."
        );
        self.sessions
            .append_message(
                session_id_of_artifact(source_id),
                ChatMessage::assistant(&message).with_artifacts(vec![source_id.to_owned()]),
            )
            .await
            .map_err(|error| format!("{message}\n{issue}\nCannot record draft notice: {error}"))?;
        Ok(format!("{message}\n{issue}"))
    }

    pub(crate) async fn edit_design(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        design_id: &str,
        edit: (&EditOrder<'_>, bool),
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let (order, use_review_draft) = edit;
        let design = self
            .designs
            .load(design_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("design `{design_id}` does not exist"))
            })?;
        let label = format!("edit {design_id}");
        let mut draft = if use_review_draft {
            log(&format!("{label}: repairing the retained review draft"));
            self.designs
                .review_draft_for_edit(design_id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
        } else {
            design.clone()
        };
        let retained = use_review_draft.then(|| draft.clone());
        // A repair can name pages that exist only in the retained draft.
        let scope_baseline = retained.as_ref().unwrap_or(&design);
        let repair_indexes: Option<Vec<_>> = retained.as_ref().map(|retained| {
            retained
                .screens
                .iter()
                .enumerate()
                .filter_map(|(index, screen)| {
                    (design.screens.get(index) != Some(screen)).then_some(index)
                })
                .collect()
        });
        log(&format!("{label}: resolving edit requirements and scope"));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let plan = self
            .plan_demo_edit(
                client,
                scope_baseline,
                order,
                (&attachments, context.effort(), repair_indexes.as_deref()),
            )
            .await
            .map_err(GenerationStop::Failed)?;
        let instruction = plan.instruction.as_str();
        // Structural edits need the full artifact in coordination. Workers remain
        // isolated; the requirements check protects unselected existing screens.
        let additions_already_in_draft = draft.screens.len().saturating_sub(design.screens.len());
        let needs_new_screens = plan.minimum_new_screens > additions_already_in_draft;
        if let Some(allowed) = &repair_indexes {
            if let Some(&index) = plan.screens.iter().find(|index| !allowed.contains(index)) {
                return Err(GenerationStop::Failed(format!(
                    "Cannot repair draft: screen {} ({}) is unchanged in this draft. To change it, start a separate edit from the saved version.",
                    index + 1,
                    scope_baseline.screens[index].name
                )));
            }
            if allowed.is_empty() && !needs_new_screens {
                return Err(GenerationStop::Failed(
                    "Cannot repair draft: no existing screens differ from the saved version."
                        .into(),
                ));
            }
        }
        let indexes = if needs_new_screens {
            Vec::new()
        } else if let Some(allowed) = &repair_indexes
            && plan.screens.is_empty()
        {
            allowed.clone()
        } else {
            plan.screens.clone()
        };
        log(&format!("{label}: checking layout before editing"));
        let measured = self.audit_demo(&draft).await?.instructions(None);
        let preview_scope = repair_indexes
            .clone()
            .unwrap_or_else(|| plan.screens.clone());
        let preview = self.designs.start_preview(
            design_id,
            &self.notifier,
            Some((design.clone(), preview_scope)),
        );
        if use_review_draft {
            preview.show(&draft);
        }
        let mut correction = if let Some(allowed) = &repair_indexes {
            format!(
                "The supplied design is the retained draft, not the accepted baseline. Repair only the remaining issues. Editable existing screen indexes (zero-based): {allowed:?}. Keep shared settings, all retained screen positions, and working interactions. Append new screens only if the plan requires missing screens."
            )
        } else {
            String::new()
        };
        let mut attempt = 0;
        let mut previous_failures: Option<(Design, Vec<DemoEditFailure>)> = None;
        let mut navigation_errors = HashSet::new();
        let mut correction_screens = Vec::new();
        let progress = self.shared_progress(&[design_id.to_owned()], 5, 95).pop();
        let (final_design, verified_indexes, layout_warnings) = loop {
            // Fresh regeneration drops old markup only once; corrections must
            // see and repair the latest draft, including newly added screens.
            let is_fresh = order.is_fresh && attempt == 0;
            let total = draft.screens.len();
            let is_focused_correction = !correction_screens.is_empty();
            let write_indexes = if is_focused_correction {
                &correction_screens
            } else {
                &indexes
            };
            let (design_json, note) = if write_indexes.is_empty() {
                (serde_json::to_string(&draft), String::new())
            } else if is_fresh {
                (
                    focused_design_json(&draft, write_indexes, true),
                    fresh_note("screen", "screens", write_indexes, total),
                )
            } else {
                (
                    focused_design_json(&draft, write_indexes, false),
                    focus_note("screen", "screens", write_indexes, total),
                )
            };
            let design_json =
                design_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
            let correction_instruction = format!("{instruction}\n{correction}");
            let is_focused_patch =
                is_focused_correction || (use_review_draft && !write_indexes.is_empty());
            let allowed_replacements = repair_indexes.as_ref().map(|allowed| {
                let mut allowed: Vec<_> = allowed
                    .iter()
                    .copied()
                    .filter(|index| plan.screens.is_empty() || plan.screens.contains(index))
                    .collect();
                allowed.extend(scope_baseline.screens.len()..total);
                allowed
            });
            let can_append = use_review_draft
                && write_indexes.is_empty()
                && plan.minimum_new_screens > total.saturating_sub(design.screens.len());
            let findings = findings_for(&measured, "screens", write_indexes);
            let input = EditInput {
                instruction: &correction_instruction,
                artifact_json: &design_json,
                note: &note,
                findings: &findings,
                conversation: order.conversation,
            };
            let original = draft.clone();
            let repair_screens = if is_focused_patch {
                write_indexes.clone()
            } else {
                Vec::new()
            };
            let request = ArtifactRequest {
                effort: context.effort().to_owned(),
                label: if attempt == 0 {
                    label.clone()
                } else {
                    format!("{label}: correction round {attempt}/{MAX_DEMO_CORRECTIONS}")
                },
                parse: Box::new(move |content| {
                    let patch = parse_patch(content)?;
                    if let Some(allowed) = &allowed_replacements {
                        if let Some((_, setting)) = [
                            (patch.title.is_some(), "title"),
                            (patch.theme.is_some(), "theme"),
                            (patch.app_window.is_some(), "app window"),
                        ]
                        .into_iter()
                        .find(|(changed, _)| *changed)
                        {
                            return Err(format!(
                                "Repair draft cannot change the shared {setting}; omit that field."
                            ));
                        }
                        for operation in &patch.screens {
                            let index = operation.index;
                            if operation.insert
                                && can_append
                                && index == total
                                && operation.screen.is_some()
                            {
                                continue;
                            }
                            if operation.insert || operation.screen.is_none() {
                                return Err(format!(
                                    "Repair draft cannot insert or remove a screen at index {index}; keep retained screen positions."
                                ));
                            }
                            if !allowed.contains(&index) {
                                let target = original.screens.get(index).map_or_else(
                                    || format!("unknown screen index {index}"),
                                    |screen| {
                                        format!(
                                            "protected screen {} ({}, index {index})",
                                            index + 1,
                                            screen.name
                                        )
                                    },
                                );
                                return Err(format!(
                                    "Repair draft cannot replace {target}; omit it. Editable indexes: {allowed:?}."
                                ));
                            }
                        }
                    }
                    if !repair_screens.is_empty()
                        && (patch.title.is_some()
                            || patch.theme.is_some()
                            || patch.app_window.is_some()
                            || patch.screens.iter().any(|operation| {
                                operation.insert
                                    || operation.screen.is_none()
                                    || !repair_screens.contains(&operation.index)
                            }))
                    {
                        return Err("A focused correction may replace only the specified screens. Do not insert screens, delete screens, or change shared settings.".into());
                    }
                    apply_patch(&original, patch)
                }),
                progress: progress.clone(),
                live: (!is_focused_patch && !use_review_draft).then(|| preview.edit_sink(&draft)),
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                self.user_message(&edit_prompt(&context.request, &input), &attachments),
            ];
            let edited = if use_review_draft || is_focused_correction {
                self.request_valid(client, messages, &request, log).await?
            } else {
                self.request_edit(
                    client,
                    EditExecution {
                        original: &draft,
                        messages,
                        input: &input,
                        indexes: &indexes,
                        is_fresh,
                        request: &request,
                    },
                    log,
                )
                .await?
            };
            plan.check_structure(&design, scope_baseline, &edited)
                .map_err(GenerationStop::Failed)?;
            if let Some(retained) = &retained {
                require_repair_scope(&design, retained, &edited)?;
            }
            if edited != design {
                self.designs
                    .keep_review_draft(design_id, &edited, Some(&design))
                    .await
                    .map_err(|error| {
                        GenerationStop::Failed(format!("Cannot checkpoint the edit draft: {error}"))
                    })?;
            }
            if let Err(error) =
                design_model::markup::validate_demo_navigation(&edited, Some(&design))
            {
                if attempt == MAX_DEMO_CORRECTIONS || !navigation_errors.insert(error.clone()) {
                    return Err(GenerationStop::Failed(format!(
                        "Cannot save demo edit: {error}"
                    )));
                }
                log(&format!(
                    "{label}: correcting navigation before layout review: {error}"
                ));
                correction = format!(
                    "Repair this navigation issue in the latest draft. Preserve other completed work. {error}"
                );
                draft = edited;
                attempt += 1;
                continue;
            }
            preview.show(&edited);
            // A fix can make a new problem. The touched screens are measured
            // again, and the model tweaks them until they measure clean or
            // the effort's rounds run out.
            let touched = if let Some(allowed) = &repair_indexes {
                allowed
                    .iter()
                    .copied()
                    .chain(scope_baseline.screens.len()..edited.screens.len())
                    .collect()
            } else if design.theme != edited.theme
                || design.viewport != edited.viewport
                || design.app_window != edited.app_window
            {
                (0..edited.screens.len()).collect()
            } else {
                touched_indexes(&design.screens, &edited.screens, &indexes)
            };
            let fix = EditFix {
                request: &context.request,
                context: &request,
                indexes: if is_focused_patch {
                    write_indexes.clone()
                } else {
                    touched.clone()
                },
            };
            let fallback = edited.clone();
            let polished = self.fix_edited_design(client, edited, &fix, log).await;
            let (final_design, audit) = match polished {
                Ok(result) => result,
                Err(error) => {
                    plan.check_structure(&design, scope_baseline, &fallback)
                        .map_err(GenerationStop::Failed)?;
                    let notice = self
                        .review_draft_notice(design_id, &error, false)
                        .await
                        .map_err(GenerationStop::Failed)?;
                    return Err(GenerationStop::Failed(notice));
                }
            };
            plan.check_structure(&design, scope_baseline, &final_design)
                .map_err(GenerationStop::Failed)?;
            if let Some(retained) = &retained {
                require_repair_scope(&design, retained, &final_design)?;
            }
            if final_design != design && final_design != fallback {
                self.designs
                    .keep_review_draft(design_id, &final_design, Some(&design))
                    .await
                    .map_err(|error| {
                        GenerationStop::Failed(format!(
                            "Cannot checkpoint the reviewed edit: {error}"
                        ))
                    })?;
            }
            if let Err(error) = audit.require_layout_clean_on(Some(&touched)) {
                let notice = self
                    .review_draft_notice(design_id, &error, false)
                    .await
                    .map_err(GenerationStop::Failed)?;
                return Err(GenerationStop::Failed(notice));
            }
            let layout_warnings = audit.instructions(Some(&touched));
            log(&format!(
                "{}: reviewing requested feature coverage before saving",
                request.label
            ));
            let failures = self
                .verify_demo_edit(
                    client,
                    &plan,
                    DemoEditEvidence {
                        candidate_id: design_id,
                        before: &design,
                        after: &final_design,
                        attachments: &attachments,
                        browser_evidence: &audit.prototype_evidence(),
                        previous_failures: previous_failures
                            .as_ref()
                            .map_or(&[], |(_, failures)| failures.as_slice()),
                    },
                    context.effort(),
                )
                .await
                .map_err(GenerationStop::Failed)?;
            if failures.is_empty() {
                break (final_design, touched, layout_warnings);
            }
            let improved = previous_failures
                .as_ref()
                .is_none_or(|(_, previous)| review_improved(previous, &failures));
            if !improved || attempt == MAX_DEMO_CORRECTIONS {
                let reason = if improved {
                    format!("the {MAX_DEMO_CORRECTIONS}-correction-round limit was reached")
                } else {
                    let defect_ids: Vec<_> = failures
                        .iter()
                        .map(|failure| format!("{}:{}", failure.requirement, failure.id))
                        .collect();
                    log(&format!(
                        "{label}: discarded correction round {attempt}; unresolved defect IDs: {}",
                        defect_ids.join(", ")
                    ));
                    "reviewed defects did not decrease without regression".to_owned()
                };
                let remaining = if !improved
                    && let Some((best, previous)) = &previous_failures
                    && best != &design
                {
                    self.designs
                        .keep_review_draft(design_id, best, Some(&design))
                        .await
                        .map_err(|error| {
                            GenerationStop::Failed(format!(
                                "Cannot restore the better review draft: {error}"
                            ))
                        })?;
                    previous.as_slice()
                } else {
                    failures.as_slice()
                };
                let details: Vec<_> = remaining
                    .iter()
                    .map(|failure| failure.detail.as_str())
                    .collect();
                return Err(GenerationStop::Failed(format!(
                    "Cannot save incomplete demo edit after {attempt} correction round(s): {reason}; remaining issues: {}",
                    details.join("; ")
                )));
            }
            correction_screens.clear();
            if failures.iter().all(|failure| !failure.screens.is_empty()) {
                correction_screens.extend(
                    failures
                        .iter()
                        .flat_map(|failure| failure.screens.iter().copied()),
                );
                correction_screens.sort_unstable();
                correction_screens.dedup();
            }
            if let Some(allowed) = &repair_indexes
                && let Some(index) = failures
                    .iter()
                    .flat_map(|failure| failure.screens.iter().copied())
                    .find(|&index| {
                        index < scope_baseline.screens.len() && !allowed.contains(&index)
                    })
            {
                return Err(GenerationStop::Failed(format!(
                    "Cannot finish draft repair: review requires a change to protected screen {} ({}). Edit this screen separately from the saved version.",
                    index + 1,
                    scope_baseline.screens[index].name
                )));
            }
            let details: Vec<_> = failures
                .iter()
                .map(|failure| failure.detail.as_str())
                .collect();
            let scope_note = if correction_screens.is_empty() {
                ""
            } else {
                " Replace only the specified screens. Do not change shared settings or insert or delete screens."
            };
            correction = format!(
                "Repair the supplied latest draft rather than starting over. Preserve completed work and satisfy the entire acceptance contract.{scope_note} The last review found {} unresolved defects:\n{}",
                failures.len(),
                details.join("\n")
            );
            draft = final_design;
            attempt += 1;
            log(&format!(
                "{label}: correction round {attempt}/{MAX_DEMO_CORRECTIONS}; {} unresolved defects",
                failures.len()
            ));
            previous_failures = Some((draft.clone(), failures));
        };
        let mut report = saved_edit_report(design_id, context.request.kind, &design, &final_design)
            .map_err(GenerationStop::Failed)?;
        let verified: Vec<String> = verified_indexes
            .iter()
            .map(|index| (index + 1).to_string())
            .collect();
        if !layout_warnings.is_empty() {
            report.push_str(&format!("\nNeeds review — advisory layout findings on the edited screens:\n{}\nNo blocking layout defects were measured; this is not a claim that all polish is verified.", layout_warnings.join("\n")));
        } else if !verified.is_empty() {
            report.push_str(&format!("\nRendered layout verified on screen(s) {}. Untouched screens are not claimed as verified.", verified.join(", ")));
        }
        self.designs
            .save_if_current(design_id, &final_design, Some(&design))
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        if let Err(error) = self.designs.clear_review_draft(design_id).await {
            tracing::warn!(%design_id, %error, "could not clear the accepted review draft");
        }
        self.notifier.notify();
        log(&format!("edit {design_id}: saved"));
        Ok(report)
    }
}
