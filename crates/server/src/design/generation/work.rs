use super::edit_requirements::DemoEditEvidence;
use super::*;

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
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for design_id in order.artifact_ids {
            match self
                .edit_design(client, context, design_id, order, log)
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
        let session_id = session_id_of_artifact(source_id);
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
                .keep_review_draft(source_id, design)
                .await
                .map_err(|error| error.to_string())?;
        }
        self.notifier.notify();
        let preservation = if is_new {
            "This is a draft, not a completed candidate."
        } else {
            "The saved version is unchanged. Open the same candidate to see the review draft."
        };
        let message = format!(
            "Needs review: preserved draft for `{source_id}`. {preservation} Layout and requirement completion are not verified.\n{issue}"
        );
        self.sessions
            .append_message(
                session_id,
                ChatMessage::assistant(&message).with_artifacts(vec![source_id.to_owned()]),
            )
            .await
            .map_err(|error| format!("{message}\nCannot record draft notice: {error}"))?;
        Ok(message)
    }

    pub(crate) async fn edit_design(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        design_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let design = self
            .designs
            .load(design_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("design `{design_id}` does not exist"))
            })?;
        let label = format!("edit {design_id}");
        log(&format!("{label}: resolving edit requirements and scope"));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let plan = self
            .plan_demo_edit(client, &design, order, (&attachments, context.effort()))
            .await
            .map_err(GenerationStop::Failed)?;
        let instruction = plan.instruction.as_str();
        // Structural edits need the full artifact in coordination. Workers remain
        // isolated; the requirements check protects unselected existing screens.
        let indexes = if plan.minimum_new_screens > 0 {
            Vec::new()
        } else {
            plan.screens.clone()
        };
        log(&format!("{label}: checking layout before editing"));
        let measured = self.audit_demo(&design).await?.instructions(None);
        let findings = findings_for(&measured, "screens", &indexes);
        let preview = self.designs.start_preview(
            design_id,
            &self.notifier,
            Some((design.clone(), plan.screens.clone())),
        );
        let mut draft = design.clone();
        let mut correction = String::new();
        let mut attempt = 0;
        let mut previous_missing = None;
        let mut last_navigation_error = None;
        let progress = self.shared_progress(&[design_id.to_owned()], 5, 95).pop();
        let (final_design, verified_indexes, layout_warnings) = loop {
            // Fresh regeneration drops old markup only once; corrections must
            // see and repair the latest draft, including newly added screens.
            let is_fresh = order.is_fresh && attempt == 0;
            let total = draft.screens.len();
            let (design_json, note) = if indexes.is_empty() {
                (serde_json::to_string(&draft), String::new())
            } else if is_fresh {
                (
                    focused_design_json(&draft, &indexes, true),
                    fresh_note("screen", "screens", &indexes, total),
                )
            } else {
                (
                    focused_design_json(&draft, &indexes, false),
                    focus_note("screen", "screens", &indexes, total),
                )
            };
            let design_json =
                design_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
            let correction_instruction = format!("{instruction}\n{correction}");
            let input = EditInput {
                instruction: &correction_instruction,
                artifact_json: &design_json,
                note: &note,
                findings: &findings,
                conversation: order.conversation,
            };
            let original = draft.clone();
            let request = ArtifactRequest {
                effort: context.effort().to_owned(),
                label: if attempt == 0 {
                    label.clone()
                } else {
                    format!("{label}: correction round {attempt}/3")
                },
                parse: Box::new(move |content| apply_patch(&original, parse_patch(content)?)),
                progress: progress.clone(),
                live: Some(preview.edit_sink(&draft)),
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                self.user_message(&edit_prompt(&context.request, &input), &attachments),
            ];
            let edited = self
                .request_edit(
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
                .await?;
            plan.check_structure(&design, &edited)
                .map_err(GenerationStop::Failed)?;
            if edited != design {
                self.designs
                    .keep_review_draft(design_id, &edited)
                    .await
                    .map_err(|error| {
                        GenerationStop::Failed(format!("Cannot checkpoint the edit draft: {error}"))
                    })?;
            }
            if let Err(error) =
                design_model::markup::validate_demo_navigation(&edited, Some(&design))
            {
                if attempt == 3 || last_navigation_error.as_ref() == Some(&error) {
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
                last_navigation_error = Some(error);
                draft = edited;
                attempt += 1;
                continue;
            }
            preview.show(&edited);
            // A fix can make a new problem. The touched screens are measured
            // again, and the model tweaks them until they measure clean or
            // the effort's rounds run out.
            let touched = if design.theme != edited.theme
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
                indexes: touched,
            };
            let fallback = edited.clone();
            let polished = self.fix_edited_design(client, edited, &fix, log).await;
            let (final_design, audit) = match polished {
                Ok(result) => result,
                Err(error) => {
                    plan.check_structure(&design, &fallback)
                        .map_err(GenerationStop::Failed)?;
                    let notice = self
                        .save_review_draft(design_id, &fallback, &error, false)
                        .await
                        .map_err(GenerationStop::Failed)?;
                    return Err(GenerationStop::Failed(notice));
                }
            };
            plan.check_structure(&design, &final_design)
                .map_err(GenerationStop::Failed)?;
            if final_design != design && final_design != fallback {
                self.designs
                    .keep_review_draft(design_id, &final_design)
                    .await
                    .map_err(|error| {
                        GenerationStop::Failed(format!(
                            "Cannot checkpoint the reviewed edit: {error}"
                        ))
                    })?;
            }
            if let Err(error) = audit.require_layout_clean_on(Some(&fix.indexes)) {
                let notice = self
                    .save_review_draft(design_id, &final_design, &error, false)
                    .await
                    .map_err(GenerationStop::Failed)?;
                return Err(GenerationStop::Failed(notice));
            }
            let layout_warnings = audit.instructions(Some(&fix.indexes));
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
                    },
                    context.effort(),
                )
                .await
                .map_err(GenerationStop::Failed)?;
            if failures.is_empty() {
                break (final_design, fix.indexes, layout_warnings);
            }
            let stop_reason = if attempt == 3 {
                Some("the three-correction-round limit was reached")
            } else if previous_missing.is_some_and(|count| failures.len() >= count) {
                Some("requirement coverage did not improve")
            } else {
                None
            };
            if let Some(reason) = stop_reason {
                return Err(GenerationStop::Failed(format!(
                    "Cannot save incomplete demo edit after {attempt} correction round(s): {reason}; remaining issues: {}",
                    failures.join("; ")
                )));
            }
            previous_missing = Some(failures.len());
            correction = format!(
                "Repair the supplied latest draft rather than starting over. Preserve completed work and satisfy the entire acceptance contract. The last review found {} incomplete checklist items:\n{}",
                failures.len(),
                failures.join("\n")
            );
            draft = final_design;
            attempt += 1;
            log(&format!(
                "{}: correction round {attempt}/3; {} incomplete checklist items",
                request.label,
                failures.len()
            ));
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
            .save(design_id, &final_design)
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
