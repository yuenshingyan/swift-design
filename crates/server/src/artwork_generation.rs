//! The artwork half of the built-in generation engine.
//!
//! An artwork session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for artworks: the artwork prompts, the artwork
//! patch, the artwork store, and cover-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Artwork, Cover, CoverSize};

use crate::artworks::{ArtworkStore, PENDING_AD_CLASS, is_pending_cover};
use crate::candidates::{candidate_id, next_candidate_number};
use crate::concepts::{Concept, concept_note};
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
use crate::instructions::ARTWORK_RULES;
use crate::model_client::LogSink;
use crate::request::{SessionRequest, request_input};

/// The covers each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type ArtworkChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Cover>>>>;

/// What one artwork candidate call needs.
struct ArtworkCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` covers plus the outline.
    preview_covers: Option<usize>,
    /// The id the candidate is saved under.
    artwork_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

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
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for artwork_id in crate::generation::trailing_continue_ids(&messages) {
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
                let design_ids = self.edit_artworks(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
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
                let design_ids = self.edit_artworks(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
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
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for artwork_id in order.artifact_ids {
            match self
                .edit_artwork(client, context, artwork_id, order, log)
                .await
            {
                Ok(()) => saved.push(artwork_id.clone()),
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
    ) -> Result<(), GenerationStop> {
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
        let measured =
            crate::artwork_polish::dom_findings(&artwork, &self.base_url(), &label, log).await;
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
            parse: Box::new(move |content| {
                crate::artwork_patch::apply_patch(
                    &original,
                    crate::artwork_patch::parse_patch(content)?,
                )
            }),
            progress: self.shared_progress(&[artwork_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
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
        artworks
            .save(artwork_id, &final_artwork)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {artwork_id}: saved"));
        Ok(())
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
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(artwork);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = artwork.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::artwork_polish::dom_findings(&artwork, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = artwork.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change an artwork that is already good.
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
            let images = self.cover_images(&artwork, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} cover images)",
                findings.len(),
                images.len()
            ));
            let artwork_json =
                serde_json::to_string(&artwork).map_err(|error| error.to_string())?;
            let prompt =
                crate::artwork_polish::polish_prompt(&artwork_json, &findings, images.len());
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
            let improved = crate::artwork_patch::parse_patch(&content)
                .and_then(|patch| crate::artwork_patch::apply_patch(&artwork, patch));
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
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(artwork);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = artwork.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::artwork_polish::dom_findings(&artwork, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "covers", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = artwork.clone();
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
            let improved = crate::artwork_patch::parse_patch(&content)
                .and_then(|patch| crate::artwork_patch::apply_patch(&artwork, patch));
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
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for cover images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = artwork
            .covers
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let artwork = artwork.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_cover(&artwork, index, &base_url).await;
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

/// Saves an artwork while it streams in, so the canvas shows the covers
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct ArtworkLiveSaver {
    artworks: ArtworkStore,
    notifier: ChangeNotifier,
    artwork_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final artwork. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl ArtworkLiveSaver {
    fn new(artworks: &ArtworkStore, notifier: &ChangeNotifier, artwork_id: &str) -> Self {
        Self {
            artworks: artworks.clone(),
            notifier: notifier.clone(),
            artwork_id: artwork_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial artwork. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, artwork: Artwork, rank: usize) {
        if !artwork.validate().is_empty() {
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
                .artworks
                .save(&saver.artwork_id, &artwork)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final artwork after every partial save landed.
    async fn finish(&self, artwork: &Artwork) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.artworks
            .save(&self.artwork_id, artwork)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The artwork a streaming reply has written so far: everything before
/// the covers plus every complete cover. `None` until the first cover is
/// complete, or when the text before the covers is not an artwork.
fn partial_artwork(text: &str) -> Option<Artwork> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "covers")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new covers a streaming continuation reply has completed so far.
fn partial_continuation_covers(written: usize, text: &str) -> Vec<Cover> {
    let Some((_, items)) = complete_array_items(text, "covers") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"covers\":[{}]}}", items.join(","));
    continuation_covers(written, &json).unwrap_or_default()
}

/// The artwork to show while the chunks run: the preview, then every
/// chunk up to the last one that has covers, with placeholders for the
/// covers an earlier chunk still owes.
fn shown_artwork(preview: &Artwork, chunks: &[ContinueChunk], board: &[Vec<Cover>]) -> Artwork {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|covers| !covers.is_empty()) else {
        return shown;
    };
    for (chunk, covers) in chunks.iter().zip(board).take(last) {
        shown.covers.extend(covers.iter().cloned());
        for offset in covers.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.covers.push(placeholder_cover(title));
        }
    }
    shown.covers.extend(board[last].iter().cloned());
    shown
}

/// A cover that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops an artwork that does
/// not.
fn placeholder_cover(title: &str) -> Cover {
    Cover {
        html: format!(
            "<div class=\"{PENDING_AD_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new covers in a continuation reply, in order. Accepts a patch
/// (the covers of its operations at or past the existing covers) and, as
/// a fallback, a whole artwork (its covers past the existing ones).
fn continuation_covers(written: usize, content: &str) -> Result<Vec<Cover>, String> {
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
        .get("covers")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no covers array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("cover").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("cover"))
            .filter(|cover| cover.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, cover)| {
            serde_json::from_value::<Cover>(cover.clone()).map_err(|error| {
                format!(
                    "new cover {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new covers to the artwork in progress. The
/// outline stays until every title has a cover, so a short reply
/// leaves the artwork continuable.
fn apply_artwork_continuation(original: &Artwork, content: &str) -> Result<Artwork, String> {
    let new_covers = continuation_covers(original.covers.len(), content)?;
    if new_covers.is_empty() {
        return Err(
            "the reply adds no covers: reply with a patch of inserts, one per new cover".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.covers.extend(new_covers);
    if continued.covers.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The artwork system prompt: role, artwork rules, the artwork
/// schema, the clarification protocol, and one example artwork.
fn artwork_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Artwork)).unwrap_or_default();
    format!(
        "You build cover art as JSON artworks: video thumbnails, channel banners, profile headers, album covers, and book covers judged at a glance. \
         Each cover is one HTML fragment plus its own CSS, for the px canvas of the artwork's size: \
         1280 by 720 px for thumbnail, 2560 by 1440 px for banner, 1500 by 500 px for \
         header, 3000 by 3000 px for album, 1600 by 2560 px for book. \
         One cover is a single piece. Two or more covers are A/B variants of the same size, in priority order.\n\
         Follow these rules:\n{rules}\n\
         The artwork must conform to this JSON Schema:\n{schema}\n\
         Example artwork:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one artwork JSON. No prose, no code fences.",
        rules = ARTWORK_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-artwork.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` covers and
