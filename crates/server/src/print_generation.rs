//! The print half of the built-in generation engine.
//!
//! A print session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for prints: the print prompts, the print
//! patch, the print store, and sheet-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Orientation, Print, PrintSize, Sheet};

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
use crate::instructions::PRINT_RULES;
use crate::model_client::LogSink;
use crate::prints::{PENDING_SHEET_CLASS, PrintStore, is_pending_sheet};
use crate::request::{SessionRequest, request_input};

/// The sheets each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type PrintChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Sheet>>>>;

/// What one print candidate call needs.
struct PrintCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` sheets plus the outline.
    preview_sheets: Option<usize>,
    /// The id the candidate is saved under.
    print_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

impl GenerationEngine {
    /// The print store, or the failure a print run reports
    /// without one.
    fn print_store(&self) -> Result<&PrintStore, GenerationStop> {
        self.prints.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no print store: print sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview prints the latest user turn asked to continue:
    /// every print named by a trailing continue request that still
    /// is a preview.
    pub(crate) async fn continue_print_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(prints) = &self.prints else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for print_id in crate::generation::trailing_continue_ids(&messages) {
            // A print that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(print)) = prints.load(&print_id).await
                && print.is_preview()
            {
                previews.push(print_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for a print session and returns the
    /// outcome.
    pub(crate) async fn execute_print(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => {
                self.generate_print_candidates(client, context, log).await
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
                let design_ids = self.edit_prints(client, context, &order, log).await?;
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
                let design_ids = self.edit_prints(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let print_id = self
                    .merge_prints(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![print_id],
                })
            }
            GenerationTask::Continue(print_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, print_ids, log)
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
                        "no print was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one print per requested variation. Returns the ids.
    async fn generate_print_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let prints = self.print_store()?;
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
        let first_number = match prints.list().await {
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
                let request = PrintCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_sheets: context.preview_screens(),
                    print_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_print_candidate(&client, &request, &attachments, &share, &log)
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
            if matches!(prints.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no print candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one print candidate, repairs it through
    /// fix rounds until it validates, and polishes it. The print is
    /// saved under `request.print_id` while it streams in, when the
    /// draft validates, and once more after the polish.
    async fn generate_print_candidate(
        &self,
        client: &reqwest::Client,
        request: &PrintCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Print, GenerationStop> {
        let prints = self.print_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": print_system_prompt() }),
            self.user_message(&print_candidate_prompt(request), attachments),
        ];
        let saver = PrintLiveSaver::new(prints, &self.notifier, &request.print_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_print),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(print) = partial_print(text) {
                    let rank = print.sheets.len();
                    live_saver.offer(print, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.sheets.len());
        let polished = self
            .polish_print(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        prints
            .clear_user_paths(&request.print_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new print candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_prints(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let prints = self.print_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let print = prints
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("print `{id}` does not exist")))?;
            loaded.push((id.as_str(), print));
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
        let rows = prints
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let print_id = candidate_id(base, number);
        log(&format!("merging {} into {print_id}", sources.join(", ")));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&print_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = PrintCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_sheets: None,
            print_id: print_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_print_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {print_id}"));
        Ok(print_id)
    }

    /// Applies `instruction` to each print in turn and returns the
    /// ones it saved. One failure is logged and the rest still run; the
    /// turn fails only when every edit failed.
    async fn edit_prints(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for print_id in order.artifact_ids {
            match self.edit_print(client, context, print_id, order, log).await {
                Ok(()) => saved.push(print_id.clone()),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {print_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_print(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        print_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<(), GenerationStop> {
        let prints = self.print_store()?;
        let print = prints
            .load(print_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| GenerationStop::Failed(format!("print `{print_id}` does not exist")))?;
        let instruction = order.instruction;
        let label = format!("edit {print_id}");
        // A change that names sheets is about those sheets: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named sheets without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "sheet")
            .into_iter()
            .filter(|index| *index < print.sheets.len())
            .collect();
        let measured =
            crate::print_polish::dom_findings(&print, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "sheets", &indexes);
        let total = print.sheets.len();
        let (print_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&print), String::new())
        } else if order.is_fresh {
            (
                focused_print_json(&print, &indexes, true),
                fresh_note("sheet", "sheets", &indexes, total),
            )
        } else {
            (
                focused_print_json(&print, &indexes, false),
                focus_note("sheet", "sheets", &indexes, total),
            )
        };
        let print_json = print_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &print_json,
            note: &note,
            findings: &findings,
            conversation: order.conversation,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": print_system_prompt() }),
            self.user_message(&print_edit_prompt(&context.request, &input), &attachments),
        ];
        let original = print.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| {
                crate::print_patch::apply_patch(
                    &original,
                    crate::print_patch::parse_patch(content)?,
                )
            }),
            progress: self.shared_progress(&[print_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
        // A fix can make a new problem. The touched sheets are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&print.sheets, &edited.sheets, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_print = self
            .fix_edited_print(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        prints
            .save(print_id, &final_print)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {print_id}: saved"));
        Ok(())
    }

    /// Writes the remaining sheets of the preview print `print_id`
    /// in chunks. The print is saved after every chunk, so the canvas
    /// shows it grow, then polished once it is complete. Returns how
    /// many sheets were added; 0 when the print is complete already.
    pub(crate) async fn continue_print(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        print_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let prints = self.print_store().map_err(stop_to_string)?;
        let mut print = prints
            .load(print_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("print `{print_id}` does not exist"))?;
        // A run that stopped may have left placeholder sheets behind.
        print.sheets.retain(|sheet| !is_pending_sheet(sheet));
        if !print.is_preview() {
            log(&format!(
                "continue {print_id}: the print is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {print_id}");
        let start = print.sheets.len();
        let planned = print.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} sheets written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = PrintLiveSaver::new(prints, &self.notifier, print_id);
        let board = self
            .write_print_chunks(
                client,
                context,
                &print,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = print.clone();
        if let Ok(board) = board.lock() {
            for sheets in board.iter() {
                continued.sheets.extend(sheets.iter().cloned());
            }
        }
        let added = continued.sheets.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the print stays continuable.
            if let Err(error) = saver.finish(&print).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added a sheet"));
        }
        // A failed chunk leaves the print continuable: the outline
        // stays until every title has a sheet.
        if continued.sheets.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_print),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_print = self
            .polish_print(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_print).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new sheets"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_print_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Print,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &PrintLiveSaver,
        log: &LogSink,
    ) -> PrintChunkBoard {
        let start = preview.sheets.len();
        let planned = preview.outline.len();
        let board: PrintChunkBoard = Arc::new(std::sync::Mutex::new(
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
                saver.offer(shown_print(&preview, &chunks, &board), written);
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
                    .write_print_chunk(
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
    /// showing the print grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_print_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Print,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &PrintChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let print_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": print_system_prompt() }),
            self.user_message(
                &print_continue_prompt(&context.request, preview, &print_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.sheets.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_print_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let sheets = partial_continuation_sheets(written, text);
                if let Ok(mut board) = live_board.lock()
                    && sheets.len() > board[position].len()
                {
                    board[position] = sheets;
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
        let sheets: Vec<Sheet> = continued.sheets[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = sheets;
        }
        show();
        Ok(())
    }

    /// Reviews a valid print as a print designer, one round per
    /// effort level. An improved print that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_print(
        &self,
        client: &reqwest::Client,
        mut print: Print,
        context: &ArtifactRequest<'_, Print>,
        log: &LogSink,
    ) -> Result<Print, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(print);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = print.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::print_polish::dom_findings(&print, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = print.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a print that is already good.
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
            let images = self.sheet_images(&print, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} sheet images)",
                findings.len(),
                images.len()
            ));
            let print_json = serde_json::to_string(&print).map_err(|error| error.to_string())?;
            let prompt = crate::print_polish::polish_prompt(&print_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": print_system_prompt() }),
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
            let improved = crate::print_patch::parse_patch(&content)
                .and_then(|patch| crate::print_patch::apply_patch(&print, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => print = improved,
                Ok(_) => log(&format!(
                    "{label}: polished print failed validation; keeping the previous version"
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

    /// Measures the touched sheets of an edited print and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// sheets measure clean, a round does not help, or the effort's round
    /// limit runs out. Returns the best version measured.
    async fn fix_edited_print(
        &self,
        client: &reqwest::Client,
        mut print: Print,
        fix: &EditFix<'_, Print>,
        log: &LogSink,
    ) -> Result<Print, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(print);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = print.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::print_polish::dom_findings(&print, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "sheets", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = print.clone();
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
                "{label}: fix round {round} of at most {limit} ({} findings on the touched sheets)",
                findings.len()
            ));
            let print_json = focused_print_json(&print, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("sheet", "sheets", &fix.indexes, print.sheets.len());
            let instruction = fix_instruction("sheets");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &print_json,
                note: &note,
                findings: &findings,
                conversation: "",
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": print_system_prompt() }),
                serde_json::json!({ "role": "user", "content": print_edit_prompt(fix.request, &input) }),
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
            let improved = crate::print_patch::parse_patch(&content)
                .and_then(|patch| crate::print_patch::apply_patch(&print, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => print = improved,
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

    /// PNG screenshots of the print's sheets for the polish pass, at
    /// most `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images
    /// or no Chrome is installed.
    async fn sheet_images(&self, print: &Print, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for sheet images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = print
            .sheets
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let print = print.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_sheet(&print, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: sheet {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}

/// Saves a print while it streams in, so the canvas shows the sheets
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct PrintLiveSaver {
    prints: PrintStore,
    notifier: ChangeNotifier,
    print_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final print. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl PrintLiveSaver {
    fn new(prints: &PrintStore, notifier: &ChangeNotifier, print_id: &str) -> Self {
        Self {
            prints: prints.clone(),
            notifier: notifier.clone(),
            print_id: print_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial print. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, print: Print, rank: usize) {
        if !print.validate().is_empty() {
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
            if saver.prints.save(&saver.print_id, &print).await.is_ok() {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final print after every partial save landed.
    async fn finish(&self, print: &Print) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.prints
            .save(&self.print_id, print)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The print a streaming reply has written so far: everything before
/// the sheets plus every complete sheet. `None` until the first sheet is
/// complete, or when the text before the sheets is not a print.
fn partial_print(text: &str) -> Option<Print> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "sheets")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new sheets a streaming continuation reply has completed so far.
fn partial_continuation_sheets(written: usize, text: &str) -> Vec<Sheet> {
    let Some((_, items)) = complete_array_items(text, "sheets") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"sheets\":[{}]}}", items.join(","));
    continuation_sheets(written, &json).unwrap_or_default()
}

/// The print to show while the chunks run: the preview, then every
/// chunk up to the last one that has sheets, with placeholders for the
/// sheets an earlier chunk still owes.
fn shown_print(preview: &Print, chunks: &[ContinueChunk], board: &[Vec<Sheet>]) -> Print {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|sheets| !sheets.is_empty()) else {
        return shown;
    };
    for (chunk, sheets) in chunks.iter().zip(board).take(last) {
        shown.sheets.extend(sheets.iter().cloned());
        for offset in sheets.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.sheets.push(placeholder_sheet(title));
        }
    }
    shown.sheets.extend(board[last].iter().cloned());
    shown
}

/// A sheet that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a print that does
/// not.
fn placeholder_sheet(title: &str) -> Sheet {
    Sheet {
        html: format!(
            "<div class=\"{PENDING_SHEET_CLASS} pending\"><p class=\"pending-label\">Writing</p>\
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

/// The new sheets in a continuation reply, in order. Accepts a patch
/// (the sheets of its operations at or past the existing sheets) and, as
/// a fallback, a whole print (its sheets past the existing ones).
fn continuation_sheets(written: usize, content: &str) -> Result<Vec<Sheet>, String> {
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
        .get("sheets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no sheets array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("sheet").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("sheet"))
            .filter(|sheet| sheet.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, sheet)| {
            serde_json::from_value::<Sheet>(sheet.clone()).map_err(|error| {
                format!(
                    "new sheet {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new sheets to the print in progress. The
/// outline stays until every title has a sheet, so a short reply leaves
/// the print continuable.
fn apply_print_continuation(original: &Print, content: &str) -> Result<Print, String> {
    let new_sheets = continuation_sheets(original.sheets.len(), content)?;
    if new_sheets.is_empty() {
        return Err(
            "the reply adds no sheets: reply with a patch of inserts, one per new sheet".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.sheets.extend(new_sheets);
    if continued.sheets.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The print system prompt: role, print rules, the print
/// schema, the clarification protocol, and one example print.
fn print_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Print)).unwrap_or_default();
    format!(
        "You build print pieces as JSON prints: posters, flyers, and similar pieces put on paper. \
         Each sheet is one HTML fragment plus its own CSS, for the px canvas of the print's size, \
         rotated by its orientation: 559 by 794 px for A5, 794 by 1123 px for A4, \
         1123 by 1587 px for A3, 816 by 1056 px for Letter, 1056 by 1632 px for Tabloid. \
         One sheet is a poster. Two sheets are a flyer with a front and a back.\n\
         Follow these rules:\n{rules}\n\
         The print must conform to this JSON Schema:\n{schema}\n\
         Example print:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one print JSON. No prose, no code fences.",
        rules = PRINT_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-print.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` sheets and
/// the full outline.
fn print_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} sheets of the print, in order, starting with \
         the first sheet. Put the sheet titles of the complete print in `outline`, in order, \
         every sheet title of the complete print. The app asks you for the remaining sheets \
         later. Make these {count} sheets show the theme, the layout language, and the text \
         density of the whole print.\n"
    )
}

/// The prompt line for the app's size and orientation choices. Empty
/// when the agent decides both, or when the user typed values the JSON
/// does not carry.
fn size_note(size: Option<&str>, orientation: Option<&str>) -> String {
    let size = size.and_then(PrintSize::from_name);
    let orientation = orientation.and_then(Orientation::from_name);
    match (size, orientation) {
        (Some(size), Some(orientation)) => {
            let viewport = orientation.apply(size.viewport());
            format!(
                "Lay the sheets out on the {} size, {}: {} by {} px. Set `size` to `{}` and \
                 `orientation` to `{}`.\n",
                size.as_str(),
                orientation.as_str(),
                viewport.width,
                viewport.height,
                size.as_str(),
                orientation.as_str()
            )
        }
        (Some(size), None) => format!(
            "Lay the sheets out on the {} size. Set `size` to `{}`.\n",
            size.as_str(),
            size.as_str()
        ),
        (None, Some(orientation)) => format!(
            "Turn every sheet {}. Set `orientation` to `{}`.\n",
            orientation.as_str(),
            orientation.as_str()
        ),
        (None, None) => String::new(),
    }
}

/// The prompt line that holds the print to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// sheets than the length, so the count goes to the outline instead.
fn sheet_count_note(sheet_count: Option<u32>, preview_sheets: Option<usize>) -> String {
    let Some(count) = sheet_count else {
        return String::new();
    };
    match preview_sheets {
        Some(_) => {
            format!("The user asked for {count} sheets. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} sheets. Write exactly {count} sheets.\n"),
    }
}

/// The user prompt for one print candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
fn print_candidate_prompt(request: &PrintCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a print piece for this request. The request and the answers are \
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
    if let Some(count) = request.preview_sheets {
        prompt.push_str(&print_preview_note(count));
    }
    prompt.push_str(&sheet_count_note(
        options.sheet_count,
        request.preview_sheets,
    ));
    prompt.push_str(&size_note(
        options.print_size.as_deref(),
        options.orientation.as_deref(),
    ));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("print", merge));
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
        "low" => prompt.push_str("Keep the print concise: fewer sheets, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the print JSON.");
    prompt
}

/// The user prompt for a print edit: the print as it is, the
/// request, and the change the user asked for.
fn print_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the print to change:\n{print_json}\n{note}\
         The print is for this request:\n{request}\n{conversation}\
         Apply this change: {critique}\n{findings}\
         A reference like [sheet 3, node 0/1 <h2.title>: What changed] names a sheet \
         (1-based) and one element in that sheet's html by its index path from the sheet root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [sheet 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one sheet the same way, without their text. A reference like [sheet 3] \
         names the sheet alone: the change is about that sheet. Change only what the critique asks for. Keep every other sheet and \
         value as it is. Return every changed sheet complete: html, css, and notes.\n{format}",
        print_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        conversation = crate::edit_focus::conversation_block(input.conversation),
        findings = findings_note(input.findings),
        format = crate::print_patch::PATCH_FORMAT
    )
}

/// The print as a focused edit sees it: the title, the theme, the
/// size, the orientation, the sheet count, and only the sheets at
/// `indexes`, each with its index.
fn focused_print_json(
    print: &Print,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let sheets: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            print.sheets.get(*index).map(|sheet| {
                let sheet = if is_fresh {
                    fresh_sheet(sheet)
                } else {
                    sheet.clone()
                };
                serde_json::json!({ "index": index, "sheet": sheet })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": print.title,
        "theme": print.theme,
        "size": print.size,
        "orientation": print.orientation,
        "sheet_count": print.sheets.len(),
        "sheets": sheets,
    }))
}

/// The sheet as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
fn fresh_sheet(sheet: &Sheet) -> Sheet {
    Sheet {
        html: String::new(),
        css: None,
        ..sheet.clone()
    }
}

/// The user prompt for one print continuation chunk: the preview
/// print and the chunk's sheets to add, as a patch of inserts.
fn print_continue_prompt(
    request: &SessionRequest,
    print: &Print,
    print_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = print.sheets.len();
    let planned = print.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = print
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a print in progress: its theme, its size and orientation, its first {written} \
         sheets, and `outline`, the sheet titles of the complete print:\n{print_json}\n\
         The print is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} sheets: outline titles {} to {last} of {planned}, in order, one sheet per \
         title:\n{}\n\
         Keep the theme. Match the existing sheets in CSS style, font sizes, spacing, colors, \
         and visual language, so the print reads as one piece. Do not change or repeat \
         the existing sheets.\n",
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
        "Reply with only a JSON patch that appends the new sheets, not the whole print:\n\
         {{\"sheets\":[{{\"index\":{written},\"insert\":true,\"sheet\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new sheet index {written} and insert true, in reading order. Each sheet \
         carries html, css, and notes. Omit title, theme, size, orientation, outline, and the existing sheets."
    ));
    prompt
}

/// Extracts and parses the print JSON from a model reply.
fn parse_print(content: &str) -> Result<Print, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "no JSON object in reply".to_owned())?;
    if end < start {
        return Err("no JSON object in reply".to_owned());
    }
    serde_json::from_str(&content[start..=end]).map_err(|error| format!("invalid print: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        apply_print_continuation, continuation_sheets, focused_print_json, parse_print,
        partial_print, placeholder_sheet, print_edit_prompt, print_system_prompt, sheet_count_note,
        shown_print, size_note,
    };
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::model_client::LogSink;
    use crate::prints::PrintStore;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{FakeModelServer, SAMPLE_PRINT, low_effort_options, sample_print};

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_print_edit_shows_only_the_named_sheets_and_their_findings() {
        let print = sample_print();
        let focused = focused_print_json(&print, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["sheet_count"], print.sheets.len());
        assert_eq!(value["size"], "a4");
        assert_eq!(value["orientation"], "portrait");
        assert_eq!(value["sheets"].as_array().unwrap().len(), 1);
        assert_eq!(value["sheets"][0]["index"], 1);
        let request = SessionRequest {
            request: "A launch flyer.".to_owned(),
            kind: ArtifactKind::Print,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["sheets[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[sheet 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only sheet 2 is shown.\n",
            findings: &findings,
            conversation: "Conversation, oldest first:\nuser: earlier ask\n",
        };
        let prompt = print_edit_prompt(&request, &input);
        assert!(prompt.contains("Only sheet 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- sheets[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [sheet 2, node 0/2 <p>: x] Fix the overflow."));
        assert!(prompt.contains("Conversation, oldest first:\nuser: earlier ask\n"));
        assert!(prompt.contains("Apply only the change asked below."));
        assert!(!prompt.contains("slide"));
    }

    #[test]
    fn the_sheet_count_and_the_size_hold_the_print_to_the_asked_shape() {
        assert_eq!(size_note(None, None), "");
        assert_eq!(size_note(Some("a2"), None), "");
        assert_eq!(
            size_note(Some("a3"), Some("landscape")),
            "Lay the sheets out on the a3 size, landscape: 1587 by 1123 px. Set `size` to `a3` and `orientation` to `landscape`.\n"
        );
        assert_eq!(
            size_note(Some("letter"), None),
            "Lay the sheets out on the letter size. Set `size` to `letter`.\n"
        );
        assert_eq!(
            size_note(None, Some("landscape")),
            "Turn every sheet landscape. Set `orientation` to `landscape`.\n"
        );
        assert_eq!(sheet_count_note(None, None), "");
        assert_eq!(sheet_count_note(None, Some(1)), "");
        assert_eq!(
            sheet_count_note(Some(2), None),
            "The user asked for 2 sheets. Write exactly 2 sheets.\n"
        );
        // A preview writes one sheet, so the length goes to the outline.
        assert_eq!(
            sheet_count_note(Some(2), Some(1)),
            "The user asked for 2 sheets. Put exactly 2 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        prints: PrintStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            prints: PrintStore::new(directory.path().join("prints")),
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
        .with_prints(stores.prints.clone())
    }

    /// A fresh one-candidate, low-effort print session, still in
    /// intake.
    async fn print_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("report", "Launch", "A launch flyer.")
                    .with_kind(ArtifactKind::Print)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// A print session past its setup card: the app's own questions
    /// were asked, so the next planner turn is free to write.
    async fn set_up_print_session(sessions: &SessionStore) {
        print_session(sessions).await;
        sessions
            .apply("report", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn print_system_prompt_carries_print_rules_the_schema_and_the_example() {
        let prompt = print_system_prompt();
        assert!(prompt.contains("print pieces"));
        assert!(prompt.contains("794 by 1123 px for A4"));
        assert!(prompt.contains("\"sheets\""));
        assert!(prompt.contains("\"size\""));
        assert!(prompt.contains("\"orientation\""));
        assert!(prompt.contains("Swift Design launch flyer"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
        assert!(!prompt.contains("\"slides\""));
    }

    #[test]
    fn partial_print_returns_complete_sheets_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"size":"a3","orientation":"landscape","sheets":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let print = partial_print(text).unwrap();
        assert_eq!(print.sheets.len(), 1);
        assert_eq!(print.size, design_model::PrintSize::A3);
        assert_eq!(print.orientation, design_model::Orientation::Landscape);
        assert!(partial_print("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_sheets_reject_a_short_reply() {
        let mut preview = sample_print();
        preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        assert!(apply_print_continuation(&preview, "{\"sheets\":[]}").is_err());
        let patch = r#"{"sheets":[{"index":2,"insert":true,"sheet":{"html":"<h2>C</h2>"}}]}"#;
        let continued = apply_print_continuation(&preview, patch).unwrap();
        assert_eq!(continued.sheets.len(), 3);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_sheets(2, patch).unwrap().len(), 1);
        assert!(continuation_sheets(2, "{\"slides\":[]}").is_err());
        assert!(parse_print("no json").is_err());
    }

    #[test]
    fn shown_prints_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_print();
        preview.outline = (1..=4).map(|number| format!("Sheet {number}")).collect();
        let chunks = [
            ContinueChunk { first: 2, count: 1 },
            ContinueChunk { first: 3, count: 1 },
        ];
        let board = vec![Vec::new(), vec![placeholder_sheet("x")]];
        let shown = shown_print(&preview, &chunks, &board);
        assert_eq!(shown.sheets.len(), 4);
        assert!(shown.sheets[2].html.contains("Sheet 3"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_print_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        print_session(&stores.sessions).await;
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
    async fn a_valid_print_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_PRINT);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_print_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .prints
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
        assert!(planner.contains("You plan print pieces"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("print pieces as JSON prints"));
        assert!(request.contains("Build a print piece"));
        let runs = stores.sessions.runs("report").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_print_open_patches_that_print() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"sheets":[{"index":0,"sheet":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .prints
            .save("report-candidate-1", &sample_print())
            .await
            .unwrap();
        print_session(&stores.sessions).await;
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
            .prints
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.sheets[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("report").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing print session with `count` saved candidates.
    async fn reviewing_print_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .prints
                .save(&format!("report-candidate-{number}"), &sample_print())
                .await
                .unwrap();
        }
        set_up_print_session(&stores.sessions).await;
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
    async fn a_regenerated_sheet_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its sheet itself.
        server.push_text(
            r#"{"sheets":[{"index":0,"sheet":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut print = sample_print();
        print.sheets[0].html = "<h1>Old title markup</h1>".to_owned();
        stores
            .prints
            .save("report-candidate-1", &print)
            .await
            .unwrap();
        reviewing_print_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::regenerate_request(
                    "[sheet 1] Write this sheet anew.",
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
        assert!(text.contains("Write sheet 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .prints
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.sheets[0].html.contains("Fresh"));
        assert_eq!(edited.sheets.len(), print.sheets.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_prints_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_PRINT);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"sheets":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_print_session_with(&stores, 2).await;
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
                .prints
                .load("report-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one print"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_print_session_without_a_print_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_PRINT);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_print_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("report", silent_log()).await.unwrap_err();
        assert!(error.contains("no print store"));
    }
}
