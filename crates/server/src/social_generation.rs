//! The social half of the built-in generation engine.
//!
//! A social session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for socials: the social prompts, the social
//! patch, the social store, and frame-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Format, Frame, Social};

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
use crate::instructions::SOCIAL_RULES;
use crate::model_client::LogSink;
use crate::request::{SessionRequest, request_input};
use crate::socials::{PENDING_FRAME_CLASS, SocialStore, is_pending_frame};

/// The frames each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type SocialChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Frame>>>>;

/// What one social candidate call needs.
struct SocialCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` frames plus the outline.
    preview_frames: Option<usize>,
    /// The id the candidate is saved under.
    social_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

impl GenerationEngine {
    /// The social store, or the failure a social run reports
    /// without one.
    fn social_store(&self) -> Result<&SocialStore, GenerationStop> {
        self.socials.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no social store: social sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview socials the latest user turn asked to continue:
    /// every social named by a trailing continue request that still
    /// is a preview.
    pub(crate) async fn continue_social_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(socials) = &self.socials else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for social_id in crate::generation::trailing_continue_ids(&messages) {
            // A social that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(social)) = socials.load(&social_id).await
                && social.is_preview()
            {
                previews.push(social_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for a social session and returns the
    /// outcome.
    pub(crate) async fn execute_social(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => {
                self.generate_social_candidates(client, context, log).await
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
                let design_ids = self.edit_socials(client, context, &order, log).await?;
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
                let design_ids = self.edit_socials(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let social_id = self
                    .merge_socials(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![social_id],
                })
            }
            GenerationTask::Continue(social_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, social_ids, log)
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
                        "no social was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one social per requested variation. Returns the ids.
    async fn generate_social_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let socials = self.social_store()?;
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
        let first_number = match socials.list().await {
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
                let request = SocialCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_frames: context.preview_screens(),
                    social_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_social_candidate(&client, &request, &attachments, &share, &log)
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
            if matches!(socials.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no social candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one social candidate, repairs it through
    /// fix rounds until it validates, and polishes it. The social is
    /// saved under `request.social_id` while it streams in, when the
    /// draft validates, and once more after the polish.
    async fn generate_social_candidate(
        &self,
        client: &reqwest::Client,
        request: &SocialCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Social, GenerationStop> {
        let socials = self.social_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": social_system_prompt() }),
            self.user_message(&social_candidate_prompt(request), attachments),
        ];
        let saver = SocialLiveSaver::new(socials, &self.notifier, &request.social_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_social),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(social) = partial_social(text) {
                    let rank = social.frames.len();
                    live_saver.offer(social, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.frames.len());
        let polished = self
            .polish_social(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        socials
            .clear_user_paths(&request.social_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new social candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_socials(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let socials = self.social_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let social = socials
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("social `{id}` does not exist")))?;
            loaded.push((id.as_str(), social));
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
        let rows = socials
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let social_id = candidate_id(base, number);
        log(&format!("merging {} into {social_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&social_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = SocialCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_frames: None,
            social_id: social_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_social_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {social_id}"));
        Ok(social_id)
    }

    /// Applies `instruction` to each social in turn and returns the
    /// ones it saved. One failure is logged and the rest still run; the
    /// turn fails only when every edit failed.
    async fn edit_socials(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for social_id in order.artifact_ids {
            match self
                .edit_social(client, context, social_id, order, log)
                .await
            {
                Ok(()) => saved.push(social_id.clone()),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {social_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_social(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        social_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<(), GenerationStop> {
        let socials = self.social_store()?;
        let social = socials
            .load(social_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("social `{social_id}` does not exist"))
            })?;
        let instruction = order.instruction;
        let label = format!("edit {social_id}");
        // A change that names frames is about those frames: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named frames without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "frame")
            .into_iter()
            .filter(|index| *index < social.frames.len())
            .collect();
        let measured =
            crate::social_polish::dom_findings(&social, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "frames", &indexes);
        let total = social.frames.len();
        let (social_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&social), String::new())
        } else if order.is_fresh {
            (
                focused_social_json(&social, &indexes, true),
                fresh_note("frame", "frames", &indexes, total),
            )
        } else {
            (
                focused_social_json(&social, &indexes, false),
                focus_note("frame", "frames", &indexes, total),
            )
        };
        let social_json = social_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &social_json,
            note: &note,
            findings: &findings,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": social_system_prompt() }),
            self.user_message(&social_edit_prompt(&context.request, &input), &attachments),
        ];
        let original = social.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| {
                crate::social_patch::apply_patch(
                    &original,
                    crate::social_patch::parse_patch(content)?,
                )
            }),
            progress: self.shared_progress(&[social_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
        // A fix can make a new problem. The touched frames are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&social.frames, &edited.frames, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_social = self
            .fix_edited_social(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        socials
            .save(social_id, &final_social)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {social_id}: saved"));
        Ok(())
    }

    /// Writes the remaining frames of the preview social `social_id`
    /// in chunks. The social is saved after every chunk, so the canvas
    /// shows it grow, then polished once it is complete. Returns how
    /// many frames were added; 0 when the social is complete already.
    pub(crate) async fn continue_social(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        social_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let socials = self.social_store().map_err(stop_to_string)?;
        let mut social = socials
            .load(social_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("social `{social_id}` does not exist"))?;
        // A run that stopped may have left placeholder frames behind.
        social.frames.retain(|frame| !is_pending_frame(frame));
        if !social.is_preview() {
            log(&format!(
                "continue {social_id}: the social is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {social_id}");
        let start = social.frames.len();
        let planned = social.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} frames written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = SocialLiveSaver::new(socials, &self.notifier, social_id);
        let board = self
            .write_social_chunks(
                client,
                context,
                &social,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = social.clone();
        if let Ok(board) = board.lock() {
            for frames in board.iter() {
                continued.frames.extend(frames.iter().cloned());
            }
        }
        let added = continued.frames.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the social stays continuable.
            if let Err(error) = saver.finish(&social).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added a frame"));
        }
        // A failed chunk leaves the social continuable: the outline
        // stays until every title has a frame.
        if continued.frames.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_social),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_social = self
            .polish_social(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_social).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new frames"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_social_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Social,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &SocialLiveSaver,
        log: &LogSink,
    ) -> SocialChunkBoard {
        let start = preview.frames.len();
        let planned = preview.outline.len();
        let board: SocialChunkBoard = Arc::new(std::sync::Mutex::new(
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
                saver.offer(shown_social(&preview, &chunks, &board), written);
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
                    .write_social_chunk(
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
    /// showing the social grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_social_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Social,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &SocialChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let social_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": social_system_prompt() }),
            self.user_message(
                &social_continue_prompt(&context.request, preview, &social_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.frames.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_social_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let frames = partial_continuation_frames(written, text);
                if let Ok(mut board) = live_board.lock()
                    && frames.len() > board[position].len()
                {
                    board[position] = frames;
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
        let frames: Vec<Frame> = continued.frames[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = frames;
        }
        show();
        Ok(())
    }

    /// Reviews a valid social as a social media designer, one round per
    /// effort level. An improved social that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_social(
        &self,
        client: &reqwest::Client,
        mut social: Social,
        context: &ArtifactRequest<'_, Social>,
        log: &LogSink,
    ) -> Result<Social, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(social);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = social.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::social_polish::dom_findings(&social, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = social.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a social that is already good.
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
            let images = self.frame_images(&social, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} frame images)",
                findings.len(),
                images.len()
            ));
            let social_json = serde_json::to_string(&social).map_err(|error| error.to_string())?;
            let prompt = crate::social_polish::polish_prompt(&social_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": social_system_prompt() }),
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
            let improved = crate::social_patch::parse_patch(&content)
                .and_then(|patch| crate::social_patch::apply_patch(&social, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => social = improved,
                Ok(_) => log(&format!(
                    "{label}: polished social failed validation; keeping the previous version"
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

    /// Measures the touched frames of an edited social and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// frames measure clean, a round does not help, or the effort's round
    /// limit runs out. Returns the best version measured.
    async fn fix_edited_social(
        &self,
        client: &reqwest::Client,
        mut social: Social,
        fix: &EditFix<'_, Social>,
        log: &LogSink,
    ) -> Result<Social, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(social);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = social.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::social_polish::dom_findings(&social, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "frames", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = social.clone();
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
                "{label}: fix round {round} of at most {limit} ({} findings on the touched frames)",
                findings.len()
            ));
            let social_json = focused_social_json(&social, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("frame", "frames", &fix.indexes, social.frames.len());
            let instruction = fix_instruction("frames");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &social_json,
                note: &note,
                findings: &findings,
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": social_system_prompt() }),
                serde_json::json!({ "role": "user", "content": social_edit_prompt(fix.request, &input) }),
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
            let improved = crate::social_patch::parse_patch(&content)
                .and_then(|patch| crate::social_patch::apply_patch(&social, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => social = improved,
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

    /// PNG screenshots of the social's frames for the polish pass, at
    /// most `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images
    /// or no Chrome is installed.
    async fn frame_images(&self, social: &Social, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for frame images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = social
            .frames
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let social = social.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_frame(&social, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: frame {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}

/// Saves a social while it streams in, so the canvas shows the frames
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct SocialLiveSaver {
    socials: SocialStore,
    notifier: ChangeNotifier,
    social_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final social. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl SocialLiveSaver {
    fn new(socials: &SocialStore, notifier: &ChangeNotifier, social_id: &str) -> Self {
        Self {
            socials: socials.clone(),
            notifier: notifier.clone(),
            social_id: social_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial social. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, social: Social, rank: usize) {
        if !social.validate().is_empty() {
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
            if saver.socials.save(&saver.social_id, &social).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final social after every partial save landed.
    async fn finish(&self, social: &Social) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.socials
            .save(&self.social_id, social)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The social a streaming reply has written so far: everything before
/// the frames plus every complete frame. `None` until the first frame is
/// complete, or when the text before the frames is not a social.
fn partial_social(text: &str) -> Option<Social> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "frames")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new frames a streaming continuation reply has completed so far.
fn partial_continuation_frames(written: usize, text: &str) -> Vec<Frame> {
    let Some((_, items)) = complete_array_items(text, "frames") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"frames\":[{}]}}", items.join(","));
    continuation_frames(written, &json).unwrap_or_default()
}

/// The social to show while the chunks run: the preview, then every
/// chunk up to the last one that has frames, with placeholders for the
/// frames an earlier chunk still owes.
fn shown_social(preview: &Social, chunks: &[ContinueChunk], board: &[Vec<Frame>]) -> Social {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|frames| !frames.is_empty()) else {
        return shown;
    };
    for (chunk, frames) in chunks.iter().zip(board).take(last) {
        shown.frames.extend(frames.iter().cloned());
        for offset in frames.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.frames.push(placeholder_frame(title));
        }
    }
    shown.frames.extend(board[last].iter().cloned());
    shown
}

/// A frame that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a social that does
/// not.
fn placeholder_frame(title: &str) -> Frame {
    Frame {
        html: format!(
            "<div class=\"{PENDING_FRAME_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new frames in a continuation reply, in order. Accepts a patch
/// (the frames of its operations at or past the existing frames) and, as
/// a fallback, a whole social (its frames past the existing ones).
fn continuation_frames(written: usize, content: &str) -> Result<Vec<Frame>, String> {
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
        .get("frames")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no frames array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("frame").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("frame"))
            .filter(|frame| frame.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, frame)| {
            serde_json::from_value::<Frame>(frame.clone()).map_err(|error| {
                format!(
                    "new frame {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new frames to the social in progress. The
/// outline stays until every title has a frame, so a short reply leaves
/// the social continuable.
fn apply_social_continuation(original: &Social, content: &str) -> Result<Social, String> {
    let new_frames = continuation_frames(original.frames.len(), content)?;
    if new_frames.is_empty() {
        return Err(
            "the reply adds no frames: reply with a patch of inserts, one per new frame".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.frames.extend(new_frames);
    if continued.frames.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The social system prompt: role, social rules, the social
/// schema, the clarification protocol, and one example social.
fn social_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Social)).unwrap_or_default();
    format!(
        "You build social posts and carousels as JSON socials, for Instagram, LinkedIn, X, and Facebook. \
         Each frame is one HTML fragment plus its own CSS, for the px canvas of the social's format: \
         1080 by 1080 px for square, 1080 by 1350 px for portrait, 1080 by 1920 px for story, \
         1200 by 630 px for landscape. One frame is a single post. Two or more frames are a carousel.\n\
         Follow these rules:\n{rules}\n\
         The social must conform to this JSON Schema:\n{schema}\n\
         Example social:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one social JSON. No prose, no code fences.",
        rules = SOCIAL_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-social.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` frames and
/// the full outline.
fn social_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} frames of the social, in order, starting with \
         the first frame. Put the frame titles of the complete social in `outline`, in order, \
         every frame title of the complete social. The app asks you for the remaining frames \
         later. Make these {count} frames show the theme, the layout language, and the text \
         density of the whole social.\n"
    )
}

/// The prompt line for the app's format choice. Empty when the agent
/// decides, or when the user typed a format the JSON does not carry.
fn format_note(format: Option<&str>) -> String {
    match format.and_then(Format::from_name) {
        Some(format) => {
            let viewport = format.viewport();
            format!(
                "Lay the frames out on the {} format: {} by {} px. Set `format` to `{}`.\n",
                format.as_str(),
                viewport.width,
                viewport.height,
                format.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the social to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// frames than the length, so the count goes to the outline instead.
fn frame_count_note(frame_count: Option<u32>, preview_frames: Option<usize>) -> String {
    let Some(count) = frame_count else {
        return String::new();
    };
    match preview_frames {
        Some(_) => {
            format!("The user asked for {count} frames. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} frames. Write exactly {count} frames.\n"),
    }
}

/// The user prompt for one social candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
fn social_candidate_prompt(request: &SocialCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a social post or carousel for this request. The request and the answers are \
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
    if let Some(count) = request.preview_frames {
        prompt.push_str(&social_preview_note(count));
    }
    prompt.push_str(&frame_count_note(
        options.frame_count,
        request.preview_frames,
    ));
    prompt.push_str(&format_note(options.format.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("social", merge));
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
        "low" => prompt.push_str("Keep the social concise: fewer frames, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the social JSON.");
    prompt
}

/// The user prompt for a social edit: the social as it is, the
/// request, and the change the user asked for.
fn social_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the social to change:\n{social_json}\n{note}\
         The social is for this request:\n{request}\n\
         Apply this change: {critique}\n{findings}\
         A reference like [frame 3, node 0/1 <h2.title>: What changed] names a frame \
         (1-based) and one element in that frame's html by its index path from the frame root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [frame 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one frame the same way, without their text. A reference like [frame 3] \
         names the frame alone: the change is about that frame. Change only what the critique asks for. Keep every other frame and \
         value as it is. Return every changed frame complete: html, css, and notes.\n{format}",
        social_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        findings = findings_note(input.findings),
        format = crate::social_patch::PATCH_FORMAT
    )
}

/// The social as a focused edit sees it: the title, the theme, the
/// format, the frame count, and only the frames at `indexes`, each with
/// its index.
fn focused_social_json(
    social: &Social,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let frames: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            social.frames.get(*index).map(|frame| {
                let frame = if is_fresh {
                    fresh_frame(frame)
                } else {
                    frame.clone()
                };
                serde_json::json!({ "index": index, "frame": frame })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": social.title,
        "theme": social.theme,
        "format": social.format,
        "frame_count": social.frames.len(),
        "frames": frames,
    }))
}

/// The frame as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
fn fresh_frame(frame: &Frame) -> Frame {
    Frame {
        html: String::new(),
        css: None,
        ..frame.clone()
    }
}

/// The user prompt for one social continuation chunk: the preview
/// social and the chunk's frames to add, as a patch of inserts.
fn social_continue_prompt(
    request: &SessionRequest,
    social: &Social,
    social_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = social.frames.len();
    let planned = social.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = social
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a social in progress: its theme, its format, its first {written} frames, and \
         `outline`, the frame titles of the complete social:\n{social_json}\n\
         The social is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} frames: outline titles {} to {last} of {planned}, in order, one frame per \
         title:\n{}\n\
         Keep the theme. Match the existing frames in CSS style, font sizes, spacing, colors, \
         and visual language, so the social reads as one social. Do not change or repeat \
         the existing frames.\n",
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
        "Reply with only a JSON patch that appends the new frames, not the whole social:\n\
         {{\"frames\":[{{\"index\":{written},\"insert\":true,\"frame\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new frame index {written} and insert true, in reading order. Each frame \
         carries html, css, and notes. Omit title, theme, format, outline, and the existing frames."
    ));
    prompt
}

/// Extracts and parses the social JSON from a model reply.
fn parse_social(content: &str) -> Result<Social, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid social: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        apply_social_continuation, continuation_frames, focused_social_json, format_note,
        frame_count_note, parse_social, partial_social, placeholder_frame, shown_social,
        social_edit_prompt, social_system_prompt,
    };
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::socials::SocialStore;
    use crate::test_support::{FakeModelServer, SAMPLE_SOCIAL, low_effort_options, sample_social};

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_social_edit_shows_only_the_named_frames_and_their_findings() {
        let social = sample_social();
        let focused = focused_social_json(&social, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["frame_count"], social.frames.len());
        assert_eq!(value["format"], "portrait");
        assert_eq!(value["frames"].as_array().unwrap().len(), 1);
        assert_eq!(value["frames"][0]["index"], 1);
        let request = SessionRequest {
            request: "A launch carousel.".to_owned(),
            kind: ArtifactKind::Social,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["frames[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[frame 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only frame 2 is shown.\n",
            findings: &findings,
        };
        let prompt = social_edit_prompt(&request, &input);
        assert!(prompt.contains("Only frame 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- frames[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [frame 2, node 0/2 <p>: x] Fix the overflow."));
        assert!(!prompt.contains("slide"));
    }

    #[test]
    fn the_frame_count_and_the_format_hold_the_social_to_the_asked_shape() {
        assert_eq!(format_note(None), "");
        assert_eq!(format_note(Some("banner")), "");
        assert_eq!(
            format_note(Some("story")),
            "Lay the frames out on the story format: 1080 by 1920 px. Set `format` to `story`.\n"
        );
        assert_eq!(frame_count_note(None, None), "");
        assert_eq!(frame_count_note(None, Some(3)), "");
        assert_eq!(
            frame_count_note(Some(8), None),
            "The user asked for 8 frames. Write exactly 8 frames.\n"
        );
        // A preview writes three frames, so the length goes to the outline.
        assert_eq!(
            frame_count_note(Some(8), Some(3)),
            "The user asked for 8 frames. Put exactly 8 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        socials: SocialStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            socials: SocialStore::new(directory.path().join("socials")),
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
        .with_socials(stores.socials.clone())
    }

    /// A fresh one-candidate, low-effort social session, still in
    /// intake.
    async fn social_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("report", "Launch", "A launch carousel.")
                    .with_kind(ArtifactKind::Social)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// A social session past its setup card: the app's own questions
    /// were asked, so the next planner turn is free to write.
    async fn set_up_social_session(sessions: &SessionStore) {
        social_session(sessions).await;
        sessions
            .apply("report", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn social_system_prompt_carries_social_rules_the_schema_and_the_example() {
        let prompt = social_system_prompt();
        assert!(prompt.contains("social posts and carousels"));
        assert!(prompt.contains("1080 by 1350 px for portrait"));
        assert!(prompt.contains("\"frames\""));
        assert!(prompt.contains("\"format\""));
        assert!(prompt.contains("Swift Design launch carousel"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
        assert!(!prompt.contains("\"slides\""));
    }

    #[test]
    fn partial_social_returns_complete_frames_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"format":"story","frames":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let social = partial_social(text).unwrap();
        assert_eq!(social.frames.len(), 1);
        assert_eq!(social.format, design_model::Format::Story);
        assert!(partial_social("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_frames_reject_a_short_reply() {
        let mut preview = sample_social();
        preview.outline = vec![
            "A".to_owned(),
            "B".to_owned(),
            "C".to_owned(),
            "D".to_owned(),
        ];
        assert!(apply_social_continuation(&preview, "{\"frames\":[]}").is_err());
        let patch = r#"{"frames":[{"index":3,"insert":true,"frame":{"html":"<h2>D</h2>"}}]}"#;
        let continued = apply_social_continuation(&preview, patch).unwrap();
        assert_eq!(continued.frames.len(), 4);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_frames(3, patch).unwrap().len(), 1);
        assert!(continuation_frames(3, "{\"slides\":[]}").is_err());
        assert!(parse_social("no json").is_err());
    }

    #[test]
    fn shown_socials_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_social();
        preview.outline = (1..=7).map(|number| format!("Frame {number}")).collect();
        let chunks = [
            ContinueChunk { first: 3, count: 2 },
            ContinueChunk { first: 5, count: 2 },
        ];
        let board = vec![
            Vec::new(),
            vec![placeholder_frame("x"), placeholder_frame("y")],
        ];
        let shown = shown_social(&preview, &chunks, &board);
        assert_eq!(shown.frames.len(), 7);
        assert!(shown.frames[3].html.contains("Frame 4"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_social_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        social_session(&stores.sessions).await;
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
    async fn a_valid_social_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_SOCIAL);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_social_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .socials
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
        assert!(planner.contains("You plan social posts"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("social posts and carousels"));
        assert!(request.contains("Build a social post or carousel"));
        let runs = stores.sessions.runs("report").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_social_open_patches_that_social() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"frames":[{"index":0,"frame":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .socials
            .save("report-candidate-1", &sample_social())
            .await
            .unwrap();
        social_session(&stores.sessions).await;
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
            .socials
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.frames[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("report").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing social session with `count` saved candidates.
    async fn reviewing_social_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .socials
                .save(&format!("report-candidate-{number}"), &sample_social())
                .await
                .unwrap();
        }
        set_up_social_session(&stores.sessions).await;
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
    async fn a_regenerated_frame_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its frame itself.
        server.push_text(
            r#"{"frames":[{"index":0,"frame":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut social = sample_social();
        social.frames[0].html = "<h1>Old title markup</h1>".to_owned();
        stores
            .socials
            .save("report-candidate-1", &social)
            .await
            .unwrap();
        reviewing_social_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::regenerate_request(
                    "[frame 1] Write this frame anew.",
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
        assert!(text.contains("Write frame 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .socials
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.frames[0].html.contains("Fresh"));
        assert_eq!(edited.frames.len(), social.frames.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_socials_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_SOCIAL);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"frames":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_social_session_with(&stores, 2).await;
        let pinned = vec![
            "report-candidate-1".to_owned(),
            "report-candidate-2".to_owned(),
        ];
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::user(
                    "[candidate 1] [candidate 2] Hook from 1, call to action from 2.",
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
                .socials
                .load("report-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one social"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_social_session_without_a_social_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_SOCIAL);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_social_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("report", silent_log()).await.unwrap_err();
        assert!(error.contains("no social store"));
    }
}
