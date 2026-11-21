use crate::design::annotations::AnnotationBatch;
use crate::generation::UploadAttachment;
use crate::screenshots::screenshot_marked_screen;

use super::edit_requirements::DemoEditRequirements;
use super::*;

const MAX_DEMO_CORRECTIONS: usize = 10;

fn desktop_repair_baseline(saved: &Design, retained: &Design) -> Design {
    let mut desktop = saved.clone();
    desktop.screens.clear();
    let mut positions = vec![usize::MAX; retained.screens.len()];
    for (index, screen) in retained.screens.iter().enumerate() {
        let accepted = saved.screens.get(index);
        if let Some(accepted) = accepted
            && (accepted.is_mobile(saved.viewport) || screen.is_mobile(retained.viewport))
        {
            positions[index] = desktop.screens.len();
            desktop.screens.push(accepted.clone());
        } else if accepted.is_some() || !screen.is_mobile(retained.viewport) {
            positions[index] = desktop.screens.len();
            desktop.screens.push(screen.clone());
        }
    }
    let missing_destination = desktop.screens.len().max(desktop.outline.len());
    // The remapper also accepts existing outline positions as destinations.
    positions.extend(positions.len()..=missing_destination);
    for position in &mut positions {
        if *position == usize::MAX {
            // Do not silently retarget a link to the next desktop screen.
            *position = missing_destination;
        }
    }
    if desktop.screens.len() != retained.screens.len() {
        for (index, screen) in desktop.screens.iter_mut().enumerate() {
            if saved.screens.get(index) != Some(screen) {
                screen.html = design_model::markup::remap_screen_links(&screen.html, &positions);
            }
        }
    }
    desktop
}

