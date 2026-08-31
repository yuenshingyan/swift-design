//! The mailing half of the built-in generation engine.
//!
//! A mailing session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for mailings: the mailing prompts, the mailing
//! patch, the mailing store, and email-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Email, EmailFormat, Mailing};

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
use crate::instructions::MAILING_RULES;
use crate::mailings::{MailingStore, PENDING_EMAIL_CLASS, is_pending_email};
use crate::model_client::LogSink;
use crate::request::{SessionRequest, request_input};

/// The emails each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type MailingChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Email>>>>;

/// What one mailing candidate call needs.
struct MailingCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` emails plus the outline.
    preview_emails: Option<usize>,
    /// The id the candidate is saved under.
    mailing_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

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
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for mailing_id in crate::generation::trailing_continue_ids(&messages) {
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
            } => {
                let order = EditOrder {
                    artifact_ids: &designs,
                    instruction: &instruction,
                    is_fresh: false,
                };
                let design_ids = self.edit_mailings(client, context, &order, log).await?;
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
                let design_ids = self.edit_mailings(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
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
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for mailing_id in order.artifact_ids {
            match self
                .edit_mailing(client, context, mailing_id, order, log)
                .await
            {
                Ok(()) => saved.push(mailing_id.clone()),
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
    ) -> Result<(), GenerationStop> {
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
        let measured =
            crate::mailing_polish::dom_findings(&mailing, &self.base_url(), &label, log).await;
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
            parse: Box::new(move |content| {
                crate::mailing_patch::apply_patch(
                    &original,
                    crate::mailing_patch::parse_patch(content)?,
                )
            }),
            progress: self.shared_progress(&[mailing_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
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
        mailings
            .save(mailing_id, &final_mailing)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {mailing_id}: saved"));
        Ok(())
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
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(mailing);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = mailing.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::mailing_polish::dom_findings(&mailing, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = mailing.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a mailing that is already good.
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
            let images = self.email_images(&mailing, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} email images)",
                findings.len(),
                images.len()
            ));
            let mailing_json =
                serde_json::to_string(&mailing).map_err(|error| error.to_string())?;
            let prompt =
                crate::mailing_polish::polish_prompt(&mailing_json, &findings, images.len());
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
            let improved = crate::mailing_patch::parse_patch(&content)
                .and_then(|patch| crate::mailing_patch::apply_patch(&mailing, patch));
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
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(mailing);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = mailing.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::mailing_polish::dom_findings(&mailing, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "emails", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = mailing.clone();
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
            let improved = crate::mailing_patch::parse_patch(&content)
                .and_then(|patch| crate::mailing_patch::apply_patch(&mailing, patch));
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
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for email images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = mailing
            .emails
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let mailing = mailing.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_email(&mailing, index, &base_url).await;
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

/// Saves a mailing while it streams in, so the canvas shows the emails
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct MailingLiveSaver {
    mailings: MailingStore,
    notifier: ChangeNotifier,
    mailing_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final mailing. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl MailingLiveSaver {
    fn new(mailings: &MailingStore, notifier: &ChangeNotifier, mailing_id: &str) -> Self {
        Self {
            mailings: mailings.clone(),
            notifier: notifier.clone(),
            mailing_id: mailing_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial mailing. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, mailing: Mailing, rank: usize) {
        if !mailing.validate().is_empty() {
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
                .mailings
                .save(&saver.mailing_id, &mailing)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final mailing after every partial save landed.
    async fn finish(&self, mailing: &Mailing) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.mailings
            .save(&self.mailing_id, mailing)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The mailing a streaming reply has written so far: everything before
/// the emails plus every complete email. `None` until the first email is
/// complete, or when the text before the emails is not a mailing.
fn partial_mailing(text: &str) -> Option<Mailing> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "emails")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new emails a streaming continuation reply has completed so far.
fn partial_continuation_emails(written: usize, text: &str) -> Vec<Email> {
    let Some((_, items)) = complete_array_items(text, "emails") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"emails\":[{}]}}", items.join(","));
    continuation_emails(written, &json).unwrap_or_default()
}

/// The mailing to show while the chunks run: the preview, then every
/// chunk up to the last one that has emails, with placeholders for the
/// emails an earlier chunk still owes.
fn shown_mailing(preview: &Mailing, chunks: &[ContinueChunk], board: &[Vec<Email>]) -> Mailing {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|emails| !emails.is_empty()) else {
        return shown;
    };
    for (chunk, emails) in chunks.iter().zip(board).take(last) {
        shown.emails.extend(emails.iter().cloned());
        for offset in emails.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.emails.push(placeholder_email(title));
        }
    }
    shown.emails.extend(board[last].iter().cloned());
    shown
}

/// An email that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a mailing that does
/// not.
fn placeholder_email(title: &str) -> Email {
    Email {
        html: format!(
            "<div class=\"{PENDING_EMAIL_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new emails in a continuation reply, in order. Accepts a patch
/// (the emails of its operations at or past the existing emails) and, as
/// a fallback, a whole mailing (its emails past the existing ones).
fn continuation_emails(written: usize, content: &str) -> Result<Vec<Email>, String> {
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
        .get("emails")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no emails array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("email").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("email"))
            .filter(|email| email.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, email)| {
            serde_json::from_value::<Email>(email.clone()).map_err(|error| {
                format!(
                    "new email {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new emails to the mailing in progress. The
/// outline stays until every title has an email, so a short reply
/// leaves the mailing continuable.
fn apply_mailing_continuation(original: &Mailing, content: &str) -> Result<Mailing, String> {
    let new_emails = continuation_emails(original.emails.len(), content)?;
    if new_emails.is_empty() {
        return Err(
            "the reply adds no emails: reply with a patch of inserts, one per new email".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.emails.extend(new_emails);
    if continued.emails.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The mailing system prompt: role, mailing rules, the mailing
/// schema, the clarification protocol, and one example mailing.
fn mailing_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Mailing)).unwrap_or_default();
    format!(
        "You build mailings as JSON mailings: emails, newsletters, and email sequences read in an inbox. \
         Each email is one HTML fragment plus its own CSS, for the px canvas of the mailing's format, \
         600 px wide: 600 by 800 px for short, 600 by 1200 px for standard, \
         600 by 1800 px for long. \
         One email is a single send. Two or more emails are a sequence, in send order.\n\
         Follow these rules:\n{rules}\n\
         The mailing must conform to this JSON Schema:\n{schema}\n\
         Example mailing:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one mailing JSON. No prose, no code fences.",
        rules = MAILING_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-mailing.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` emails and
