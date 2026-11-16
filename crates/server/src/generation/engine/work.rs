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
    pub(crate) async fn say(&self, session_id: &str, content: &str) -> Result<(), String> {
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

    // Reuse a fresh generation's reserved id; fork edits and continuations so
    // polish never overwrites the user's saved version or erases safe work.
}
