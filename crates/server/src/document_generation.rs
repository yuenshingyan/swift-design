//! The document half of the built-in generation engine.
//!
//! A document session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for documents: the document prompts, the document
//! patch, the document store, and page-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Document, Page, Paper};

use crate::candidates::{candidate_id, next_candidate_number};
use crate::concepts::{Concept, concept_note};
use crate::documents::{DocumentStore, PENDING_PAGE_CLASS, is_pending_page};
use crate::edit_focus::{
    EditFix, EditInput, EditOrder, MergeInput, findings_for, findings_note, fix_instruction,
    focus_note, fresh_note, merge_note, merge_sources, referenced_indexes, touched_indexes,
};
use crate::events::ChangeNotifier;
use crate::generation::{
    ArtifactRequest, Attachments, CONTINUE_DRAFT_SHARE, ContinueChunk, DRAFT_SHARE,
    GenerationContext, GenerationEngine, GenerationOutcome, GenerationStop, GenerationTask,
    ShareSink, candidate_template, complete_array_items, continue_chunks, failure_message,
    stop_to_string, template_note, user_content_with_images, writing_effort,
};
use crate::instructions::DOCUMENT_RULES;
use crate::model_client::LogSink;
use crate::request::{SessionRequest, request_input};

/// The pages each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type DocumentChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Page>>>>;