/// the full outline.
fn mailing_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} emails of the mailing, in order, starting with \
         the first email. Put the email titles of the complete mailing in `outline`, in order, \
         every email title of the complete mailing. The app asks you for the remaining emails \
         later. Make these {count} emails show the theme, the layout language, and the text \
         density of the whole mailing.\n"
    )
}

/// The prompt line for the app's format choice. Empty when the agent
/// decides it, or when the user typed a value the JSON does not carry.
fn format_note(format: Option<&str>) -> String {
    match format.and_then(EmailFormat::from_name) {
        Some(format) => {
            let viewport = format.viewport();
            format!(
                "Lay the emails out on the {} format: {} by {} px. Set `format` to `{}`.\n",
                format.as_str(),
                viewport.width,
                viewport.height,
                format.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the mailing to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// emails than the length, so the count goes to the outline instead.
fn email_count_note(email_count: Option<u32>, preview_emails: Option<usize>) -> String {
    let Some(count) = email_count else {
        return String::new();
    };
    match preview_emails {
        Some(_) => {
            format!("The user asked for {count} emails. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} emails. Write exactly {count} emails.\n"),
    }
}

/// The user prompt for one mailing candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
fn mailing_candidate_prompt(request: &MailingCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a mailing for this request. The request and the answers are \
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
    if let Some(count) = request.preview_emails {
        prompt.push_str(&mailing_preview_note(count));
    }
    prompt.push_str(&email_count_note(
        options.email_count,
        request.preview_emails,
    ));
    prompt.push_str(&format_note(options.email_format.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("mailing", merge));
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
        "low" => prompt.push_str("Keep the mailing concise: fewer emails, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the mailing JSON.");
    prompt
}

/// The user prompt for a mailing edit: the mailing as it is, the
/// request, and the change the user asked for.
fn mailing_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the mailing to change:\n{mailing_json}\n{note}\
         The mailing is for this request:\n{request}\n\
         Apply this change: {critique}\n{findings}\
         A reference like [email 3, node 0/1 <h2.title>: What changed] names an email \
         (1-based) and one element in that email's html by its index path from the email root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [email 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one email the same way, without their text. A reference like [email 3] \
         names the email alone: the change is about that email. Change only what the critique asks for. Keep every other email and \
         value as it is. Return every changed email complete: html, css, and notes.\n{format}",
        mailing_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        findings = findings_note(input.findings),
        format = crate::mailing_patch::PATCH_FORMAT
    )
}

/// The mailing as a focused edit sees it: the title, the theme, the
/// format, the email count, and only the emails at `indexes`, each
/// with its index.
fn focused_mailing_json(
    mailing: &Mailing,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let emails: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            mailing.emails.get(*index).map(|email| {
                let email = if is_fresh {
                    fresh_email(email)
                } else {
                    email.clone()
                };
                serde_json::json!({ "index": index, "email": email })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": mailing.title,
        "theme": mailing.theme,
        "format": mailing.format,
        "email_count": mailing.emails.len(),
        "emails": emails,
    }))
}

/// The email as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
fn fresh_email(email: &Email) -> Email {
    Email {
        html: String::new(),
        css: None,
        ..email.clone()
    }
}

/// The user prompt for one mailing continuation chunk: the preview
/// mailing and the chunk's emails to add, as a patch of inserts.
fn mailing_continue_prompt(
    request: &SessionRequest,
    mailing: &Mailing,
    mailing_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = mailing.emails.len();
    let planned = mailing.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = mailing
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a mailing in progress: its theme, its format, its first {written} \
         emails, and `outline`, the email titles of the complete mailing:\n{mailing_json}\n\
         The mailing is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} emails: outline titles {} to {last} of {planned}, in order, one email per \
         title:\n{}\n\
         Keep the theme. Match the existing emails in CSS style, font sizes, spacing, colors, \
         and visual language, so the mailing reads as one piece. Do not change or repeat \
         the existing emails.\n",
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
        "Reply with only a JSON patch that appends the new emails, not the whole mailing:\n\
         {{\"emails\":[{{\"index\":{written},\"insert\":true,\"email\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new email index {written} and insert true, in send order. Each email \
         carries html, css, and notes. Omit title, theme, format, outline, and the existing emails."
    ));
    prompt
}

