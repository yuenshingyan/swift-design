use super::*;

impl GenerationEngine {
    /// Runs the chosen task and returns the outcome.
    pub(super) async fn execute(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        if context.request.kind == design_model::ArtifactKind::Deck {
            return self.execute_deck(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Document {
            return self.execute_document(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Social {
            return self.execute_social(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Print {
            return self.execute_print(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Mailing {
            return self.execute_mailing(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Campaign {
            return self.execute_campaign(client, context, task, log).await;
        }
        if context.request.kind == design_model::ArtifactKind::Artwork {
            return self.execute_artwork(client, context, task, log).await;
        }
        match task {
            GenerationTask::Candidates => self.generate_candidates(client, context, log).await,
            GenerationTask::Edit {
                designs,
                instruction,
                conversation,
            } => {
                let order = EditOrder {
                    artifact_ids: &designs,
                    instruction: &instruction,
                    is_fresh: false,
                    conversation: &conversation,
                };
                let edits = self.edit_designs(client, context, &order, log).await?;
                Ok(GenerationOutcome::edited(edits))
            }
            GenerationTask::Regenerate {
                design,
                instruction,
                conversation,
            } => {
                let order = EditOrder {
                    artifact_ids: std::slice::from_ref(&design),
                    instruction: &instruction,
                    is_fresh: true,
                    conversation: &conversation,
                };
                let edits = self.edit_designs(client, context, &order, log).await?;
                Ok(GenerationOutcome::edited(edits))
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let design_id = self
                    .merge_designs(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![design_id],
                })
            }
            GenerationTask::Continue(design_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, design_ids, log)
                    .await;
                if outcomes.iter().all(|(_, outcome)| outcome.is_err()) {
                    let failures: Vec<String> = outcomes
                        .iter()
                        .filter_map(|(id, outcome)| {
                            outcome.as_ref().err().map(|error| format!("{id}: {error}"))
                        })
                        .collect();
                    return Err(GenerationStop::Failed(failure_message(
                        &failures,
                        "no design was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Records the run outcome and updates the session. A written design
    /// is reported and the run finishes; a clarification writes the set
    /// and returns to clarifying; a failure is recorded and propagated.
    pub(super) async fn settle(
        &self,
        session_id: &str,
        run_id: &str,
        outcome: Result<GenerationOutcome, GenerationStop>,
        log: &LogSink,
    ) -> Result<GenerationOutcome, String> {
        match outcome {
            // The engine returns clarification as an Err below; an Ok
            // clarification cannot arise, but the type allows it.
            Ok(GenerationOutcome::NeedsClarification { question_set }) => {
                Ok(GenerationOutcome::NeedsClarification { question_set })
            }
            Ok(GenerationOutcome::Replied) => {
                self.finish_run(session_id, run_id, "replied", None, Vec::new())
                    .await;
                Ok(GenerationOutcome::Replied)
            }
            Ok(GenerationOutcome::Wrote { design_ids }) => {
                let summary = wrote_summary(&design_ids);
                self.complete_artifacts(
                    session_id,
                    run_id,
                    ChatMessage::assistant(&summary).with_artifacts(design_ids.clone()),
                )
                .await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            Ok(GenerationOutcome::Edited {
                design_ids,
                reports,
            }) => {
                let summary = format!("{EDIT_REPORT_PREFIX}{}", reports.join("\n\n"));
                let mut message =
                    ChatMessage::assistant(&summary).with_artifacts(design_ids.clone());
                message.snapshot_stamps = Some(self.edit_snapshots(session_id, &design_ids).await);
                self.complete_artifacts(session_id, run_id, message).await?;
                Ok(GenerationOutcome::Edited {
                    design_ids,
                    reports,
                })
            }
            Err(GenerationStop::NeedsClarification(set)) => {
                let number = self
                    .sessions
                    .write_question_set(session_id, &set)
                    .await
                    .map_err(|error| error.to_string())?;
                self.sessions
                    .append_message(
                        session_id,
                        ChatMessage::assistant_questions(&set.message, number),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                self.sessions
                    .apply(session_id, design_model::WorkflowEvent::QuestionsAsked)
                    .await
                    .map_err(|error| error.to_string())?;
                self.finish_run(session_id, run_id, "needs_clarification", None, Vec::new())
                    .await;
                log("generation needs clarification; returned to the questions");
                Ok(GenerationOutcome::NeedsClarification {
                    question_set: number,
                })
            }
            Err(GenerationStop::Failed(message)) => {
                self.finish_run(
                    session_id,
                    run_id,
                    "failed",
                    Some(message.clone()),
                    Vec::new(),
                )
                .await;
                Err(message)
            }
        }
    }

    /// Captures exact edit baselines without using a queued prompt's arrival time.
    pub(super) async fn edit_snapshots(
        &self,
        session_id: &str,
        ids: &[String],
    ) -> HashMap<String, String> {
        let mut snapshots = HashMap::new();
        let Ok(Some(session)) = self.sessions.read(session_id).await else {
            return snapshots;
        };
        for id in ids {
            if let Some(history) = self.artifact_history(session.artifact_kind, id).await
                && let Some(snapshot) = history.first()
            {
                snapshots.insert(id.clone(), snapshot.stamp.clone());
            }
        }
        snapshots
    }

    pub(super) async fn artifact_history(
        &self,
        kind: ArtifactKind,
        id: &str,
    ) -> Option<Vec<Snapshot>> {
        match kind {
            ArtifactKind::Demo => self.designs.history(id).await.ok(),
            ArtifactKind::Deck => self.decks.as_ref()?.history(id).await.ok(),
            ArtifactKind::Document => self.documents.as_ref()?.history(id).await.ok(),
            ArtifactKind::Social => self.socials.as_ref()?.history(id).await.ok(),
            ArtifactKind::Print => self.prints.as_ref()?.history(id).await.ok(),
            ArtifactKind::Mailing => self.mailings.as_ref()?.history(id).await.ok(),
            ArtifactKind::Campaign => self.campaigns.as_ref()?.history(id).await.ok(),
            ArtifactKind::Artwork => self.artworks.as_ref()?.history(id).await.ok(),
        }
    }

    /// Persists a completion report with the artifact ids used for reverting the turn.
    pub(super) async fn complete_artifacts(
        &self,
        session_id: &str,
        run_id: &str,
        message: ChatMessage,
    ) -> Result<(), String> {
        let artifacts = message.artifacts.clone();
        self.sessions
            .append_message(session_id, message)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        self.finish_run(session_id, run_id, "succeeded", None, artifacts)
            .await;
        Ok(())
    }

    /// Appends an assistant turn and wakes the studio.
    pub(super) async fn say(&self, session_id: &str, content: &str) -> Result<(), String> {
        self.sessions
            .append_message(session_id, ChatMessage::assistant(content))
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }

    /// Records the end of the run and wakes the studio.
    pub(super) async fn finish_run(
        &self,
        session_id: &str,
        run_id: &str,
        result: &str,
        error: Option<String>,
        artifacts: Vec<String>,
    ) {
        if let Err(failure) = self
            .sessions
            .finish_run(session_id, run_id, result, error, artifacts)
            .await
        {
            tracing::warn!(%failure, "recording the generation run failed");
        }
        self.notifier.notify();
    }

    /// Writes one design per requested variation. Returns the ids.
    pub(super) async fn generate_candidates(
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
                Ok::<(), GenerationStop>(())
            });
        }
        let mut saved = Vec::new();
        let mut failures = Vec::new();
        while let Some(outcome) = tasks.join_next().await {
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(GenerationStop::NeedsClarification(set))) => {
                    tasks.shutdown().await;
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Ok(Err(GenerationStop::Failed(error))) => failures.push(error),
                Err(error) => failures.push(format!("candidate task failed: {error}")),
            }
        }
        // Every candidate saves itself, so the written ids are those on
        // disk under this base.
        for id in &ids {
            if matches!(self.designs.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no candidate reached the store",
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
    pub(super) async fn merge_designs(
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

    /// Asks the model for `count` distinct concepts in one call. A reply
    /// that does not parse yields no concepts, and the candidates are
    /// written without them.
    pub(crate) async fn plan_concepts(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        count: usize,
        attachments: &Attachments,
        log: &LogSink,
    ) -> Result<Vec<Concept>, GenerationStop> {
        log(&format!("planning {count} concepts"));
        let messages = vec![
            serde_json::json!({ "role": "system", "content": concept_prompt(count) }),
            self.user_message(&concept_input(&context.request), attachments),
        ];
        let started = std::time::Instant::now();
        let reply = self.model.chat(client, &messages, context.effort()).await?;
        log(&format!(
            "concepts: reply in {:.0}s",
            started.elapsed().as_secs_f32()
        ));
        let concepts = parse_concepts(&reply);
        if concepts.is_empty() {
            log("concept reply did not parse; writing candidates without concepts");
        } else {
            let names: Vec<&str> = concepts
                .iter()
                .map(|concept| concept.name.as_str())
                .collect();
            log(&format!("concepts: {}", names.join(" · ")));
        }
        Ok(concepts)
    }

    /// Applies a critique to one chosen design: the model rewrites the
    /// design against the brief and the critique, the result is
    /// validated, polished, and saved under the same id.
    /// Applies `instruction` to each design in turn and returns the ones
    /// it saved. One failure is logged and the rest still run; the turn
    /// fails only when every edit failed.
    pub(super) async fn edit_designs(
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

    pub(super) async fn edit_design(
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
        let plan = self
            .plan_demo_edit(client, &design, order, context.effort())
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
        let total = design.screens.len();
        let (design_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&design), String::new())
        } else if order.is_fresh {
            (
                focused_design_json(&design, &indexes, true),
                fresh_note("screen", "screens", &indexes, total),
            )
        } else {
            (
                focused_design_json(&design, &indexes, false),
                focus_note("screen", "screens", &indexes, total),
            )
        };
        let design_json = design_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &design_json,
            note: &note,
            findings: &findings,
            conversation: order.conversation,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(&edit_prompt(&context.request, &input), &attachments),
        ];
        let original = design.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| apply_patch(&original, parse_patch(content)?)),
            progress: self.shared_progress(&[design_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_edit(
                client,
                EditExecution {
                    original: &design,
                    messages,
                    input: &input,
                    indexes: &indexes,
                    is_fresh: order.is_fresh,
                    request: &request,
                },
                log,
            )
            .await?;
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
        let final_design = self
            .fix_edited_design(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        plan.check_structure(&design, &final_design)
            .map_err(GenerationStop::Failed)?;
        log(&format!(
            "{}: reviewing requested feature coverage before saving",
            request.label
        ));
        self.verify_demo_edit(client, &plan, &final_design, context.effort())
            .await
            .map_err(GenerationStop::Failed)?;
        let mut report = saved_edit_report(design_id, context.request.kind, &design, &final_design)
            .map_err(GenerationStop::Failed)?;
        let verified: Vec<String> = fix
            .indexes
            .iter()
            .map(|index| (index + 1).to_string())
            .collect();
        if !verified.is_empty() {
            report.push_str(&format!("\nRendered layout verified on screen(s) {}. Untouched screens are not claimed as verified.", verified.join(", ")));
        }
        self.designs
            .save(design_id, &final_design)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {design_id}: saved"));
        Ok(report)
    }
}