/// What one document candidate call needs.
struct DocumentCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` pages plus the outline.
    preview_pages: Option<usize>,
    /// The id the candidate is saved under.
    document_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

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
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for document_id in crate::generation::trailing_continue_ids(&messages) {
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
            } => {
                let order = EditOrder {
                    artifact_ids: &designs,
                    instruction: &instruction,
                    is_fresh: false,
                };
                let design_ids = self.edit_documents(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Regenerate {
                design,
                instruction,
            } => {
                let order = EditOrder {
                    artifact_ids: std::slice::from_ref(&design),
                    instruction: &instruction,
                    is_fresh: true,
                };
                let design_ids = self.edit_documents(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
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
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for document_id in order.artifact_ids {
            match self
                .edit_document(client, context, document_id, order, log)
                .await
            {
                Ok(()) => saved.push(document_id.clone()),
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
    ) -> Result<(), GenerationStop> {
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
        let measured =
            crate::document_polish::dom_findings(&document, &self.base_url(), &label, log).await;
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
            parse: Box::new(move |content| {
                crate::document_patch::apply_patch(
                    &original,
                    crate::document_patch::parse_patch(content)?,
                )
            }),
            progress: self.shared_progress(&[document_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
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
        documents
            .save(document_id, &final_document)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {document_id}: saved"));
        Ok(())
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
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(document);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = document.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::document_polish::dom_findings(&document, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = document.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a document that is already good.
            if findings.is_empty() {
                stop = crate::polish::PolishStop::Clean;
                break;
            }
            // The last round did not reduce the findings, so the next
            // will not either.
            if previous_count.is_some_and(|before| findings.len() >= before) {
                stop = crate::polish::PolishStop::NoImprovement;
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
            let prompt =
                crate::document_polish::polish_prompt(&document_json, &findings, images.len());
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
            let improved = crate::document_patch::parse_patch(&content)
                .and_then(|patch| crate::document_patch::apply_patch(&document, patch));
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
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(document);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = document.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::document_polish::dom_findings(&document, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "pages", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = document.clone();
            }
            if findings.is_empty() {
                stop = crate::polish::PolishStop::Clean;
                break;
            }
            if previous_count.is_some_and(|before| findings.len() >= before) {
                stop = crate::polish::PolishStop::NoImprovement;
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
            let improved = crate::document_patch::parse_patch(&content)
                .and_then(|patch| crate::document_patch::apply_patch(&document, patch));
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
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for page images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = document
            .pages
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let document = document.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_page(&document, index, &base_url).await;
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

/// Saves a document while it streams in, so the canvas shows the pages
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct DocumentLiveSaver {
    documents: DocumentStore,
    notifier: ChangeNotifier,
    document_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final document. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl DocumentLiveSaver {
    fn new(documents: &DocumentStore, notifier: &ChangeNotifier, document_id: &str) -> Self {
        Self {
            documents: documents.clone(),
            notifier: notifier.clone(),
            document_id: document_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial document. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, document: Document, rank: usize) {
        if !document.validate().is_empty() {
            return;
        }
        {
            let Ok(mut saved) = self.saved_rank.lock() else {
                return;
            };
            if saved.is_some_and(|saved| rank <= saved) {
                return;
            }
            *saved = Some(rank);
        }
        let saver = self.clone();
        tokio::spawn(async move {
            let _guard = saver.write_lock.lock().await;
            if saver.is_finished.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            if saver
                .documents
                .save(&saver.document_id, &document)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final document after every partial save landed.
    async fn finish(&self, document: &Document) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.documents
            .save(&self.document_id, document)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The document a streaming reply has written so far: everything before
/// the pages plus every complete page. `None` until the first page is
/// complete, or when the text before the pages is not a document.
fn partial_document(text: &str) -> Option<Document> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "pages")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new pages a streaming continuation reply has completed so far.
fn partial_continuation_pages(written: usize, text: &str) -> Vec<Page> {
    let Some((_, items)) = complete_array_items(text, "pages") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"pages\":[{}]}}", items.join(","));
    continuation_pages(written, &json).unwrap_or_default()
}

/// The document to show while the chunks run: the preview, then every
/// chunk up to the last one that has pages, with placeholders for the
/// pages an earlier chunk still owes.
fn shown_document(preview: &Document, chunks: &[ContinueChunk], board: &[Vec<Page>]) -> Document {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|pages| !pages.is_empty()) else {
        return shown;
    };
    for (chunk, pages) in chunks.iter().zip(board).take(last) {
        shown.pages.extend(pages.iter().cloned());
        for offset in pages.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.pages.push(placeholder_page(title));
        }
    }
    shown.pages.extend(board[last].iter().cloned());
    shown
}

/// A page that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a document that does
/// not.
fn placeholder_page(title: &str) -> Page {
    Page {
        html: format!(
            "<div class=\"{PENDING_PAGE_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
             <h2 class=\"pending-title\">{}</h2></div>",
            crate::render::escape_html(title),
        ),
        css: Some(
            ".pending { display: flex; flex-direction: column; align-items: center; \
             justify-content: center; height: 100%; gap: 16px; opacity: 0.55; }\n\
             .pending-label { margin: 0; font-size: 14px; letter-spacing: 0.3em; \
             text-transform: uppercase; color: var(--muted); }\n\
             .pending-title { margin: 0; max-width: 600px; text-align: center; \
             font-size: 32px; color: var(--text); }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// The new pages in a continuation reply, in order. Accepts a patch
/// (the pages of its operations at or past the existing pages) and, as
/// a fallback, a whole document (its pages past the existing ones).
fn continuation_pages(written: usize, content: &str) -> Result<Vec<Page>, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    let value: serde_json::Value = serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid JSON: {error}"))?;
    let items = value
        .get("pages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no pages array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("page").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("page"))
            .filter(|page| page.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, page)| {
            serde_json::from_value::<Page>(page.clone()).map_err(|error| {
                format!(
                    "new page {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new pages to the document in progress. The
/// outline stays until every title has a page, so a short reply leaves
/// the document continuable.
fn apply_document_continuation(original: &Document, content: &str) -> Result<Document, String> {
    let new_pages = continuation_pages(original.pages.len(), content)?;
    if new_pages.is_empty() {
        return Err(
            "the reply adds no pages: reply with a patch of inserts, one per new page".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.pages.extend(new_pages);
    if continued.pages.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The document system prompt: role, document rules, the document
/// schema, the clarification protocol, and one example document.
fn document_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Document)).unwrap_or_default();
    format!(
        "You build paged documents as JSON documents: reports, memos, proposals, letters, and guides. \
         Each page is one HTML fragment plus its own CSS, for the px canvas of the document's paper: \
         794 by 1123 px for A4, 816 by 1056 px for Letter.\n\
         Follow these rules:\n{rules}\n\
         The document must conform to this JSON Schema:\n{schema}\n\
         Example document:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one document JSON. No prose, no code fences.",
        rules = DOCUMENT_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-document.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` pages and