/// Extracts and parses the mailing JSON from a model reply.
fn parse_mailing(content: &str) -> Result<Mailing, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid mailing: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        apply_mailing_continuation, continuation_emails, email_count_note, focused_mailing_json,
        format_note, mailing_edit_prompt, mailing_system_prompt, parse_mailing, partial_mailing,
        placeholder_email, shown_mailing,
    };
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::mailings::MailingStore;
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{
        FakeModelServer, SAMPLE_MAILING, low_effort_options, sample_mailing,
    };

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_mailing_edit_shows_only_the_named_emails_and_their_findings() {
        let mailing = sample_mailing();
        let focused = focused_mailing_json(&mailing, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["email_count"], mailing.emails.len());
        assert_eq!(value["format"], "standard");
        assert_eq!(value["emails"].as_array().unwrap().len(), 1);
        assert_eq!(value["emails"][0]["index"], 1);
        let request = SessionRequest {
            request: "A launch email.".to_owned(),
            kind: ArtifactKind::Mailing,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["emails[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[email 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only email 2 is shown.\n",
            findings: &findings,
        };
        let prompt = mailing_edit_prompt(&request, &input);
        assert!(prompt.contains("Only email 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- emails[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [email 2, node 0/2 <p>: x] Fix the overflow."));
        assert!(!prompt.contains("slide"));
    }

    #[test]
    fn the_email_count_and_the_format_hold_the_mailing_to_the_asked_shape() {
        assert_eq!(format_note(None), "");
        assert_eq!(format_note(Some("tall")), "");
        assert_eq!(
            format_note(Some("long")),
            "Lay the emails out on the long format: 600 by 1800 px. Set `format` to `long`.\n"
        );
        assert_eq!(
            format_note(Some("short")),
            "Lay the emails out on the short format: 600 by 800 px. Set `format` to `short`.\n"
        );
        assert_eq!(email_count_note(None, None), "");
        assert_eq!(email_count_note(None, Some(1)), "");
        assert_eq!(
            email_count_note(Some(2), None),
            "The user asked for 2 emails. Write exactly 2 emails.\n"
        );
        // A preview writes one email, so the length goes to the outline.
        assert_eq!(
            email_count_note(Some(2), Some(1)),
            "The user asked for 2 emails. Put exactly 2 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        mailings: MailingStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            mailings: MailingStore::new(directory.path().join("mailings")),
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
        .with_mailings(stores.mailings.clone())
    }

    /// A fresh one-candidate, low-effort mailing session, still in
    /// intake.
    async fn mailing_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("report", "Launch", "A launch email.")
                    .with_kind(ArtifactKind::Mailing)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// A mailing session past its setup card: the app's own questions
    /// were asked, so the next planner turn is free to write.
    async fn set_up_mailing_session(sessions: &SessionStore) {
        mailing_session(sessions).await;
        sessions
            .apply("report", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn mailing_system_prompt_carries_mailing_rules_the_schema_and_the_example() {
        let prompt = mailing_system_prompt();
        assert!(prompt.contains("email sequences read in an inbox"));
        assert!(prompt.contains("600 by 1200 px for standard"));
        assert!(prompt.contains("\"emails\""));
        assert!(prompt.contains("\"format\""));
        assert!(prompt.contains("Swift Design launch email"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
        assert!(!prompt.contains("\"slides\""));
    }

    #[test]
    fn partial_mailing_returns_complete_emails_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"format":"long","emails":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let mailing = partial_mailing(text).unwrap();
        assert_eq!(mailing.emails.len(), 1);
        assert_eq!(mailing.format, design_model::EmailFormat::Long);
        assert!(partial_mailing("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_emails_reject_a_short_reply() {
        let mut preview = sample_mailing();
        preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        assert!(apply_mailing_continuation(&preview, "{\"emails\":[]}").is_err());
        let patch = r#"{"emails":[{"index":2,"insert":true,"email":{"html":"<h2>C</h2>"}}]}"#;
        let continued = apply_mailing_continuation(&preview, patch).unwrap();
        assert_eq!(continued.emails.len(), 3);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_emails(2, patch).unwrap().len(), 1);
        assert!(continuation_emails(2, "{\"slides\":[]}").is_err());
        assert!(parse_mailing("no json").is_err());
    }

    #[test]
    fn shown_mailings_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_mailing();
        preview.outline = (1..=4).map(|number| format!("Email {number}")).collect();
        let chunks = [
            ContinueChunk { first: 2, count: 1 },
            ContinueChunk { first: 3, count: 1 },
        ];
        let board = vec![Vec::new(), vec![placeholder_email("x")]];
        let shown = shown_mailing(&preview, &chunks, &board);
        assert_eq!(shown.emails.len(), 4);
        assert!(shown.emails[2].html.contains("Email 3"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_mailing_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        mailing_session(&stores.sessions).await;
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
    async fn a_valid_mailing_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_MAILING);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_mailing_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .mailings
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
        assert!(planner.contains("You plan emails"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("mailings as JSON mailings"));
        assert!(request.contains("Build a mailing"));
        let runs = stores.sessions.runs("report").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_mailing_open_patches_that_mailing() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"emails":[{"index":0,"email":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .mailings
            .save("report-candidate-1", &sample_mailing())
            .await
            .unwrap();
        mailing_session(&stores.sessions).await;
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
            .mailings
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.emails[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("report").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing mailing session with `count` saved candidates.
    async fn reviewing_mailing_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .mailings
                .save(&format!("report-candidate-{number}"), &sample_mailing())
                .await
                .unwrap();
        }
        set_up_mailing_session(&stores.sessions).await;
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
    async fn a_regenerated_email_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its email itself.
        server.push_text(
            r#"{"emails":[{"index":0,"email":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut mailing = sample_mailing();
        mailing.emails[0].html = "<h1>Old title markup</h1>".to_owned();
        stores
            .mailings
            .save("report-candidate-1", &mailing)
            .await
            .unwrap();
        reviewing_mailing_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::regenerate_request(
                    "[email 1] Write this email anew.",
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
        assert!(text.contains("Write email 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .mailings
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.emails[0].html.contains("Fresh"));
        assert_eq!(edited.emails.len(), mailing.emails.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_mailings_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_MAILING);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"emails":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_mailing_session_with(&stores, 2).await;
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
                .mailings
                .load("report-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one mailing"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_mailing_session_without_a_mailing_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_MAILING);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_mailing_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("report", silent_log()).await.unwrap_err();
        assert!(error.contains("no mailing store"));
    }
}
