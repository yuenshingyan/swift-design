use super::*;

impl GenerationEngine {
    /// The document store, or the failure a document run reports
    /// without one.
    fn document_store(&self) -> Result<&DocumentStore, GenerationStop> {
        self.documents.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no document store: document sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview documents the latest user turn asked to continue:
    /// every document named by a trailing continue request that still
    /// is a preview.
    pub(crate) async fn continue_document_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(documents) = &self.documents else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for document_id in trailing_continue_ids(&messages) {
            // A document that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(document)) = documents.load(&document_id).await
                && document.is_preview()
            {
                previews.push(document_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for a document session and returns the
    /// outcome.
    pub(crate) async fn execute_document(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => {
                self.generate_document_candidates(client, context, log)
                    .await
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
                let edits = self.edit_documents(client, context, &order, log).await?;
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
                let edits = self.edit_documents(client, context, &order, log).await?;
                Ok(GenerationOutcome::edited(edits))
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let document_id = self
                    .merge_documents(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![document_id],
                })
            }
            GenerationTask::Continue(document_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, document_ids, log)
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
                        "no document was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one document per requested variation. Returns the ids.
    async fn generate_document_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let documents = self.document_store()?;
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
        let first_number = match documents.list().await {
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
                let request = DocumentCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_pages: context.preview_screens(),
                    document_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_document_candidate(&client, &request, &attachments, &share, &log)
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
            if matches!(documents.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no document candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one document candidate, repairs it through
    /// fix rounds until it validates, and polishes it. The document is
    /// saved under `request.document_id` while it streams in, when the
    /// draft validates, and once more after the polish.
    async fn generate_document_candidate(
        &self,
        client: &reqwest::Client,
        request: &DocumentCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Document, GenerationStop> {
        let documents = self.document_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": document_system_prompt() }),
            self.user_message(&document_candidate_prompt(request), attachments),
        ];
        let saver = DocumentLiveSaver::new(documents, &self.notifier, &request.document_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_document),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(document) = partial_document(text) {
                    let rank = document.pages.len();
                    live_saver.offer(document, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.pages.len());
        let polished = self
            .polish_document(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        documents
            .clear_user_paths(&request.document_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new document candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_documents(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let documents = self.document_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let document = documents
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("document `{id}` does not exist")))?;
            loaded.push((id.as_str(), document));
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
        let rows = documents
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let document_id = candidate_id(base, number);
        log(&format!(
            "merging {} into {document_id}",
            sources.join(", ")
        ));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&document_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = DocumentCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_pages: None,
            document_id: document_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_document_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {document_id}"));
        Ok(document_id)
    }

    /// Applies `instruction` to each document in turn and returns the
    /// ones it saved. One failure is logged and the rest still run; the
    /// turn fails only when every edit failed.
    async fn edit_documents(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for document_id in order.artifact_ids {
            match self
                .edit_document(client, context, document_id, order, log)
                .await
            {
                Ok(report) => saved.push((document_id.clone(), report)),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {document_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_document(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        document_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let documents = self.document_store()?;
        let document = documents
            .load(document_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("document `{document_id}` does not exist"))
            })?;
        let instruction = order.instruction;
        let label = format!("edit {document_id}");
        // A change that names pages is about those pages: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named pages without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "page")
            .into_iter()
            .filter(|index| *index < document.pages.len())
            .collect();
        let measured = dom_findings(&document, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "pages", &indexes);
        let total = document.pages.len();
        let (document_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&document), String::new())
        } else if order.is_fresh {
            (
                focused_document_json(&document, &indexes, true),
                fresh_note("page", "pages", &indexes, total),
            )
        } else {
            (
                focused_document_json(&document, &indexes, false),
                focus_note("page", "pages", &indexes, total),
            )
        };
        let document_json =
            document_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &document_json,
            note: &note,
            findings: &findings,
            conversation: order.conversation,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": document_system_prompt() }),
            self.user_message(
                &document_edit_prompt(&context.request, &input),
                &attachments,
            ),
        ];
        let original = document.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| apply_patch(&original, parse_patch(content)?)),
            progress: self.shared_progress(&[document_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_edit(
                client,
                EditExecution {
                    original: &document,
                    messages,
                    input: &input,
                    indexes: &indexes,
                    is_fresh: order.is_fresh,
                    request: &request,
                },
                log,
            )
            .await?;
        // A fix can make a new problem. The touched pages are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&document.pages, &edited.pages, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_document = self
            .fix_edited_document(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        let report = saved_edit_report(
            document_id,
            context.request.kind,
            &document,
            &final_document,
        )
        .map_err(GenerationStop::Failed)?;
        documents
            .save(document_id, &final_document)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {document_id}: saved"));
        Ok(report)
    }

    /// Writes the remaining pages of the preview document `document_id`
    /// in chunks. The document is saved after every chunk, so the canvas
    /// shows it grow, then polished once it is complete. Returns how
    /// many pages were added; 0 when the document is complete already.
    pub(crate) async fn continue_document(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        document_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let documents = self.document_store().map_err(stop_to_string)?;
        let mut document = documents
            .load(document_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("document `{document_id}` does not exist"))?;
        // A run that stopped may have left placeholder pages behind.
        document.pages.retain(|page| !is_pending_page(page));
        if !document.is_preview() {
            log(&format!(
                "continue {document_id}: the document is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {document_id}");
        let start = document.pages.len();
        let planned = document.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} pages written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = DocumentLiveSaver::new(documents, &self.notifier, document_id);
        let board = self
            .write_document_chunks(
                client,
                context,
                &document,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = document.clone();
        if let Ok(board) = board.lock() {
            for pages in board.iter() {
                continued.pages.extend(pages.iter().cloned());
            }
        }
        let added = continued.pages.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the document stays continuable.
            if let Err(error) = saver.finish(&document).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added a page"));
        }
        // A failed chunk leaves the document continuable: the outline
        // stays until every title has a page.
        if continued.pages.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_document),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_document = self
            .polish_document(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_document).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new pages"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_document_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Document,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &DocumentLiveSaver,
        log: &LogSink,
    ) -> DocumentChunkBoard {
        let start = preview.pages.len();
        let planned = preview.outline.len();
        let board: DocumentChunkBoard = Arc::new(std::sync::Mutex::new(
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
                saver.offer(shown_document(&preview, &chunks, &board), written);
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
                    .write_document_chunk(
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
    /// showing the document grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_document_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Document,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &DocumentChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let document_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": document_system_prompt() }),
            self.user_message(
                &document_continue_prompt(&context.request, preview, &document_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.pages.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_document_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let pages = partial_continuation_pages(written, text);
                if let Ok(mut board) = live_board.lock()
                    && pages.len() > board[position].len()
                {
                    board[position] = pages;
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
        let pages: Vec<Page> = continued.pages[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = pages;
        }
        show();
        Ok(())
    }

    /// Reviews a valid document as an editorial designer, one round per
    /// effort level. An improved document that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_document(
        &self,
        client: &reqwest::Client,
        mut document: Document,
        context: &ArtifactRequest<'_, Document>,
        log: &LogSink,
    ) -> Result<Document, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !can_audit() {
            log(&format!(
                "{label}: {}",
                PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(document);
        }
        let limit = polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = document.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings = dom_findings(&document, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = document.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a document that is already good.
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
            let images = self.page_images(&document, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} page images)",
                findings.len(),
                images.len()
            ));
            let document_json =
                serde_json::to_string(&document).map_err(|error| error.to_string())?;
            let prompt = polish_prompt(&document_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": document_system_prompt() }),
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
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&document, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => document = improved,
                Ok(_) => log(&format!(
                    "{label}: polished document failed validation; keeping the previous version"
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

    /// Measures the touched pages of an edited document and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// pages measure clean, a round does not help, or the effort's round
    /// limit runs out. Returns the best version measured.
    async fn fix_edited_document(
        &self,
        client: &reqwest::Client,
        mut document: Document,
        fix: &EditFix<'_, Document>,
        log: &LogSink,
    ) -> Result<Document, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !can_audit() {
            fix.context.report(1.0);
            return Ok(document);
        }
        let limit = polish_round_limit(&fix.context.effort);
        let mut best = document.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured = dom_findings(&document, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "pages", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = document.clone();
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
                "{label}: fix round {round} of at most {limit} ({} findings on the touched pages)",
                findings.len()
            ));
            let document_json = focused_document_json(&document, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("page", "pages", &fix.indexes, document.pages.len());
            let instruction = fix_instruction("pages");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &document_json,
                note: &note,
                findings: &findings,
                conversation: "",
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": document_system_prompt() }),
                serde_json::json!({ "role": "user", "content": document_edit_prompt(fix.request, &input) }),
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
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&document, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => document = improved,
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

    /// PNG screenshots of the document's pages for the polish pass, at
    /// most `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images
    /// or no Chrome is installed.
    async fn page_images(&self, document: &Document, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !supports_vision(self.model.model()) {
            return Vec::new();
        }
        if find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for page images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = document.pages.len().min(POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let document = document.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = screenshot_page(&document, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: page {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}
