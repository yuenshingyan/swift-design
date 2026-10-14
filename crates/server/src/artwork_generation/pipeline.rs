use super::*;

impl GenerationEngine {
    /// The artwork store, or the failure an artwork run reports
    /// without one.
    fn artwork_store(&self) -> Result<&ArtworkStore, GenerationStop> {
        self.artworks.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no artwork store: artwork sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview artworks the latest user turn asked to continue:
    /// every artwork named by a trailing continue request that still
    /// is a preview.
    pub(crate) async fn continue_artwork_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(artworks) = &self.artworks else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for artwork_id in trailing_continue_ids(&messages) {
            // An artwork that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(artwork)) = artworks.load(&artwork_id).await
                && artwork.is_preview()
            {
                previews.push(artwork_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for an artwork session and returns the
    /// outcome.
    pub(crate) async fn execute_artwork(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => {
                self.generate_artwork_candidates(client, context, log).await
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
                let edits = self.edit_artworks(client, context, &order, log).await?;
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
                let edits = self.edit_artworks(client, context, &order, log).await?;
                Ok(GenerationOutcome::edited(edits))
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let artwork_id = self
                    .merge_artworks(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![artwork_id],
                })
            }
            GenerationTask::Continue(artwork_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, artwork_ids, log)
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
                        "no artwork was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one artwork per requested variation. Returns the ids.
    async fn generate_artwork_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let artworks = self.artwork_store()?;
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
        let first_number = match artworks.list().await {
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
                let request = ArtworkCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_covers: context.preview_screens(),
                    artwork_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_artwork_candidate(&client, &request, &attachments, &share, &log)
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
            if matches!(artworks.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no artwork candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one artwork candidate, repairs it through
    /// fix rounds until it validates, and polishes it. The artwork is
    /// saved under `request.artwork_id` while it streams in, when the
    /// draft validates, and once more after the polish.
    async fn generate_artwork_candidate(
        &self,
        client: &reqwest::Client,
        request: &ArtworkCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Artwork, GenerationStop> {
        let artworks = self.artwork_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": artwork_system_prompt() }),
            self.user_message(&artwork_candidate_prompt(request), attachments),
        ];
        let saver = ArtworkLiveSaver::new(artworks, &self.notifier, &request.artwork_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_artwork),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(artwork) = partial_artwork(text) {
                    let rank = artwork.covers.len();
                    live_saver.offer(artwork, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.covers.len());
        let polished = self
            .polish_artwork(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        artworks
            .clear_user_paths(&request.artwork_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new artwork candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_artworks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let artworks = self.artwork_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let artwork = artworks
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("artwork `{id}` does not exist")))?;
            loaded.push((id.as_str(), artwork));
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
        let rows = artworks
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let artwork_id = candidate_id(base, number);
        log(&format!("merging {} into {artwork_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&artwork_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = ArtworkCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_covers: None,
            artwork_id: artwork_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_artwork_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {artwork_id}"));
        Ok(artwork_id)
    }

    /// Applies `instruction` to each artwork in turn and returns the
    /// ones it saved. One failure is logged and the rest still run; the
    /// turn fails only when every edit failed.
    async fn edit_artworks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for artwork_id in order.artifact_ids {
            match self
                .edit_artwork(client, context, artwork_id, order, log)
                .await
            {
                Ok(report) => saved.push((artwork_id.clone(), report)),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {artwork_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_artwork(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        artwork_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let artworks = self.artwork_store()?;
        let artwork = artworks
            .load(artwork_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("artwork `{artwork_id}` does not exist"))
            })?;
        let instruction = order.instruction;
        let label = format!("edit {artwork_id}");
        // A change that names covers is about those covers: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named covers without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "cover")
            .into_iter()
            .filter(|index| *index < artwork.covers.len())
            .collect();
        let measured = dom_findings(&artwork, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "covers", &indexes);
        let total = artwork.covers.len();
        let (artwork_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&artwork), String::new())
        } else if order.is_fresh {
            (
                focused_artwork_json(&artwork, &indexes, true),
                fresh_note("cover", "covers", &indexes, total),
            )
        } else {
            (
                focused_artwork_json(&artwork, &indexes, false),
                focus_note("cover", "covers", &indexes, total),
            )
        };
        let artwork_json =
            artwork_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &artwork_json,
            note: &note,
            findings: &findings,
            conversation: order.conversation,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": artwork_system_prompt() }),
            self.user_message(&artwork_edit_prompt(&context.request, &input), &attachments),
        ];
        let original = artwork.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| apply_patch(&original, parse_patch(content)?)),
            progress: self.shared_progress(&[artwork_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_edit(
                client,
                EditExecution {
                    original: &artwork,
                    messages,
                    input: &input,
                    indexes: &indexes,
                    is_fresh: order.is_fresh,
                    request: &request,
                },
                log,
            )
            .await?;
        // A fix can make a new problem. The touched covers are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&artwork.covers, &edited.covers, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_artwork = self
            .fix_edited_artwork(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        let report = saved_edit_report(artwork_id, context.request.kind, &artwork, &final_artwork)
            .map_err(GenerationStop::Failed)?;
        artworks
            .save(artwork_id, &final_artwork)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {artwork_id}: saved"));
        Ok(report)
    }

    /// Writes the remaining covers of the preview artwork `artwork_id`
    /// in chunks. The artwork is saved after every chunk, so the canvas
    /// shows it grow, then polished once it is complete. Returns how
    /// many covers were added; 0 when the artwork is complete already.
    pub(crate) async fn continue_artwork(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        artwork_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let artworks = self.artwork_store().map_err(stop_to_string)?;
        let mut artwork = artworks
            .load(artwork_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("artwork `{artwork_id}` does not exist"))?;
        // A run that stopped may have left placeholder covers behind.
        artwork.covers.retain(|cover| !is_pending_cover(cover));
        if !artwork.is_preview() {
            log(&format!(
                "continue {artwork_id}: the artwork is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {artwork_id}");
        let start = artwork.covers.len();
        let planned = artwork.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} covers written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = ArtworkLiveSaver::new(artworks, &self.notifier, artwork_id);
        let board = self
            .write_artwork_chunks(
                client,
                context,
                &artwork,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = artwork.clone();
        if let Ok(board) = board.lock() {
            for covers in board.iter() {
                continued.covers.extend(covers.iter().cloned());
            }
        }
        let added = continued.covers.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the artwork stays continuable.
            if let Err(error) = saver.finish(&artwork).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added a cover"));
        }
        // A failed chunk leaves the artwork continuable: the outline
        // stays until every title has a cover.
        if continued.covers.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_artwork),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_artwork = self
            .polish_artwork(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_artwork).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new covers"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_artwork_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Artwork,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &ArtworkLiveSaver,
        log: &LogSink,
    ) -> ArtworkChunkBoard {
        let start = preview.covers.len();
        let planned = preview.outline.len();
        let board: ArtworkChunkBoard = Arc::new(std::sync::Mutex::new(
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
                saver.offer(shown_artwork(&preview, &chunks, &board), written);
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
                    .write_artwork_chunk(
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
    /// showing the artwork grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_artwork_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Artwork,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &ArtworkChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let artwork_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": artwork_system_prompt() }),
            self.user_message(
                &artwork_continue_prompt(&context.request, preview, &artwork_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.covers.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_artwork_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let covers = partial_continuation_covers(written, text);
                if let Ok(mut board) = live_board.lock()
                    && covers.len() > board[position].len()
                {
                    board[position] = covers;
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
        let covers: Vec<Cover> = continued.covers[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = covers;
        }
        show();
        Ok(())
    }

    /// Reviews a valid artwork as an artwork designer, one round per
    /// effort level. An improved artwork that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_artwork(
        &self,
        client: &reqwest::Client,
        mut artwork: Artwork,
        context: &ArtifactRequest<'_, Artwork>,
        log: &LogSink,
    ) -> Result<Artwork, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !can_audit() {
            log(&format!(
                "{label}: {}",
                PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(artwork);
        }
        let limit = polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = artwork.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings = dom_findings(&artwork, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = artwork.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change an artwork that is already good.
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
            let images = self.cover_images(&artwork, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} cover images)",
                findings.len(),
                images.len()
            ));
            let artwork_json =
                serde_json::to_string(&artwork).map_err(|error| error.to_string())?;
            let prompt = polish_prompt(&artwork_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": artwork_system_prompt() }),
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
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&artwork, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => artwork = improved,
                Ok(_) => log(&format!(
                    "{label}: polished artwork failed validation; keeping the previous version"
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

    /// Measures the touched covers of an edited artwork and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// covers measure clean, a round does not help, or the effort's round
    /// limit runs out. Returns the best version measured.
    async fn fix_edited_artwork(
        &self,
        client: &reqwest::Client,
        mut artwork: Artwork,
        fix: &EditFix<'_, Artwork>,
        log: &LogSink,
    ) -> Result<Artwork, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !can_audit() {
            fix.context.report(1.0);
            return Ok(artwork);
        }
        let limit = polish_round_limit(&fix.context.effort);
        let mut best = artwork.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured = dom_findings(&artwork, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "covers", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = artwork.clone();
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
                "{label}: fix round {round} of at most {limit} ({} findings on the touched covers)",
                findings.len()
            ));
            let artwork_json = focused_artwork_json(&artwork, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("cover", "covers", &fix.indexes, artwork.covers.len());
            let instruction = fix_instruction("covers");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &artwork_json,
                note: &note,
                findings: &findings,
                conversation: "",
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": artwork_system_prompt() }),
                serde_json::json!({ "role": "user", "content": artwork_edit_prompt(fix.request, &input) }),
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
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&artwork, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => artwork = improved,
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

    /// PNG screenshots of the artwork's covers for the polish pass, at
    /// most `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images
    /// or no Chrome is installed.
    async fn cover_images(&self, artwork: &Artwork, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !supports_vision(self.model.model()) {
            return Vec::new();
        }
        if find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for cover images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = artwork.covers.len().min(POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let artwork = artwork.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = screenshot_cover(&artwork, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: cover {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}