/// the full outline.
fn document_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} pages of the document, in order, starting with \
         the first page. Put the page titles of the complete document in `outline`, in order, \
         every page title of the complete document. The app asks you for the remaining pages \
         later. Make these {count} pages show the theme, the layout language, and the text \
         density of the whole document.\n"
    )
}

/// The prompt line for the app's paper choice. Empty when the agent
/// decides, or when the user typed a paper the JSON does not carry.
fn paper_note(paper: Option<&str>) -> String {
    match paper.and_then(Paper::from_name) {
        Some(paper) => {
            let viewport = paper.viewport();
            format!(
                "Lay the pages out on {} paper: {} by {} px. Set `paper` to `{}`.\n",
                paper.label(),
                viewport.width,
                viewport.height,
                paper.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the document to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// pages than the length, so the count goes to the outline instead.
fn page_count_note(page_count: Option<u32>, preview_pages: Option<usize>) -> String {
    let Some(count) = page_count else {
        return String::new();
    };
    match preview_pages {
        Some(_) => {
            format!("The user asked for {count} pages. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} pages. Write exactly {count} pages.\n"),
    }
}

/// The user prompt for one document candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
fn document_candidate_prompt(request: &DocumentCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a document for this request. The request and the answers are authoritative; do \
         not override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are pages or slides of another artifact. Use them for the \
             look only.\n",
        );
    }
    if let Some(count) = request.preview_pages {
        prompt.push_str(&document_preview_note(count));
    }
    prompt.push_str(&page_count_note(options.page_count, request.preview_pages));
    prompt.push_str(&paper_note(options.paper.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("document", merge));
    }
    let count = options.variation_count();
    // A merge is one candidate on its own, not one of the run's set.
    if count > 1 && request.merge.is_none() {
        prompt.push_str(&format!(
            "This is candidate {candidate_number} of {count}. Make it distinct from the other \
             candidates in theme, structure, and angle.\n"
        ));
        prompt.push_str(&concept_note(request.concepts, candidate_number - 1));
    }
    match options.effort.as_str() {
        "low" => prompt.push_str("Keep the document concise: fewer pages, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the document JSON.");
    prompt
}

/// The user prompt for a document edit: the document as it is, the
/// request, and the change the user asked for.
fn document_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the document to change:\n{document_json}\n{note}\
         The document is for this request:\n{request}\n\
         Apply this change: {critique}\n{findings}\
         A reference like [page 3, node 0/1 <h2.title>: What changed] names a page \
         (1-based) and one element in that page's html by its index path from the page root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [page 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one page the same way, without their text. A reference like [page 3] \
         names the page alone: the change is about that page. Change only what the critique asks for. Keep every other page and \
         value as it is. Return every changed page complete: html, css, and notes.\n{format}",
        document_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        findings = findings_note(input.findings),
        format = crate::document_patch::PATCH_FORMAT
    )
}

/// The document as a focused edit sees it: the title, the theme, the
/// paper, the page count, and only the pages at `indexes`, each with
/// its index.
fn focused_document_json(
    document: &Document,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let pages: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            document.pages.get(*index).map(|page| {
                let page = if is_fresh {
                    fresh_page(page)
                } else {
                    page.clone()
                };
                serde_json::json!({ "index": index, "page": page })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": document.title,
        "theme": document.theme,
        "paper": document.paper,
        "page_count": document.pages.len(),
        "pages": pages,
    }))
}

/// The page as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
fn fresh_page(page: &Page) -> Page {
    Page {
        html: String::new(),
        css: None,
        ..page.clone()
    }
}

/// The user prompt for one document continuation chunk: the preview
/// document and the chunk's pages to add, as a patch of inserts.
fn document_continue_prompt(
    request: &SessionRequest,
    document: &Document,
    document_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = document.pages.len();
    let planned = document.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = document
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a document in progress: its theme, its paper, its first {written} pages, and \
         `outline`, the page titles of the complete document:\n{document_json}\n\
         The document is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} pages: outline titles {} to {last} of {planned}, in order, one page per \
         title:\n{}\n\
         Keep the theme. Match the existing pages in CSS style, font sizes, spacing, colors, \
         and visual language, so the document reads as one document. Do not change or repeat \
         the existing pages.\n",
        next_titles.len(),
        first + 1,
        next_titles.join("\n")
    ));
    if first > written || last < planned {
        prompt.push_str(
            "Other requests write the other outline titles at the same time. Write only these.\n",
        );
    }
    prompt.push_str(&format!(
        "Reply with only a JSON patch that appends the new pages, not the whole document:\n\
         {{\"pages\":[{{\"index\":{written},\"insert\":true,\"page\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new page index {written} and insert true, in reading order. Each page \
         carries html, css, and notes. Omit title, theme, paper, outline, and the existing pages."
    ));
    prompt
}

