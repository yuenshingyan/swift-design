use super::*;

impl GenerationEngine {
    /// The mailing store, or the failure a mailing run reports
    /// without one.
    fn mailing_store(&self) -> Result<&MailingStore, GenerationStop> {
        self.mailings.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no mailing store: mailing sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview mailings the latest user turn asked to continue:
    /// every mailing named by a trailing continue request that still
    /// is a preview.
    pub(crate) async fn continue_mailing_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(mailings) = &self.mailings else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for mailing_id in trailing_continue_ids(&messages) {
            // A mailing that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(mailing)) = mailings.load(&mailing_id).await
                && mailing.is_preview()
            {
                previews.push(mailing_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for a mailing session and returns the
    /// outcome.
    pub(crate) async fn execute_mailing(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => {
                self.generate_mailing_candidates(client, context, log).await
            }
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
                let edits = self.edit_mailings(client, context, &order, log).await?;
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
                let edits = self.edit_mailings(client, context, &order, log).await?;
                Ok(GenerationOutcome::edited(edits))
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let mailing_id = self
                    .merge_mailings(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![mailing_id],
                })
            }
            GenerationTask::Continue(mailing_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, mailing_ids, log)
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
                        "no mailing was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one mailing per requested variation. Returns the ids.
    async fn generate_mailing_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let mailings = self.mailing_store()?;
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
        // A later run numbers after the candidates the session has, so
        // it adds to them instead of overwriting them.
        let first_number = match mailings.list().await {
            Ok(rows) => next_candidate_number(&base, rows.iter().map(|row| row.id.as_str())),
            Err(_) => 1,
        };
        let ids: Vec<String> = (0..count)
            .map(|offset| candidate_id(&base, first_number + offset))
            .collect();
        let shares = self.shared_progress(&ids, 10, 90);
        let templates = self.brief_templates(&context.options, log).await;
        // Every candidate runs at the same time; each saves itself as
        // soon as it is ready.
        let mut tasks = tokio::task::JoinSet::new();
        for candidate_number in 1..=count {
            let engine = self.clone();
            let client = client.clone();
            let context = context.clone();
            let concepts = concepts.clone();
            let template = candidate_template(&templates, candidate_number);
            let attachments = Arc::clone(&attachments);
            let share = Arc::clone(&shares[candidate_number - 1]);
            let log = Arc::clone(log);
            let id = ids[candidate_number - 1].clone();
            share(0.0);
            tasks.spawn(async move {
                let request = MailingCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_emails: context.preview_screens(),
                    mailing_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_mailing_candidate(&client, &request, &attachments, &share, &log)
                    .await?;
                log(&format!("candidate {candidate_number}: saved as {id}"));
                Ok::<(), GenerationStop>(())
            });
        }
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
        let mut saved = Vec::new();
        for id in &ids {
            if matches!(mailings.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no mailing candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one mailing candidate, repairs it through
    /// fix rounds until it validates, and polishes it. The mailing is
    /// saved under `request.mailing_id` while it streams in, when the
    /// draft validates, and once more after the polish.
    async fn generate_mailing_candidate(
        &self,
        client: &reqwest::Client,
        request: &MailingCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Mailing, GenerationStop> {
        let mailings = self.mailing_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": mailing_system_prompt() }),
            self.user_message(&mailing_candidate_prompt(request), attachments),
        ];
        let saver = MailingLiveSaver::new(mailings, &self.notifier, &request.mailing_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_mailing),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(mailing) = partial_mailing(text) {
                    let rank = mailing.emails.len();
                    live_saver.offer(mailing, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.emails.len());
        let polished = self
            .polish_mailing(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        mailings
            .clear_user_paths(&request.mailing_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new mailing candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_mailings(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let mailings = self.mailing_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let mailing = mailings
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("mailing `{id}` does not exist")))?;
            loaded.push((id.as_str(), mailing));
        }
        if loaded.is_empty() {
            return Err(GenerationStop::Failed(
                "a merge needs two candidates: pin them with @".to_owned(),
            ));
        }
        let merge = MergeInput {
            sources: merge_sources(&loaded).map_err(GenerationStop::Failed)?,
            instruction: instruction.to_owned(),
        };
        let base = context.session_id.as_str();
        let rows = mailings
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let mailing_id = candidate_id(base, number);
        log(&format!("merging {} into {mailing_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&mailing_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = MailingCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_emails: None,
            mailing_id: mailing_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_mailing_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {mailing_id}"));
        Ok(mailing_id)
    }

    /// Applies `instruction` to each mailing in turn and returns the
    /// ones it saved. One failure is logged and the rest still run; the
    /// turn fails only when every edit failed.
    async fn edit_mailings(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for mailing_id in order.artifact_ids {
            match self
                .edit_mailing(client, context, mailing_id, order, log)
                .await
            {
                Ok(report) => saved.push((mailing_id.clone(), report)),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {mailing_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_mailing(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        mailing_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let mailings = self.mailing_store()?;
        let mailing = mailings
            .load(mailing_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("mailing `{mailing_id}` does not exist"))
            })?;
        let instruction = order.instruction;
        let label = format!("edit {mailing_id}");
        // A change that names emails is about those emails: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named emails without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "email")
            .into_iter()
            .filter(|index| *index < mailing.emails.len())
            .collect();
        let measured = dom_findings(&mailing, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "emails", &indexes);
        let total = mailing.emails.len();
        let (mailing_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&mailing), String::new())
        } else if order.is_fresh {
            (
                focused_mailing_json(&mailing, &indexes, true),
                fresh_note("email", "emails", &indexes, total),
            )
        } else {
            (
                focused_mailing_json(&mailing, &indexes, false),
                focus_note("email", "emails", &indexes, total),
            )
        };
        let mailing_json =
            mailing_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &mailing_json,
            note: &note,
            findings: &findings,
            conversation: order.conversation,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": mailing_system_prompt() }),
            self.user_message(&mailing_edit_prompt(&context.request, &input), &attachments),
        ];
        let original = mailing.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| apply_patch(&original, parse_patch(content)?)),
            progress: self.shared_progress(&[mailing_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_edit(
                client,
                EditExecution {
                    original: &mailing,
                    messages,
                    input: &input,
                    indexes: &indexes,
                    is_fresh: order.is_fresh,
                    request: &request,
                },
                log,
            )
            .await?;
        // A fix can make a new problem. The touched emails are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&mailing.emails, &edited.emails, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_mailing = self
            .fix_edited_mailing(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        let report = saved_edit_report(mailing_id, context.request.kind, &mailing, &final_mailing)
            .map_err(GenerationStop::Failed)?;
        mailings
            .save(mailing_id, &final_mailing)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {mailing_id}: saved"));
        Ok(report)
    }

    /// Writes the remaining emails of the preview mailing `mailing_id`
    /// in chunks. The mailing is saved after every chunk, so the canvas
    /// shows it grow, then polished once it is complete. Returns how
    /// many emails were added; 0 when the mailing is complete already.
    pub(crate) async fn continue_mailing(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        mailing_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let mailings = self.mailing_store().map_err(stop_to_string)?;
        let mut mailing = mailings
            .load(mailing_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("mailing `{mailing_id}` does not exist"))?;
        // A run that stopped may have left placeholder emails behind.
        mailing.emails.retain(|email| !is_pending_email(email));
        if !mailing.is_preview() {
            log(&format!(
                "continue {mailing_id}: the mailing is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {mailing_id}");
        let start = mailing.emails.len();
        let planned = mailing.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} emails written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = MailingLiveSaver::new(mailings, &self.notifier, mailing_id);
        let board = self
            .write_mailing_chunks(
                client,
                context,
                &mailing,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = mailing.clone();
        if let Ok(board) = board.lock() {
            for emails in board.iter() {
                continued.emails.extend(emails.iter().cloned());
            }
        }
        let added = continued.emails.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the mailing stays continuable.
            if let Err(error) = saver.finish(&mailing).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added an email"));
        }
        // A failed chunk leaves the mailing continuable: the outline
        // stays until every title has an email.
        if continued.emails.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_mailing),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_mailing = self
            .polish_mailing(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_mailing).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new emails"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_mailing_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Mailing,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &MailingLiveSaver,
        log: &LogSink,
    ) -> MailingChunkBoard {
        let start = preview.emails.len();
        let planned = preview.outline.len();
        let board: MailingChunkBoard = Arc::new(std::sync::Mutex::new(
            chunks.iter().map(|_| Vec::new()).collect(),
        ));
        let show: Arc<dyn Fn() + Send + Sync> = {
            let board = Arc::clone(&board);
            let saver = saver.clone();
            let preview = preview.clone();
            let progress = Arc::clone(progress);
            let chunks = chunks.to_vec();
            Arc::new(move || {
                let Ok(board) = board.lock() else {
                    return;
                };
                let written: usize = board.iter().map(Vec::len).sum();
                let done = written as f32 / (planned - start).max(1) as f32;
                progress(CONTINUE_DRAFT_SHARE * done);
                saver.offer(shown_mailing(&preview, &chunks, &board), written);
            })
        };
        let mut tasks = tokio::task::JoinSet::new();
        for (position, chunk) in chunks.iter().enumerate() {
            let engine = self.clone();
            let client = client.clone();
            let context = context.clone();
            let preview = preview.clone();
            let board = Arc::clone(&board);
            let show = Arc::clone(&show);
            let log = Arc::clone(log);
            let chunk = *chunk;
            let attachments = Arc::clone(attachments);
            tasks.spawn(async move {
                engine
                    .write_mailing_chunk(
                        &client,
                        &context,
                        &preview,
                        (position, chunk),
                        &attachments,
                        &board,
                        &show,
                        &log,
                    )
                    .await
            });
        }
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log(&format!("continue chunk failed: {error}")),
                Err(error) => log(&format!("chunk task failed: {error}")),
            }
        }
        board
    }

    /// Writes one continuation chunk into row `position` of the board,
    /// showing the mailing grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_mailing_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Mailing,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &MailingChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let mailing_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": mailing_system_prompt() }),
            self.user_message(
                &mailing_continue_prompt(&context.request, preview, &mailing_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.emails.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_mailing_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let emails = partial_continuation_emails(written, text);
                if let Ok(mut board) = live_board.lock()
                    && emails.len() > board[position].len()
                {
                    board[position] = emails;
                } else {
                    return;
                }
                live_show();
            })),
        };
        let continued = self
            .request_valid(client, messages, &request, log)
            .await
            .map_err(stop_to_string)?;
        let emails: Vec<Email> = continued.emails[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = emails;
        }
        show();
        Ok(())
    }

    /// Reviews a valid mailing as a mailing designer, one round per
    /// effort level. An improved mailing that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_mailing(
        &self,
        client: &reqwest::Client,
        mut mailing: Mailing,
        context: &ArtifactRequest<'_, Mailing>,
        log: &LogSink,
    ) -> Result<Mailing, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !can_audit() {
            log(&format!(
                "{label}: {}",
                PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(mailing);
        }
        let limit = polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = mailing.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings = dom_findings(&mailing, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = mailing.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a mailing that is already good.
            if findings.is_empty() {
                stop = PolishStop::Clean;
                break;
            }
            // The last round did not reduce the findings, so the next
            // will not either.
            if previous_count.is_some_and(|before| findings.len() >= before) {
                stop = PolishStop::NoImprovement;
                break;
            }
            previous_count = Some(findings.len());
            rounds_taken = round;
            let images = self.email_images(&mailing, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} email images)",
                findings.len(),
                images.len()
            ));
            let mailing_json =
                serde_json::to_string(&mailing).map_err(|error| error.to_string())?;
            let prompt = polish_prompt(&mailing_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": mailing_system_prompt() }),
                serde_json::json!({
                    "role": "user",
                    "content": user_content_with_images(&prompt, &images),
                }),
            ];
            let content = self
                .model
                .chat_with(
                    client,
                    self.model
                        .request_body(&messages, writing_effort(&context.effort)),
                    None,
                )
                .await?;
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&mailing, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => mailing = improved,
                Ok(_) => log(&format!(
                    "{label}: polished mailing failed validation; keeping the previous version"
                )),
                Err(parse_error) => log(&format!(
                    "{label}: polish reply unusable ({parse_error}); keeping the previous version"
                )),
            }
            context.report(DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32);
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_count)
        ));
        context.report(1.0);
        Ok(best)
    }

    /// Measures the touched emails of an edited mailing and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// emails measure clean, a round does not help, or the effort's round
    /// limit runs out. Returns the best version measured.
    async fn fix_edited_mailing(
        &self,
        client: &reqwest::Client,
        mut mailing: Mailing,
        fix: &EditFix<'_, Mailing>,
        log: &LogSink,
    ) -> Result<Mailing, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !can_audit() {
            fix.context.report(1.0);
            return Ok(mailing);
        }
        let limit = polish_round_limit(&fix.context.effort);
        let mut best = mailing.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured = dom_findings(&mailing, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "emails", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = mailing.clone();
            }
            if findings.is_empty() {
                stop = PolishStop::Clean;
                break;
            }
            if previous_count.is_some_and(|before| findings.len() >= before) {
                stop = PolishStop::NoImprovement;
                break;
            }
            previous_count = Some(findings.len());
            rounds_taken = round;
            log(&format!(
                "{label}: fix round {round} of at most {limit} ({} findings on the touched emails)",
                findings.len()
            ));
            let mailing_json = focused_mailing_json(&mailing, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("email", "emails", &fix.indexes, mailing.emails.len());
            let instruction = fix_instruction("emails");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &mailing_json,
                note: &note,
                findings: &findings,
                conversation: "",
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": mailing_system_prompt() }),
                serde_json::json!({ "role": "user", "content": mailing_edit_prompt(fix.request, &input) }),
            ];
            let reply = self
                .model
                .chat_with(
                    client,
                    self.model
                        .request_body(&messages, writing_effort(&fix.context.effort)),
                    None,
                )
                .await;
            // The edit itself is done. A fix round that cannot reach the
            // model leaves the best version as it is.
            let content = match reply {
                Ok(content) => content,
                Err(error) => {
                    log(&format!("{label}: fix round {round} failed: {error}"));
                    break;
                }
            };
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&mailing, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => mailing = improved,
                Ok(_) => log(&format!(
                    "{label}: the fix failed validation; keeping the previous version"
                )),
                Err(parse_error) => log(&format!(
                    "{label}: fix reply unusable ({parse_error}); keeping the previous version"
                )),
            }
            fix.context
                .report(DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32);
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_count)
        ));
        fix.context.report(1.0);
        Ok(best)
    }

    /// PNG screenshots of the mailing's emails for the polish pass, at
    /// most `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images
    /// or no Chrome is installed.
    async fn email_images(&self, mailing: &Mailing, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !supports_vision(self.model.model()) {
            return Vec::new();
        }
        if find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for email images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = mailing.emails.len().min(POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let mailing = mailing.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = screenshot_email(&mailing, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: email {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}