/// the full outline.
fn artwork_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} covers of the artwork, in order, starting with \
         the first cover. Put the cover titles of the complete artwork in `outline`, in order, \
         every cover title of the complete artwork. The app asks you for the remaining covers \
         later. Make these {count} covers show the theme, the layout language, and the text \
         density of the whole artwork.\n"
    )
}

/// The prompt line for the app's size choice. Empty when the agent
/// decides it, or when the user typed a value the JSON does not carry.
fn size_note(size: Option<&str>) -> String {
    match size.and_then(CoverSize::from_name) {
        Some(size) => {
            let viewport = size.viewport();
            format!(
                "Lay the covers out on the {} size: {} by {} px. Set `size` to `{}`.\n",
                size.as_str(),
                viewport.width,
                viewport.height,
                size.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the artwork to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// covers than the length, so the count goes to the outline instead.
fn cover_count_note(cover_count: Option<u32>, preview_covers: Option<usize>) -> String {
    let Some(count) = cover_count else {
        return String::new();
    };
    match preview_covers {
        Some(_) => {
            format!("The user asked for {count} covers. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} covers. Write exactly {count} covers.\n"),
    }
}

/// The user prompt for one artwork candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
fn artwork_candidate_prompt(request: &ArtworkCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build an artwork for this request. The request and the answers are \
         authoritative; do not override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are screens, slides, or pages of another artifact. Use them \
             for the look only.\n",
        );
    }
    if let Some(count) = request.preview_covers {
        prompt.push_str(&artwork_preview_note(count));
    }
    prompt.push_str(&cover_count_note(
        options.cover_count,
        request.preview_covers,
    ));
    prompt.push_str(&size_note(options.cover_size.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("artwork", merge));
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
        "low" => prompt.push_str("Keep the artwork concise: fewer covers, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the artwork JSON.");
    prompt
}

/// The user prompt for an artwork edit: the artwork as it is, the
/// request, and the change the user asked for.
fn artwork_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the artwork to change:\n{artwork_json}\n{note}\
         The artwork is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [cover 3, node 0/1 <h2.title>: What changed] names a cover \
         (1-based) and one element in that cover's html by its index path from the cover root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [cover 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one cover the same way, without their text. A reference like [cover 3] \
         names the cover alone: the change is about that cover. Change only what the critique asks for. Keep every other cover and \
         value as it is. Return every changed cover complete: html, css, and notes.\n{format}",
        artwork_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = crate::edit_focus::conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = crate::artwork_patch::PATCH_FORMAT
    )
}

/// The artwork as a focused edit sees it: the title, the theme, the
/// size, the cover count, and only the covers at `indexes`, each
/// with its index.
fn focused_artwork_json(
    artwork: &Artwork,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let covers: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            artwork.covers.get(*index).map(|cover| {
                let cover = if is_fresh {
                    fresh_cover(cover)
                } else {
                    cover.clone()
                };
                serde_json::json!({ "index": index, "cover": cover })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": artwork.title,
        "theme": artwork.theme,
        "size": artwork.size,
        "cover_count": artwork.covers.len(),
        "covers": covers,
    }))
}

/// The cover as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
fn fresh_cover(cover: &Cover) -> Cover {
    Cover {
        html: String::new(),
        css: None,
        ..cover.clone()
    }
}

/// The user prompt for one artwork continuation chunk: the preview
/// artwork and the chunk's covers to add, as a patch of inserts.
fn artwork_continue_prompt(
    request: &SessionRequest,
    artwork: &Artwork,
    artwork_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = artwork.covers.len();
    let planned = artwork.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = artwork
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is an artwork in progress: its theme, its size, its first {written} \
         covers, and `outline`, the cover titles of the complete artwork:\n{artwork_json}\n\
         The artwork is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} covers: outline titles {} to {last} of {planned}, in order, one cover per \
         title:\n{}\n\
         Keep the theme. Match the existing covers in CSS style, font sizes, spacing, colors, \
         and visual language, so the artwork reads as one piece. Do not change or repeat \
         the existing covers.\n",
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
        "Reply with only a JSON patch that appends the new covers, not the whole artwork:\n\
         {{\"covers\":[{{\"index\":{written},\"insert\":true,\"cover\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new cover index {written} and insert true, in priority order. Each cover \
         carries html, css, and notes. Omit title, theme, size, outline, and the existing covers."
    ));
    prompt
}