/// Extracts and parses the document JSON from a model reply.
fn parse_document(content: &str) -> Result<Document, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end])
        .map_err(|error| format!("invalid document: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        apply_document_continuation, continuation_pages, document_edit_prompt,
        document_system_prompt, focused_document_json, page_count_note, paper_note, parse_document,
        partial_document, placeholder_page, shown_document,
    };
    use crate::designs::DesignStore;
    use crate::documents::DocumentStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{
        FakeModelServer, SAMPLE_DOCUMENT, low_effort_options, sample_document,
    };

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_document_edit_shows_only_the_named_pages_and_their_findings() {
        let document = sample_document();
        let focused = focused_document_json(&document, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["page_count"], document.pages.len());
        assert_eq!(value["paper"], "a4");
        assert_eq!(value["pages"].as_array().unwrap().len(), 1);
        assert_eq!(value["pages"][0]["index"], 1);
        let request = SessionRequest {
            request: "A report.".to_owned(),
            kind: ArtifactKind::Document,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["pages[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[page 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only page 2 is shown.\n",
            findings: &findings,
        };
        let prompt = document_edit_prompt(&request, &input);
        assert!(prompt.contains("Only page 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- pages[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [page 2, node 0/2 <p>: x] Fix the overflow."));
        assert!(!prompt.contains("slide"));
    }

    #[test]
    fn the_page_count_and_the_paper_hold_the_document_to_the_asked_shape() {
        assert_eq!(paper_note(None), "");
        assert_eq!(paper_note(Some("tabloid")), "");
        assert_eq!(
            paper_note(Some("letter")),
            "Lay the pages out on Letter paper: 816 by 1056 px. Set `paper` to `letter`.\n"
        );
        assert_eq!(page_count_note(None, None), "");
        assert_eq!(page_count_note(None, Some(3)), "");
        assert_eq!(
            page_count_note(Some(12), None),
            "The user asked for 12 pages. Write exactly 12 pages.\n"
        );
        // A preview writes three pages, so the length goes to the outline.
        assert_eq!(
            page_count_note(Some(12), Some(3)),
            "The user asked for 12 pages. Put exactly 12 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        documents: DocumentStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            documents: DocumentStore::new(directory.path().join("documents")),
            sessions: SessionStore::new(directory.path().join("sessions")),
        }
    }

    fn engine(server: &FakeModelServer, stores: &Stores) -> GenerationEngine {
        GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        )
        .with_documents(stores.documents.clone())
    }

    /// A fresh one-candidate, low-effort document session, still in
    /// intake.
    async fn document_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("report", "Report", "A report.")
                    .with_kind(ArtifactKind::Document)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// A document session past its setup card: the app's own questions
    /// were asked, so the next planner turn is free to write.
    async fn set_up_document_session(sessions: &SessionStore) {
        document_session(sessions).await;
        sessions
            .apply("report", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn document_system_prompt_carries_document_rules_the_schema_and_the_example() {
        let prompt = document_system_prompt();
        assert!(prompt.contains("paged documents"));
        assert!(prompt.contains("794 by 1123 px"));
        assert!(prompt.contains("\"pages\""));
        assert!(prompt.contains("\"paper\""));
        assert!(prompt.contains("Swift Design Quarterly Report"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
        assert!(!prompt.contains("\"slides\""));
    }

    #[test]
    fn partial_document_returns_complete_pages_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"paper":"letter","pages":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let document = partial_document(text).unwrap();
        assert_eq!(document.pages.len(), 1);
        assert_eq!(document.paper, design_model::Paper::Letter);
        assert!(partial_document("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_pages_reject_a_short_reply() {
        let mut preview = sample_document();
        preview.outline = vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
        ];
        assert!(apply_document_continuation(&preview, "{\"pages\":[]}").is_err());
        let patch = r#"{"pages":[{"index":3,"insert":true,"page":{"html":"<h2>D</h2>"}}]}"#;
        let continued = apply_document_continuation(&preview, patch).unwrap();
        assert_eq!(continued.pages.len(), 4);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_pages(3, patch).unwrap().len(), 1);
        assert!(continuation_pages(3, "{\"slides\":[]}").is_err());
        assert!(parse_document("no json").is_err());
    }

    #[test]
    fn shown_documents_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_document();
        preview.outline = (1..=7).map(|number| format!("Page {number}")).collect();
        let chunks = [
            ContinueChunk { first: 3, count: 2 },
            ContinueChunk { first: 5, count: 2 },
        ];
        let board = vec![
            Vec::new(),
            vec![placeholder_page("x"), placeholder_page("y")],
        ];
        let shown = shown_document(&preview, &chunks, &board);
        assert_eq!(shown.pages.len(), 7);
        assert!(shown.pages[3].html.contains("Page 4"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_document_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        document_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            GenerationOutcome::NeedsClarification { question_set: 1 }
        ));
        let set = stores
            .sessions
            .read_question_set("report", 1)
            .await
            .unwrap()
            .unwrap();
        assert!(set.questions.is_empty());
        assert!(set.can_proceed_with_assumptions);
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_valid_document_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DOCUMENT);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_document_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .documents
                .load("report-candidate-1")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            stores
                .designs
                .load("report-candidate-1")
                .await
                .unwrap()
                .is_none()
        );
        let planner = server.requests()[0].to_string();
        assert!(planner.contains("You plan documents"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("paged documents"));
        assert!(request.contains("Build a document"));
        let runs = stores.sessions.runs("report").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_document_open_patches_that_document() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"pages":[{"index":0,"page":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .documents
            .save("report-candidate-1", &sample_document())
            .await
            .unwrap();
        document_session(&stores.sessions).await;
        stores
            .sessions
            .apply("report", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        stores
            .sessions
            .apply("report", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::user("Tighten the title.", Some("report-candidate-1")),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let edited = stores
            .documents
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.pages[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("report").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing document session with `count` saved candidates.
    async fn reviewing_document_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .documents
                .save(&format!("report-candidate-{number}"), &sample_document())
                .await
                .unwrap();
        }
        set_up_document_session(&stores.sessions).await;
        stores
            .sessions
            .apply("report", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        stores
            .sessions
            .apply("report", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_regenerated_page_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its page itself.
        server.push_text(
            r#"{"pages":[{"index":0,"page":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut document = sample_document();
        document.pages[0].html = "<h1>Old title markup</h1>".to_owned();
        stores
            .documents
            .save("report-candidate-1", &document)
            .await
            .unwrap();
        reviewing_document_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::regenerate_request(
                    "[page 1] Write this page anew.",
                    "report-candidate-1",
                ),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let text = server.requests()[0].to_string();
        assert!(text.contains("Write page 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .documents
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.pages[0].html.contains("Fresh"));
        assert_eq!(edited.pages.len(), document.pages.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_documents_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_DOCUMENT);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"pages":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_document_session_with(&stores, 2).await;
        let pinned = vec![
            "report-candidate-1".to_owned(),
            "report-candidate-2".to_owned(),
        ];
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::user(
                    "[candidate 1] [candidate 2] Cover from 1, table from 2.",
                    None,
                )
                .with_pinned(pinned),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        let GenerationOutcome::Wrote { design_ids } = outcome else {
            panic!("expected a write");
        };
        assert_eq!(design_ids, vec!["report-candidate-3".to_owned()]);
        assert!(
            stores
                .documents
                .load("report-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one document"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_document_session_without_a_document_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DOCUMENT);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_document_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("report", silent_log()).await.unwrap_err();
        assert!(error.contains("no document store"));
    }
}