fn require_repair_scope(
    before: &Design,
    retained: &Design,
    edited: &Design,
) -> Result<(), GenerationStop> {
    let problem = if edited.title != retained.title {
        Some("the title changed".to_owned())
    } else if edited.theme != retained.theme {
        Some("the theme changed".to_owned())
    } else if edited.viewport != retained.viewport {
        Some("the canvas size changed".to_owned())
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
        edit: (&EditOrder<'_>, bool, Option<&AnnotationBatch>),
        log: &LogSink,
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let (order, _, _) = edit;
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
            "The current snapshot is unchanged. The incomplete result was kept separately."
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
        edit: (&EditOrder<'_>, bool, Option<&AnnotationBatch>),
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let (order, use_review_draft, annotations) = edit;
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
        let is_desktop_repair = use_review_draft
            && order.instruction.lines().next().is_some_and(|line| {
                line.trim()
                    .eq_ignore_ascii_case("desktop-only draft repair.")
            });
        if is_desktop_repair {
            if draft.screens.len() < design.screens.len() {
                return Err(GenerationStop::Failed(
                    "Cannot repair desktop only: the retained draft removed saved screens. Start from the current snapshot.".into()
                ));
            }
            draft = desktop_repair_baseline(&design, &draft);
        }
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
        let (attachments, plan) = if let Some(batch) = annotations {
            batch
                .check_revision(&design)
                .map_err(GenerationStop::Failed)?;
            if !design.validate().is_empty() || !supports_vision(self.model.model()) {
                return Err(GenerationStop::Failed(
                    "Annotation edits need a valid demo and a model that accepts images.".into(),
                ));
            }
            let mut attachments = Attachments::default();
            for index in batch.indexes() {
                log(&format!("{label}: capturing marked screen {}", index + 1));
                let bytes =
                    screenshot_marked_screen(&design, index, &self.base_url(), &batch.marks)
                        .await
                        .map_err(|error| {
                            GenerationStop::Failed(format!(
                                "Cannot capture marked screen {}: {error}",
                                index + 1
                            ))
                        })?;
                attachments.files.push(UploadAttachment {
                    name: format!("annotation-screen-{}.png", index + 1),
                    content_type: "image/png".into(),
                    bytes,
                });
            }
            (attachments, DemoEditRequirements::from_annotations(batch))
        } else {
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
            (attachments, plan)
        };
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
        let measured = match self.audit_demo(&draft).await {
            Ok(audit) => audit.instructions(None),
            Err(error) => {
                log(&format!(
                    "{label}: could not measure the starting layout: {error}"
                ));
                Vec::new()
            }
        };
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
        let mut best_blockers: Option<Vec<(usize, String)>> = None;
        let mut best_has_navigation_errors = false;
        let mut next_blocker = 0;
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
            let desktop_note = if is_desktop_repair {
                "Desktop-only repair: keep existing mobile screens unchanged. Omit all new mobile specimens. Repair desktop links that pointed to excluded screens."
            } else {
                ""
            };
            let correction_instruction = format!("{instruction}\n{desktop_note}\n{correction}");
            let is_annotated = annotations.is_some();
            let is_focused_patch = is_annotated
                || is_focused_correction
                || (use_review_draft && !write_indexes.is_empty());
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
                    let edited = apply_patch(&original, patch)?;
                    if is_desktop_repair
                        && edited.screens.iter().enumerate().any(|(index, screen)| {
                            screen.is_mobile(edited.viewport)
                                && original.screens.get(index) != Some(screen)
                        })
                    {
                        return Err("Desktop-only repair cannot add a mobile screen or change an existing one.".into());
                    }
                    if is_annotated
                        && repair_screens.iter().any(|&index| {
                            let before = &original.screens[index];
                            let after = &edited.screens[index];
                            before.name != after.name
                                || !design_model::markup::preserves_demo_links(
                                    &before.html,
                                    &after.html,
                                )
                        })
                    {
                        return Err("Annotation edits must preserve screen names, link destinations, and prototype actions.".into());
                    }
                    Ok(edited)
                }),
                progress: progress.clone(),
                live: (!is_focused_patch && !use_review_draft).then(|| preview.edit_sink(&draft)),
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                self.user_message(&edit_prompt(&context.request, &input), &attachments),
            ];
            let edited = if is_focused_patch || use_review_draft || attempt > 0 {
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
            if let Err(error) = plan.check_scope(scope_baseline, &edited) {
                if attempt == MAX_DEMO_CORRECTIONS {
                    return Err(GenerationStop::Failed(error));
                }
                correction.push_str(&format!(
                    "\nThe rejected patch exceeded the permitted scope. {error}"
                ));
                attempt += 1;
                continue;
            }
            if let Some(retained) = &retained {
                require_repair_scope(&design, retained, &edited)?;
            }
            // Do not replace a better recovery checkpoint before checking a correction.
            if attempt == 0 && edited != design {
                self.designs
                    .keep_review_draft(design_id, &edited, Some(&design))
                    .await
                    .map_err(|error| {
                        GenerationStop::Failed(format!("Cannot checkpoint the edit draft: {error}"))
                    })?;
            }
            let added = edited.screens.len().saturating_sub(design.screens.len());
            if added < plan.minimum_new_screens {
                if attempt == 0 || edited.screens.len() > draft.screens.len() {
                    if attempt > 0 {
                        self.designs
                            .keep_review_draft(design_id, &edited, Some(&design))
                            .await
                            .map_err(|error| {
                                GenerationStop::Failed(format!(
                                    "Cannot checkpoint the edit draft: {error}"
                                ))
                            })?;
                    }
                    draft = edited;
                }
                let remaining = plan
                    .minimum_new_screens
                    .saturating_sub(draft.screens.len().saturating_sub(design.screens.len()));
                let issue =
                    format!("The edit still needs {remaining} required separate screen(s).");
                if attempt == MAX_DEMO_CORRECTIONS {
                    let error = format!(
                        "Reached the {MAX_DEMO_CORRECTIONS}-correction-round limit. {issue}"
                    );
                    let notice = self
                        .review_draft_notice(design_id, &error, false)
                        .await
                        .map_err(GenerationStop::Failed)?;
                    return Err(GenerationStop::Failed(notice));
                }
                correction = format!(
                    "{issue} Append the missing pages to the best draft while preserving existing work."
                );
                attempt += 1;
                log(&format!(
                    "{label}: correction round {attempt}/{MAX_DEMO_CORRECTIONS}; {issue}"
                ));
                continue;
            }
            // Recheck all affected screens so a correction cannot trade one blocker for another.
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
            let mut blockers: Vec<_> =
                design_model::markup::demo_navigation_issues(&edited, Some(&design)).collect();
            let has_navigation_errors = !blockers.is_empty();
            // Check every scope-valid candidate with the same probes. Subjective
            // findings neither undo a patch nor trigger another model call.
            let mut layout_warnings = Vec::new();
            if blockers.is_empty() {
                log(&format!(
                    "{}: checking layout and interactions on edited screens",
                    request.label
                ));
                let audit = match self.audit_demo(&edited).await {
                    Ok(audit) => audit,
                    Err(error) => {
                        let notice = self
                            .review_draft_notice(design_id, &error, false)
                            .await
                            .map_err(GenerationStop::Failed)?;
                        return Err(GenerationStop::Failed(notice));
                    }
                };
                let evidence = audit.prototype_evidence();
                if !evidence.is_empty() {
                    log(&format!("{label}: browser interaction checks:\n{evidence}"));
                }
                blockers.extend(audit.usable_edit_issues(&touched));
                layout_warnings = audit.edit_warnings(&edited, &touched);
            }
            if blockers.is_empty() {
                preview.show(&edited);
                request.report(1.0);
                break (edited, touched, layout_warnings);
            }
            let improved = best_blockers.as_ref().is_none_or(|best| {
                if has_navigation_errors != best_has_navigation_errors {
                    !has_navigation_errors
                } else {
                    blockers.len() < best.len() && blockers.iter().all(|issue| best.contains(issue))
                }
            });
            let rejected = if improved {
                if edited != design && attempt > 0 {
                    self.designs
                        .keep_review_draft(design_id, &edited, Some(&design))
                        .await
                        .map_err(|error| {
                            GenerationStop::Failed(format!(
                                "Cannot checkpoint the improved edit: {error}"
                            ))
                        })?;
                }
                if !has_navigation_errors {
                    preview.show(&edited);
                }
                best_blockers = Some(blockers);
                best_has_navigation_errors = has_navigation_errors;
                next_blocker = 0;
                draft = edited;
                String::new()
            } else {
                next_blocker += 1;
                format!(
                    " The last patch was discarded. Take a different approach; it left these blockers: {}.",
                    blockers
                        .iter()
                        .map(|(_, detail)| detail.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            };
            let remaining = best_blockers.as_deref().unwrap_or_default();
            if attempt == MAX_DEMO_CORRECTIONS {
                let details = remaining
                    .iter()
                    .map(|(_, detail)| detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                let error = format!(
                    "Reached the {MAX_DEMO_CORRECTIONS}-correction-round limit. Remaining blockers: {details}"
                );
                let notice = self
                    .review_draft_notice(design_id, &error, false)
                    .await
                    .map_err(GenerationStop::Failed)?;
                return Err(GenerationStop::Failed(notice));
            }
            let editable = |index: usize| {
                index < draft.screens.len()
                    && (plan.screens.is_empty()
                        || plan.screens.contains(&index)
                        || index >= scope_baseline.screens.len())
                    && repair_indexes.as_ref().is_none_or(|allowed| {
                        allowed.contains(&index) || index >= scope_baseline.screens.len()
                    })
            };
            // Try another affected screen after a stall, starting from the best draft.
            let mut repairable: Vec<_> = remaining
                .iter()
                .map(|(index, _)| *index)
                .filter(|&index| editable(index))
                .collect();
            repairable.sort_unstable();
            repairable.dedup();
            let Some(&index) = repairable.get(next_blocker % repairable.len().max(1)) else {
                let details = remaining
                    .iter()
                    .map(|(_, detail)| detail.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                let error =
                    format!("Cannot fix a blocker outside the permitted screen scope: {details}");
                let notice = self
                    .review_draft_notice(design_id, &error, false)
                    .await
                    .map_err(GenerationStop::Failed)?;
                return Err(GenerationStop::Failed(notice));
            };
            let details = remaining
                .iter()
                .filter(|(affected, _)| *affected == index)
                .map(|(_, detail)| detail.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            correction_screens = vec![index];
            correction = format!(
                "Repair only these verified issues in the supplied best draft. Replace only the specified screen. Preserve completed work. {details}{rejected}"
            );
            attempt += 1;
            log(&format!(
                "{label}: correction round {attempt}/{MAX_DEMO_CORRECTIONS}; repairing screen {}: {details}",
                index + 1
            ));
        };
        let mut report = saved_edit_report(design_id, context.request.kind, &design, &final_design)
            .map_err(GenerationStop::Failed)?;
        let verified: Vec<String> = verified_indexes
            .iter()
            .map(|index| (index + 1).to_string())
            .collect();
        if !layout_warnings.is_empty() {
            report.push_str(&format!(
                "\nLayout warnings on the edited screens (the preview may look fine):\n{}",
                layout_warnings.join("\n")
            ));
        } else if !verified.is_empty() {
            report.push_str(&format!("\nRendered layout verified on screen(s) {}. Untouched screens are not claimed as verified.", verified.join(", ")));
        }
        self.designs
            .save_if_current(design_id, &final_design, Some(&design))
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        report.push_str("\nThe browser and structural checks passed for the saved edit. Requirements not covered by those checks were not independently verified.");
        if let Err(error) = self.designs.clear_review_draft(design_id).await {
            tracing::warn!(%design_id, %error, "could not clear the accepted review draft");
        }
        self.notifier.notify();
        log(&format!("edit {design_id}: saved"));
        Ok(report)
    }
}
