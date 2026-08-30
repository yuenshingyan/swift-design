//! The deck half of the built-in generation engine.
//!
//! A deck session runs the same loop as a demo session: read the brief,
//! ask the model for each candidate, validate, feed every validation
//! error back for a fix round, polish, and save. This module holds what
//! differs for decks: the deck prompts, the deck patch, the deck store,
//! and slide-typed continuation. The fix-round loop, the attachments,
//! the progress sinks, and the concept planning come from
//! `generation.rs`.

use std::sync::Arc;

use design_model::{Deck, Slide};

use crate::candidates::{candidate_id, next_candidate_number};
use crate::concepts::{Concept, concept_note};
use crate::decks::{DeckStore, PENDING_SLIDE_CLASS, is_pending_slide};
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
use crate::instructions::DECK_RULES;
use crate::model_client::LogSink;
use crate::request::{SessionRequest, request_input};

/// The slides each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type DeckChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Slide>>>>;

/// What one deck candidate call needs.
struct DeckCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` slides plus the outline.
    preview_slides: Option<usize>,
    /// The id the candidate is saved under.
    deck_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

impl GenerationEngine {
    /// The deck store, or the failure a deck run reports without one.
    fn deck_store(&self) -> Result<&DeckStore, GenerationStop> {
        self.decks.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no deck store: deck sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview decks the latest user turn asked to continue: every
    /// deck named by a trailing continue request that still is a
    /// preview. Pressing Finish on several candidates continues them
    /// all.
    pub(crate) async fn continue_deck_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(decks) = &self.decks else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for deck_id in crate::generation::trailing_continue_ids(&messages) {
            // A deck that is no longer a preview was finished already,
            // by this run or an earlier one.
            if let Ok(Some(deck)) = decks.load(&deck_id).await
                && deck.is_preview()
            {
                previews.push(deck_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for a deck session and returns the outcome.
    pub(crate) async fn execute_deck(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => self.generate_deck_candidates(client, context, log).await,
            GenerationTask::Edit {
                designs,
                instruction,
            } => {
                let order = EditOrder {
                    artifact_ids: &designs,
                    instruction: &instruction,
                    is_fresh: false,
                };
                let design_ids = self.edit_decks(client, context, &order, log).await?;
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
                let design_ids = self.edit_decks(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let deck_id = self
                    .merge_decks(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![deck_id],
                })
            }
            GenerationTask::Continue(deck_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, deck_ids, log)
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
                        "no deck was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one deck per requested variation. Returns the ids.
    async fn generate_deck_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let decks = self.deck_store()?;
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
        let first_number = match decks.list().await {
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
                let request = DeckCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_slides: context.preview_screens(),
                    deck_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_deck_candidate(&client, &request, &attachments, &share, &log)
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
            if matches!(decks.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no deck candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one deck candidate, repairs it through fix
    /// rounds until it validates, and polishes it. The deck is saved
    /// under `request.deck_id` while it streams in, when the draft
    /// validates, and once more after the polish.
    async fn generate_deck_candidate(
        &self,
        client: &reqwest::Client,
        request: &DeckCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Deck, GenerationStop> {
        let decks = self.deck_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": deck_system_prompt() }),
            self.user_message(&deck_candidate_prompt(request), attachments),
        ];
        let saver = DeckLiveSaver::new(decks, &self.notifier, &request.deck_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_deck),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(deck) = partial_deck(text) {
                    let rank = deck.slides.len();
                    live_saver.offer(deck, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.slides.len());
        let polished = self
            .polish_deck(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        decks
            .clear_user_paths(&request.deck_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new deck candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_decks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let decks = self.deck_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let deck = decks
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("deck `{id}` does not exist")))?;
            loaded.push((id.as_str(), deck));
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
        let rows = decks
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let deck_id = candidate_id(base, number);
        log(&format!("merging {} into {deck_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&deck_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = DeckCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_slides: None,
            deck_id: deck_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_deck_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {deck_id}"));
        Ok(deck_id)
    }

    /// Applies a critique to one chosen deck: the model rewrites the deck
    /// against the brief and the critique as a patch, the result is
    /// validated, polished at high effort, and saved under the same id.
    /// Applies `instruction` to each deck in turn and returns the ones
    /// it saved. One failure is logged and the rest still run; the turn
    /// fails only when every edit failed.
    async fn edit_decks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for deck_id in order.artifact_ids {
            match self.edit_deck(client, context, deck_id, order, log).await {
                Ok(()) => saved.push(deck_id.clone()),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {deck_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_deck(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        deck_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<(), GenerationStop> {
        let decks = self.deck_store()?;
        let deck = decks
            .load(deck_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| GenerationStop::Failed(format!("deck `{deck_id}` does not exist")))?;
        let instruction = order.instruction;
        let label = format!("edit {deck_id}");
        // A change that names slides is about those slides: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named slides without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "slide")
            .into_iter()
            .filter(|index| *index < deck.slides.len())
            .collect();
        let measured = crate::deck_polish::dom_findings(&deck, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "slides", &indexes);
        let total = deck.slides.len();
        let (deck_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&deck), String::new())
        } else if order.is_fresh {
            (
                focused_deck_json(&deck, &indexes, true),
                fresh_note("slide", "slides", &indexes, total),
            )
        } else {
            (
                focused_deck_json(&deck, &indexes, false),
                focus_note("slide", "slides", &indexes, total),
            )
        };
        let deck_json = deck_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &deck_json,
            note: &note,
            findings: &findings,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": deck_system_prompt() }),
            self.user_message(&deck_edit_prompt(&context.request, &input), &attachments),
        ];
        let original = deck.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| {
                crate::deck_patch::apply_patch(&original, crate::deck_patch::parse_patch(content)?)
            }),
            progress: self.shared_progress(&[deck_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
        // A fix can make a new problem. The touched slides are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&deck.slides, &edited.slides, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_deck = self
            .fix_edited_deck(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        decks
            .save(deck_id, &final_deck)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {deck_id}: saved"));
        Ok(())
    }

    /// Writes the remaining slides of the preview deck `deck_id` in
    /// chunks. The deck is saved after every chunk, so the canvas shows
    /// it grow, then polished once it is complete. Returns how many
    /// slides were added; 0 when the deck is complete already.
    pub(crate) async fn continue_deck(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        deck_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let decks = self.deck_store().map_err(stop_to_string)?;
        let mut deck = decks
            .load(deck_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("deck `{deck_id}` does not exist"))?;
        // A run that stopped may have left placeholder slides behind.
        deck.slides.retain(|slide| !is_pending_slide(slide));
        if !deck.is_preview() {
            log(&format!("continue {deck_id}: the deck is complete already"));
            return Ok(0);
        }
        let label = format!("continue {deck_id}");
        let start = deck.slides.len();
        let planned = deck.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} slides written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = DeckLiveSaver::new(decks, &self.notifier, deck_id);
        let board = self
            .write_deck_chunks(
                client,
                context,
                &deck,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = deck.clone();
        if let Ok(board) = board.lock() {
            for slides in board.iter() {
                continued.slides.extend(slides.iter().cloned());
            }
        }
        let added = continued.slides.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the deck stays continuable.
            if let Err(error) = saver.finish(&deck).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added a slide"));
        }
        // A failed chunk leaves the deck continuable: the outline stays
        // until every title has a slide.
        if continued.slides.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_deck),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_deck = self
            .polish_deck(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_deck).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new slides"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_deck_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Deck,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &DeckLiveSaver,
        log: &LogSink,
    ) -> DeckChunkBoard {
        let start = preview.slides.len();
        let planned = preview.outline.len();
        let board: DeckChunkBoard = Arc::new(std::sync::Mutex::new(
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
                saver.offer(shown_deck(&preview, &chunks, &board), written);
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
                    .write_deck_chunk(
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
    /// showing the deck grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_deck_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Deck,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &DeckChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let deck_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": deck_system_prompt() }),
            self.user_message(
                &deck_continue_prompt(&context.request, preview, &deck_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.slides.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_deck_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let slides = partial_continuation_slides(written, text);
                if let Ok(mut board) = live_board.lock()
                    && slides.len() > board[position].len()
                {
                    board[position] = slides;
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
        let slides: Vec<Slide> = continued.slides[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = slides;
        }
        show();
        Ok(())
    }

    /// Reviews a valid deck as a presentation designer, one round per
    /// effort level. An improved deck that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_deck(
        &self,
        client: &reqwest::Client,
        mut deck: Deck,
        context: &ArtifactRequest<'_, Deck>,
        log: &LogSink,
    ) -> Result<Deck, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(deck);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = deck.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::deck_polish::dom_findings(&deck, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = deck.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a deck that is already good.
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
            let images = self.slide_images(&deck, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} slide images)",
                findings.len(),
                images.len()
            ));
            let deck_json = serde_json::to_string(&deck).map_err(|error| error.to_string())?;
            let prompt = crate::deck_polish::polish_prompt(&deck_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": deck_system_prompt() }),
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
            let improved = crate::deck_patch::parse_patch(&content)
                .and_then(|patch| crate::deck_patch::apply_patch(&deck, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => deck = improved,
                Ok(_) => log(&format!(
                    "{label}: polished deck failed validation; keeping the previous version"
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

    /// Measures the touched slides of an edited deck and asks the model
    /// to fix what Chrome finds, round after round: until the slides
    /// measure clean, a round does not help, or the effort's round limit
    /// runs out. Returns the best version measured.
    async fn fix_edited_deck(
        &self,
        client: &reqwest::Client,
        mut deck: Deck,
        fix: &EditFix<'_, Deck>,
        log: &LogSink,
    ) -> Result<Deck, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(deck);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = deck.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::deck_polish::dom_findings(&deck, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "slides", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = deck.clone();
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
                "{label}: fix round {round} of at most {limit} ({} findings on the touched slides)",
                findings.len()
            ));
            let deck_json =
                focused_deck_json(&deck, &fix.indexes, false).map_err(|error| error.to_string())?;
            let note = focus_note("slide", "slides", &fix.indexes, deck.slides.len());
            let instruction = fix_instruction("slides");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &deck_json,
                note: &note,
                findings: &findings,
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": deck_system_prompt() }),
                serde_json::json!({ "role": "user", "content": deck_edit_prompt(fix.request, &input) }),
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
            let improved = crate::deck_patch::parse_patch(&content)
                .and_then(|patch| crate::deck_patch::apply_patch(&deck, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => deck = improved,
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

    /// PNG screenshots of the deck's slides for the polish pass, at most
    /// `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images or
    /// no Chrome is installed.
    async fn slide_images(&self, deck: &Deck, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for slide images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = deck
            .slides
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let deck = deck.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_slide(&deck, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: slide {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}

/// Saves a deck while it streams in, so the canvas shows the slides
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct DeckLiveSaver {
    decks: DeckStore,
    notifier: ChangeNotifier,
    deck_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final deck. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl DeckLiveSaver {
    fn new(decks: &DeckStore, notifier: &ChangeNotifier, deck_id: &str) -> Self {
        Self {
            decks: decks.clone(),
            notifier: notifier.clone(),
            deck_id: deck_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial deck. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, deck: Deck, rank: usize) {
        if !deck.validate().is_empty() {
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
            if saver.decks.save(&saver.deck_id, &deck).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final deck after every partial save landed.
    async fn finish(&self, deck: &Deck) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.decks
            .save(&self.deck_id, deck)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The deck a streaming reply has written so far: everything before the
/// slides plus every complete slide. `None` until the first slide is
/// complete, or when the text before the slides is not a deck.
fn partial_deck(text: &str) -> Option<Deck> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "slides")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new slides a streaming continuation reply has completed so far.
fn partial_continuation_slides(written: usize, text: &str) -> Vec<Slide> {
    let Some((_, items)) = complete_array_items(text, "slides") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"slides\":[{}]}}", items.join(","));
    continuation_slides(written, &json).unwrap_or_default()
}

/// The deck to show while the chunks run: the preview, then every chunk
/// up to the last one that has slides, with placeholders for the slides
/// an earlier chunk still owes.
fn shown_deck(preview: &Deck, chunks: &[ContinueChunk], board: &[Vec<Slide>]) -> Deck {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|slides| !slides.is_empty()) else {
        return shown;
    };
    for (chunk, slides) in chunks.iter().zip(board).take(last) {
        shown.slides.extend(slides.iter().cloned());
        for offset in slides.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.slides.push(placeholder_slide(title));
        }
    }
    shown.slides.extend(board[last].iter().cloned());
    shown
}

/// A slide that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a deck that does not.
fn placeholder_slide(title: &str) -> Slide {
    Slide {
        html: format!(
            "<div class=\"{PENDING_SLIDE_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
             <h2 class=\"pending-title\">{}</h2></div>",
            crate::render::escape_html(title),
        ),
        css: Some(
            ".pending { display: flex; flex-direction: column; align-items: center; \
             justify-content: center; height: 100%; gap: 24px; opacity: 0.55; }\n\
             .pending-label { margin: 0; font-size: 26px; letter-spacing: 0.3em; \
             text-transform: uppercase; color: var(--muted); }\n\
             .pending-title { margin: 0; max-width: 1400px; text-align: center; \
             font-size: 64px; color: var(--text); }"
                .to_owned(),
        ),
        notes: None,
    }
}

/// The new slides in a continuation reply, in order. Accepts a patch
/// (the slides of its operations at or past the existing slides) and,
/// as a fallback, a whole deck (its slides past the existing ones).
fn continuation_slides(written: usize, content: &str) -> Result<Vec<Slide>, String> {
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
        .get("slides")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no slides array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("slide").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("slide"))
            .filter(|slide| slide.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, slide)| {
            serde_json::from_value::<Slide>(slide.clone()).map_err(|error| {
                format!(
                    "new slide {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new slides to the deck in progress. The outline
/// stays until every title has a slide, so a short reply leaves the deck
/// continuable.
fn apply_deck_continuation(original: &Deck, content: &str) -> Result<Deck, String> {
    let new_slides = continuation_slides(original.slides.len(), content)?;
    if new_slides.is_empty() {
        return Err(
            "the reply adds no slides: reply with a patch of inserts, one per new slide".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.slides.extend(new_slides);
    if continued.slides.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The deck system prompt: role, deck rules, the deck schema, the
/// clarification protocol, and one example deck.
fn deck_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Deck)).unwrap_or_default();
    format!(
        "You build slide decks as JSON documents. Each slide is one HTML fragment plus its own CSS, \
         for a 1920 by 1080 px canvas.\n\
         Follow these rules:\n{rules}\n\
         The deck must conform to this JSON Schema:\n{schema}\n\
         Example deck:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one deck JSON document. No prose, no code fences.",
        rules = DECK_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-deck.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` slides and
/// the full outline.
fn deck_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} slides of the deck, in order, starting with the \
         title slide. Put the slide titles of the complete deck in `outline`, in order, every \
         slide title of the complete deck. The app asks you for the remaining slides later. \
         Make these {count} slides show the theme, the layout language, and the text density \
         of the whole deck.\n"
    )
}

/// The prompt line that holds the deck to the length the user asked
/// for. Empty when the user set no length. A preview writes fewer
/// slides than the length, so the count goes to the outline instead.
/// The prompt line for the app's scenario choice. Empty when the
/// agent decides.
fn scenario_note(scenario: Option<&str>) -> String {
    match scenario.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!(
            "The deck is for this scenario: {name}. Use the vocabulary, the examples, and the tone that fit it.\n"
        ),
        None => String::new(),
    }
}

fn slide_count_note(slide_count: Option<u32>, preview_slides: Option<usize>) -> String {
    let Some(count) = slide_count else {
        return String::new();
    };
    match preview_slides {
        Some(_) => {
            format!("The user asked for {count} slides. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} slides. Write exactly {count} slides.\n"),
    }
}

/// The user prompt for one deck candidate: the request and the answers
/// are authoritative, plus the template, preview, concept, and effort
/// notes.
fn deck_candidate_prompt(request: &DeckCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a deck for this request. The request and the answers are authoritative; do not \
         override an answer.\n{}\n",
        request_input(&request.context.request)
    );
    if let Some(template) = request.template {
        prompt.push_str(&template_note(template));
        prompt.push_str(
            "The template screens are slides of another deck. Use them for the look only.\n",
        );
    }
    if let Some(count) = request.preview_slides {
        prompt.push_str(&deck_preview_note(count));
    }
    prompt.push_str(&slide_count_note(
        options.slide_count,
        request.preview_slides,
    ));
    prompt.push_str(&scenario_note(options.scenario.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("deck", merge));
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
        "low" => prompt.push_str("Keep the deck concise: fewer slides, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the deck JSON.");
    prompt
}

/// The user prompt for a deck edit: the deck as it is, the request, and
/// the change the user asked for.
fn deck_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the deck to change:\n{deck_json}\n{note}\
         The deck is for this request:\n{request}\n\
         Apply this change: {critique}\n{findings}\
         A reference like [slide 3, node 0/1 <h2.title>: What Swift Design does] names a slide \
         (1-based) and one element in that slide's html by its index path from the slide root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [slide 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one slide the same way, without their text. A reference like [slide 3] \
         names the slide alone: the change is about that slide. Change only what the critique asks for. Keep every other slide and \
         value as it is. Return every changed slide complete: html, css, and notes.\n{format}",
        deck_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        findings = findings_note(input.findings),
        format = crate::deck_patch::PATCH_FORMAT
    )
}

/// The deck as a focused edit sees it: the title, the theme, the slide
/// count, and only the slides at `indexes`, each with its index.
fn focused_deck_json(
    deck: &Deck,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let slides: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            deck.slides.get(*index).map(|slide| {
                let slide = if is_fresh {
                    fresh_slide(slide)
                } else {
                    slide.clone()
                };
                serde_json::json!({ "index": index, "slide": slide })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": deck.title,
        "theme": deck.theme,
        "slide_count": deck.slides.len(),
        "slides": slides,
    }))
}

/// The slide as a regenerate shows it: its name and notes, without its
/// markup, so the model writes it anew instead of tweaking it.
fn fresh_slide(slide: &Slide) -> Slide {
    Slide {
        html: String::new(),
        css: None,
        ..slide.clone()
    }
}

/// The user prompt for one deck continuation chunk: the preview deck and
/// the chunk's slides to add, as a patch of inserts.
fn deck_continue_prompt(
    request: &SessionRequest,
    deck: &Deck,
    deck_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = deck.slides.len();
    let planned = deck.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = deck
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a deck in progress: its theme, its first {written} slides, and `outline`, the \
         slide titles of the complete deck:\n{deck_json}\n\
         The deck is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} slides: outline titles {} to {last} of {planned}, in order, one slide per \
         title:\n{}\n\
         Keep the theme. Match the existing slides in CSS style, font sizes, spacing, colors, \
         and visual language, so the deck reads as one deck. Do not change or repeat the \
         existing slides.\n",
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
        "Reply with only a JSON patch that appends the new slides, not the whole deck:\n\
         {{\"slides\":[{{\"index\":{written},\"insert\":true,\"slide\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new slide index {written} and insert true, in presentation order. Each slide \
         carries html, css, and notes. Omit title, theme, outline, and the existing slides."
    ));
    prompt
}

/// Extracts and parses the deck JSON from a model reply.
fn parse_deck(content: &str) -> Result<Deck, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid deck: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        apply_deck_continuation, continuation_slides, deck_edit_prompt, deck_system_prompt,
        focused_deck_json, parse_deck, partial_deck, placeholder_slide, scenario_note, shown_deck,
        slide_count_note,
    };
    use crate::decks::DeckStore;
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{FakeModelServer, SAMPLE_DECK, low_effort_options, sample_deck};

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_deck_edit_shows_only_the_named_slides_and_their_findings() {
        let deck = crate::test_support::sample_deck();
        let focused = focused_deck_json(&deck, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["slide_count"], deck.slides.len());
        assert_eq!(value["slides"].as_array().unwrap().len(), 1);
        assert_eq!(value["slides"][0]["index"], 1);
        let request = SessionRequest {
            request: "A talk.".to_owned(),
            kind: ArtifactKind::Deck,
            answers: Vec::new(),
            options: crate::test_support::low_effort_options(),
        };
        let findings = vec!["slides[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[slide 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only slide 2 is shown.\n",
            findings: &findings,
        };
        let prompt = deck_edit_prompt(&request, &input);
        assert!(prompt.contains("Only slide 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- slides[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [slide 2, node 0/2 <p>: x] Fix the overflow."));
    }

    #[test]
    fn the_slide_count_holds_the_deck_to_the_length_the_user_asked_for() {
        assert_eq!(scenario_note(None), "");
        assert_eq!(scenario_note(Some(" ")), "");
        assert!(scenario_note(Some("Training")).contains("scenario: Training"));
        assert_eq!(slide_count_note(None, None), "");
        assert_eq!(slide_count_note(None, Some(3)), "");
        assert_eq!(
            slide_count_note(Some(12), None),
            "The user asked for 12 slides. Write exactly 12 slides.\n"
        );
        // A preview writes three slides, so the length goes to the outline.
        assert_eq!(
            slide_count_note(Some(12), Some(3)),
            "The user asked for 12 slides. Put exactly 12 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        decks: DeckStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            decks: DeckStore::new(directory.path().join("decks")),
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
        .with_decks(stores.decks.clone())
    }

    /// A fresh one-candidate, low-effort deck session, still in intake.
    async fn deck_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("talk", "Talk", "A talk.")
                    .with_kind(ArtifactKind::Deck)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// A deck session past its setup card: the app's own questions were
    /// asked, so the next planner turn is free to write.
    async fn set_up_deck_session(sessions: &SessionStore) {
        deck_session(sessions).await;
        sessions
            .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn deck_system_prompt_carries_deck_rules_the_schema_and_the_example() {
        let prompt = deck_system_prompt();
        assert!(prompt.contains("slide decks"));
        assert!(prompt.contains("1920 by 1080 px"));
        assert!(prompt.contains("\"slides\""));
        assert!(prompt.contains("Swift Design Deck Overview"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
    }

    #[test]
    fn partial_deck_returns_complete_slides_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#101418","text":"#f5f5f5","accent":"#4f8cff","muted":"#8a94a6"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"slides":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let deck = partial_deck(text).unwrap();
        assert_eq!(deck.slides.len(), 1);
        assert!(partial_deck("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_slides_reject_a_short_reply() {
        let mut preview = sample_deck();
        preview.outline = vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
        ];
        assert!(apply_deck_continuation(&preview, "{\"slides\":[]}").is_err());
        let patch = r#"{"slides":[{"index":3,"insert":true,"slide":{"html":"<h2>D</h2>"}}]}"#;
        let continued = apply_deck_continuation(&preview, patch).unwrap();
        assert_eq!(continued.slides.len(), 4);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_slides(3, patch).unwrap().len(), 1);
        assert!(continuation_slides(3, "{\"screens\":[]}").is_err());
        assert!(parse_deck("no json").is_err());
    }

    #[test]
    fn shown_decks_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_deck();
        preview.outline = (1..=7).map(|number| format!("Slide {number}")).collect();
        let chunks = [
            ContinueChunk { first: 3, count: 2 },
            ContinueChunk { first: 5, count: 2 },
        ];
        let board = vec![
            Vec::new(),
            vec![placeholder_slide("x"), placeholder_slide("y")],
        ];
        let shown = shown_deck(&preview, &chunks, &board);
        assert_eq!(shown.slides.len(), 7);
        assert!(shown.slides[3].html.contains("Slide 4"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_deck_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        deck_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("talk", silent_log())
            .await
            .unwrap();
        // The deck's own questions live on this card: the scenario, the
        // length, the density, the evidence, the candidates, and the
        // variety. Without the card the deck flow asked nothing at all.
        assert!(matches!(
            outcome,
            GenerationOutcome::NeedsClarification { question_set: 1 }
        ));
        let set = stores
            .sessions
            .read_question_set("talk", 1)
            .await
            .unwrap()
            .unwrap();
        assert!(set.questions.is_empty());
        assert!(set.can_proceed_with_assumptions);
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_valid_deck_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DECK);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_deck_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .decks
                .load("talk-candidate-1")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            stores
                .designs
                .load("talk-candidate-1")
                .await
                .unwrap()
                .is_none()
        );
        let planner = server.requests()[0].to_string();
        assert!(planner.contains("You plan slide decks"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("slide decks"));
        assert!(request.contains("Build a deck"));
        let runs = stores.sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_deck_open_patches_that_deck() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"slides":[{"index":0,"slide":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:96px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .decks
            .save("talk-candidate-1", &sample_deck())
            .await
            .unwrap();
        deck_session(&stores.sessions).await;
        stores
            .sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        stores
            .sessions
            .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
        stores
            .sessions
            .append_message(
                "talk",
                ChatMessage::user("Tighten the title.", Some("talk-candidate-1")),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let edited = stores
            .decks
            .load("talk-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.slides[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("talk").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing deck session with `count` saved candidates.
    async fn reviewing_deck_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .decks
                .save(&format!("talk-candidate-{number}"), &sample_deck())
                .await
                .unwrap();
        }
        set_up_deck_session(&stores.sessions).await;
        stores
            .sessions
            .apply("talk", design_model::WorkflowEvent::GenerationStarted)
            .await
            .unwrap();
        stores
            .sessions
            .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_regenerated_slide_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its slide itself.
        server.push_text(
            r#"{"slides":[{"index":0,"slide":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:96px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut deck = sample_deck();
        deck.slides[0].html = "<h1>Old title markup</h1>".to_owned();
        stores.decks.save("talk-candidate-1", &deck).await.unwrap();
        reviewing_deck_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "talk",
                ChatMessage::regenerate_request(
                    "[slide 1] Write this slide anew.",
                    "talk-candidate-1",
                ),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let text = server.requests()[0].to_string();
        assert!(text.contains("Write slide 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .decks
            .load("talk-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.slides[0].html.contains("Fresh"));
        assert_eq!(edited.slides.len(), deck.slides.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_decks_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_DECK);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"slides":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_deck_session_with(&stores, 2).await;
        let pinned = vec!["talk-candidate-1".to_owned(), "talk-candidate-2".to_owned()];
        stores
            .sessions
            .append_message(
                "talk",
                ChatMessage::user(
                    "[candidate 1] [candidate 2] Opening from 1, close from 2.",
                    None,
                )
                .with_pinned(pinned),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("talk", silent_log())
            .await
            .unwrap();
        let GenerationOutcome::Wrote { design_ids } = outcome else {
            panic!("expected a write");
        };
        assert_eq!(design_ids, vec!["talk-candidate-3".to_owned()]);
        assert!(
            stores
                .decks
                .load("talk-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one deck"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_deck_session_without_a_deck_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_DECK);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_deck_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("talk", silent_log()).await.unwrap_err();
        assert!(error.contains("no deck store"));
    }
}