/// Extracts and parses the artwork JSON from a model reply.
fn parse_artwork(content: &str) -> Result<Artwork, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid artwork: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        apply_artwork_continuation, artwork_edit_prompt, artwork_system_prompt,
        continuation_covers, cover_count_note, focused_artwork_json, parse_artwork,
        partial_artwork, placeholder_cover, shown_artwork, size_note,
    };
    use crate::artworks::ArtworkStore;
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{
        FakeModelServer, SAMPLE_ARTWORK, low_effort_options, sample_artwork,
    };

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_artwork_edit_shows_only_the_named_covers_and_their_findings() {
        let artwork = sample_artwork();
        let focused = focused_artwork_json(&artwork, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["cover_count"], artwork.covers.len());
        assert_eq!(value["size"], "thumbnail");
        assert_eq!(value["covers"].as_array().unwrap().len(), 1);
        assert_eq!(value["covers"][0]["index"], 1);
        let request = SessionRequest {
            request: "A launch cover.".to_owned(),
            kind: ArtifactKind::Artwork,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["covers[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[cover 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only cover 2 is shown.\n",
            findings: &findings,
            conversation: "Conversation, oldest first:\nuser: earlier ask\n",
        };
        let prompt = artwork_edit_prompt(&request, &input);
        assert!(prompt.contains("Only cover 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- covers[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [cover 2, node 0/2 <p>: x] Fix the overflow."));
        assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
        assert!(prompt.contains("Apply only the change asked below."));
        assert!(!prompt.contains("slide"));
    }

    #[test]
    fn the_cover_count_and_the_size_hold_the_artwork_to_the_asked_shape() {
        assert_eq!(size_note(None), "");
        assert_eq!(size_note(Some("tall")), "");
        assert_eq!(
            size_note(Some("banner")),
            "Lay the covers out on the banner size: 2560 by 1440 px. Set `size` to `banner`.\n"
        );
        assert_eq!(
            size_note(Some("album")),
            "Lay the covers out on the album size: 3000 by 3000 px. Set `size` to `album`.\n"
        );
        assert_eq!(cover_count_note(None, None), "");
        assert_eq!(cover_count_note(None, Some(1)), "");
        assert_eq!(
            cover_count_note(Some(2), None),
            "The user asked for 2 covers. Write exactly 2 covers.\n"
        );
        // A preview writes one cover, so the length goes to the outline.
        assert_eq!(
            cover_count_note(Some(2), Some(1)),
            "The user asked for 2 covers. Put exactly 2 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        artworks: ArtworkStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            artworks: ArtworkStore::new(directory.path().join("artworks")),
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
        .with_artworks(stores.artworks.clone())
    }

    /// A fresh one-candidate, low-effort artwork session, still in
    /// intake.
    async fn artwork_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("report", "Launch", "A launch cover.")
                    .with_kind(ArtifactKind::Artwork)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// An artwork session past its setup card: the app's own questions
    /// were asked, so the next planner turn is free to write.
    async fn set_up_artwork_session(sessions: &SessionStore) {
        artwork_session(sessions).await;
        sessions
            .apply("report", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn artwork_system_prompt_carries_artwork_rules_the_schema_and_the_example() {
        let prompt = artwork_system_prompt();
        assert!(prompt.contains("judged at a glance"));
        assert!(prompt.contains("1280 by 720 px for thumbnail"));
        assert!(prompt.contains("\"covers\""));
        assert!(prompt.contains("\"size\""));
        assert!(prompt.contains("Swift Design launch thumbnails"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
        assert!(!prompt.contains("\"slides\""));
    }

    #[test]
    fn partial_artwork_returns_complete_covers_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"size":"banner","covers":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let artwork = partial_artwork(text).unwrap();
        assert_eq!(artwork.covers.len(), 1);
        assert_eq!(artwork.size, design_model::CoverSize::Banner);
        assert!(partial_artwork("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_covers_reject_a_short_reply() {
        let mut preview = sample_artwork();
        preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        assert!(apply_artwork_continuation(&preview, "{\"covers\":[]}").is_err());
        let patch = r#"{"covers":[{"index":2,"insert":true,"cover":{"html":"<h2>C</h2>"}}]}"#;
        let continued = apply_artwork_continuation(&preview, patch).unwrap();
        assert_eq!(continued.covers.len(), 3);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_covers(2, patch).unwrap().len(), 1);
        assert!(continuation_covers(2, "{\"slides\":[]}").is_err());
        assert!(parse_artwork("no json").is_err());
    }

    #[test]
    fn shown_artworks_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_artwork();
        preview.outline = (1..=4).map(|number| format!("Cover {number}")).collect();
        let chunks = [
            ContinueChunk { first: 2, count: 1 },
            ContinueChunk { first: 3, count: 1 },
        ];
        let board = vec![Vec::new(), vec![placeholder_cover("x")]];
        let shown = shown_artwork(&preview, &chunks, &board);
        assert_eq!(shown.covers.len(), 4);
        assert!(shown.covers[2].html.contains("Cover 3"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_artwork_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        artwork_session(&stores.sessions).await;
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
    async fn a_valid_artwork_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_ARTWORK);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_artwork_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .artworks
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
        assert!(planner.contains("You plan cover art"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("cover art as JSON artworks"));
        assert!(request.contains("Build an artwork"));
        let runs = stores.sessions.runs("report").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_artwork_open_patches_that_artwork() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"covers":[{"index":0,"cover":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .artworks
            .save("report-candidate-1", &sample_artwork())
            .await
            .unwrap();
        artwork_session(&stores.sessions).await;
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
            .artworks
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.covers[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("report").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing artwork session with `count` saved candidates.
    async fn reviewing_artwork_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .artworks
                .save(&format!("report-candidate-{number}"), &sample_artwork())
                .await
                .unwrap();
        }
        set_up_artwork_session(&stores.sessions).await;
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
    async fn a_regenerated_cover_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its cover itself.
        server.push_text(
            r#"{"covers":[{"index":0,"cover":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut artwork = sample_artwork();
        artwork.covers[0].html = "<h1>Old title markup</h1>".to_owned();
        stores
            .artworks
            .save("report-candidate-1", &artwork)
            .await
            .unwrap();
        reviewing_artwork_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::regenerate_request(
                    "[cover 1] Write this cover anew.",
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
        assert!(text.contains("Write cover 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .artworks
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.covers[0].html.contains("Fresh"));
        assert_eq!(edited.covers.len(), artwork.covers.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_artworks_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_ARTWORK);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"covers":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_artwork_session_with(&stores, 2).await;
        let pinned = vec![
            "report-candidate-1".to_owned(),
            "report-candidate-2".to_owned(),
        ];
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::user(
                    "[candidate 1] [candidate 2] Front from 1, back from 2.",
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
                .artworks
                .load("report-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one artwork"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_artwork_session_without_a_artwork_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_ARTWORK);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_artwork_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("report", silent_log()).await.unwrap_err();
        assert!(error.contains("no artwork store"));
    }
}
