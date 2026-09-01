//! The campaign half of the built-in generation engine.
//!
//! A campaign session runs the same loop as a deck session: read the
//! request, ask the model for each candidate, validate, feed every
//! validation error back for a fix round, polish, and save. This module
//! holds what differs for campaigns: the campaign prompts, the campaign
//! patch, the campaign store, and ad-typed continuation. The
//! fix-round loop, the attachments, the progress sinks, and the concept
//! planning come from `generation.rs`.

use std::sync::Arc;

use design_model::{Ad, AdSize, Campaign};

use crate::campaigns::{CampaignStore, PENDING_AD_CLASS, is_pending_ad};
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
use crate::instructions::CAMPAIGN_RULES;
use crate::model_client::LogSink;
use crate::request::{SessionRequest, request_input};

/// The ads each continuation chunk has produced so far, shared
/// between the chunks that run at once.
type CampaignChunkBoard = Arc<std::sync::Mutex<Vec<Vec<Ad>>>>;

/// What one campaign candidate call needs.
struct CampaignCandidateRequest<'request> {
    context: &'request GenerationContext,
    candidate_number: usize,
    concepts: &'request [Concept],
    /// `Some(n)`: write only the first `n` ads plus the outline.
    preview_ads: Option<usize>,
    /// The id the candidate is saved under.
    campaign_id: String,
    /// The template the candidate takes its look from, when the options
    /// name one.
    template: Option<&'request crate::templates::Template>,
    /// The candidates to combine, when this candidate is a merge.
    merge: Option<&'request MergeInput>,
}

impl GenerationEngine {
    /// The campaign store, or the failure a campaign run reports
    /// without one.
    fn campaign_store(&self) -> Result<&CampaignStore, GenerationStop> {
        self.campaigns.as_ref().ok_or_else(|| {
            GenerationStop::Failed(
                "this engine has no campaign store: campaign sessions cannot run".to_owned(),
            )
        })
    }

    /// The preview campaigns the latest user turn asked to continue:
    /// every campaign named by a trailing continue request that still
    /// is a preview.
    pub(crate) async fn continue_campaign_requests(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let Some(campaigns) = &self.campaigns else {
            return Ok(Vec::new());
        };
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for campaign_id in crate::generation::trailing_continue_ids(&messages) {
            // A campaign that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(campaign)) = campaigns.load(&campaign_id).await
                && campaign.is_preview()
            {
                previews.push(campaign_id);
            }
        }
        Ok(previews)
    }

    /// Runs the chosen task for a campaign session and returns the
    /// outcome.
    pub(crate) async fn execute_campaign(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        task: GenerationTask,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        match task {
            GenerationTask::Candidates => {
                self.generate_campaign_candidates(client, context, log)
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
                let design_ids = self.edit_campaigns(client, context, &order, log).await?;
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
                let design_ids = self.edit_campaigns(client, context, &order, log).await?;
                Ok(GenerationOutcome::Wrote { design_ids })
            }
            GenerationTask::Merge {
                sources,
                instruction,
            } => {
                let campaign_id = self
                    .merge_campaigns(client, context, &sources, &instruction, log)
                    .await?;
                Ok(GenerationOutcome::Wrote {
                    design_ids: vec![campaign_id],
                })
            }
            GenerationTask::Continue(campaign_ids) => {
                let outcomes = self
                    .continue_artifacts(client, context, campaign_ids, log)
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
                        "no campaign was continued",
                    )));
                }
                // The late finishes count too.
                Ok(GenerationOutcome::Wrote {
                    design_ids: outcomes.into_iter().map(|(id, _)| id).collect(),
                })
            }
        }
    }

    /// Writes one campaign per requested variation. Returns the ids.
    async fn generate_campaign_candidates(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let campaigns = self.campaign_store()?;
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
        let first_number = match campaigns.list().await {
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
                let request = CampaignCandidateRequest {
                    context: &context,
                    candidate_number,
                    concepts: &concepts,
                    preview_ads: context.preview_screens(),
                    campaign_id: id.clone(),
                    template: template.as_ref(),
                    merge: None,
                };
                engine
                    .generate_campaign_candidate(&client, &request, &attachments, &share, &log)
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
            if matches!(campaigns.load(id).await, Ok(Some(_))) {
                saved.push(id.clone());
            }
        }
        if saved.is_empty() {
            return Err(GenerationStop::Failed(failure_message(
                &failures,
                "no campaign candidate reached the store",
            )));
        }
        for failure in &failures {
            log(&format!("candidate failed: {failure}"));
        }
        Ok(GenerationOutcome::Wrote { design_ids: saved })
    }

    /// Asks the model for one campaign candidate, repairs it through
    /// fix rounds until it validates, and polishes it. The campaign is
    /// saved under `request.campaign_id` while it streams in, when the
    /// draft validates, and once more after the polish.
    async fn generate_campaign_candidate(
        &self,
        client: &reqwest::Client,
        request: &CampaignCandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Campaign, GenerationStop> {
        let campaigns = self.campaign_store()?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": campaign_system_prompt() }),
            self.user_message(&campaign_candidate_prompt(request), attachments),
        ];
        let saver = CampaignLiveSaver::new(campaigns, &self.notifier, &request.campaign_id);
        let live_saver = saver.clone();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(parse_campaign),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(campaign) = partial_campaign(text) {
                    let rank = campaign.ads.len();
                    live_saver.offer(campaign, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.ads.len());
        let polished = self
            .polish_campaign(client, draft, &context, log)
            .await
            .map_err(GenerationStop::Failed)?;
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        campaigns
            .clear_user_paths(&request.campaign_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Combines parts of `sources` into one new campaign candidate, as
    /// `instruction` asks, and returns its id. The new candidate takes
    /// the next free number and goes through the same fix and polish
    /// rounds as a fresh candidate.
    async fn merge_campaigns(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        sources: &[String],
        instruction: &str,
        log: &LogSink,
    ) -> Result<String, GenerationStop> {
        let campaigns = self.campaign_store()?;
        let mut loaded = Vec::new();
        for id in sources {
            let campaign = campaigns
                .load(id)
                .await
                .map_err(|error| GenerationStop::Failed(error.to_string()))?
                .ok_or_else(|| GenerationStop::Failed(format!("campaign `{id}` does not exist")))?;
            loaded.push((id.as_str(), campaign));
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
        let rows = campaigns
            .list()
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let number = next_candidate_number(base, rows.iter().map(|row| row.id.as_str()));
        let campaign_id = candidate_id(base, number);
        log(&format!(
            "merging {} into {campaign_id}",
            sources.join(", ")
        ));
        let attachments = self.load_attachments(&context.session_id, log).await;
        let share = self
            .shared_progress(std::slice::from_ref(&campaign_id), 5, 95)
            .pop()
            .ok_or_else(|| GenerationStop::Failed("no progress share".to_owned()))?;
        share(0.0);
        let request = CampaignCandidateRequest {
            context,
            candidate_number: number,
            concepts: &[],
            preview_ads: None,
            campaign_id: campaign_id.clone(),
            template: None,
            merge: Some(&merge),
        };
        self.generate_campaign_candidate(client, &request, &attachments, &share, log)
            .await?;
        log(&format!("merge: saved as {campaign_id}"));
        Ok(campaign_id)
    }

    /// Applies `instruction` to each campaign in turn and returns the
    /// ones it saved. One failure is logged and the rest still run; the
    /// turn fails only when every edit failed.
    async fn edit_campaigns(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<Vec<String>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for campaign_id in order.artifact_ids {
            match self
                .edit_campaign(client, context, campaign_id, order, log)
                .await
            {
                Ok(()) => saved.push(campaign_id.clone()),
                Err(GenerationStop::NeedsClarification(set)) => {
                    return Err(GenerationStop::NeedsClarification(set));
                }
                Err(GenerationStop::Failed(message)) => {
                    log(&format!("edit {campaign_id}: {message}"));
                    last_error = Some(GenerationStop::Failed(message));
                }
            }
        }
        match (saved.is_empty(), last_error) {
            (true, Some(stop)) => Err(stop),
            _ => Ok(saved),
        }
    }

    async fn edit_campaign(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        campaign_id: &str,
        order: &EditOrder<'_>,
        log: &LogSink,
    ) -> Result<(), GenerationStop> {
        let campaigns = self.campaign_store()?;
        let campaign = campaigns
            .load(campaign_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?
            .ok_or_else(|| {
                GenerationStop::Failed(format!("campaign `{campaign_id}` does not exist"))
            })?;
        let instruction = order.instruction;
        let label = format!("edit {campaign_id}");
        // A change that names ads is about those ads: the model
        // sees only them. A change that names none is systemic. A
        // regenerate sees the named ads without their markup.
        let indexes: Vec<usize> = referenced_indexes(instruction, "ad")
            .into_iter()
            .filter(|index| *index < campaign.ads.len())
            .collect();
        let measured =
            crate::campaign_polish::dom_findings(&campaign, &self.base_url(), &label, log).await;
        let findings = findings_for(&measured, "ads", &indexes);
        let total = campaign.ads.len();
        let (campaign_json, note) = if indexes.is_empty() {
            (serde_json::to_string(&campaign), String::new())
        } else if order.is_fresh {
            (
                focused_campaign_json(&campaign, &indexes, true),
                fresh_note("ad", "ads", &indexes, total),
            )
        } else {
            (
                focused_campaign_json(&campaign, &indexes, false),
                focus_note("ad", "ads", &indexes, total),
            )
        };
        let campaign_json =
            campaign_json.map_err(|error| GenerationStop::Failed(error.to_string()))?;
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = EditInput {
            instruction,
            artifact_json: &campaign_json,
            note: &note,
            findings: &findings,
        };
        let messages = vec![
            serde_json::json!({ "role": "system", "content": campaign_system_prompt() }),
            self.user_message(
                &campaign_edit_prompt(&context.request, &input),
                &attachments,
            ),
        ];
        let original = campaign.clone();
        let effort = context.effort().to_owned();
        let request = ArtifactRequest {
            effort,
            label,
            parse: Box::new(move |content| {
                crate::campaign_patch::apply_patch(
                    &original,
                    crate::campaign_patch::parse_patch(content)?,
                )
            }),
            progress: self.shared_progress(&[campaign_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self.request_valid(client, messages, &request, log).await?;
        // A fix can make a new problem. The touched ads are measured
        // again, and the model tweaks them until they measure clean or
        // the effort's rounds run out.
        let touched = touched_indexes(&campaign.ads, &edited.ads, &indexes);
        let fix = EditFix {
            request: &context.request,
            context: &request,
            indexes: touched,
        };
        let final_campaign = self
            .fix_edited_campaign(client, edited, &fix, log)
            .await
            .map_err(GenerationStop::Failed)?;
        campaigns
            .save(campaign_id, &final_campaign)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {campaign_id}: saved"));
        Ok(())
    }

    /// Writes the remaining ads of the preview campaign `campaign_id`
    /// in chunks. The campaign is saved after every chunk, so the canvas
    /// shows it grow, then polished once it is complete. Returns how
    /// many ads were added; 0 when the campaign is complete already.
    pub(crate) async fn continue_campaign(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        campaign_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let campaigns = self.campaign_store().map_err(stop_to_string)?;
        let mut campaign = campaigns
            .load(campaign_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("campaign `{campaign_id}` does not exist"))?;
        // A run that stopped may have left placeholder ads behind.
        campaign.ads.retain(|ad| !is_pending_ad(ad));
        if !campaign.is_preview() {
            log(&format!(
                "continue {campaign_id}: the campaign is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {campaign_id}");
        let start = campaign.ads.len();
        let planned = campaign.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} ads written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        let saver = CampaignLiveSaver::new(campaigns, &self.notifier, campaign_id);
        let board = self
            .write_campaign_chunks(
                client,
                context,
                &campaign,
                &chunks,
                attachments,
                progress,
                &saver,
                log,
            )
            .await;
        let mut continued = campaign.clone();
        if let Ok(board) = board.lock() {
            for ads in board.iter() {
                continued.ads.extend(ads.iter().cloned());
            }
        }
        let added = continued.ads.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the campaign stays continuable.
            if let Err(error) = saver.finish(&campaign).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(format!("{label}: no chunk added an ad"));
        }
        // A failed chunk leaves the campaign continuable: the outline
        // stays until every title has an ad.
        if continued.ads.len() >= planned {
            continued.outline.clear();
        }
        saver.finish(&continued).await?;
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_campaign),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_campaign = self
            .polish_campaign(client, continued, &polish_context, log)
            .await?;
        saver.finish(&final_campaign).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new ads"));
        Ok(added)
    }

    /// Runs every continuation chunk of `preview` at the same time and
    /// returns the board with what each chunk wrote. A chunk that fails
    /// is logged and leaves its row empty.
    #[allow(clippy::too_many_arguments)]
    async fn write_campaign_chunks(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Campaign,
        chunks: &[ContinueChunk],
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        saver: &CampaignLiveSaver,
        log: &LogSink,
    ) -> CampaignChunkBoard {
        let start = preview.ads.len();
        let planned = preview.outline.len();
        let board: CampaignChunkBoard = Arc::new(std::sync::Mutex::new(
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
                saver.offer(shown_campaign(&preview, &chunks, &board), written);
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
                    .write_campaign_chunk(
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
    /// showing the campaign grow while the reply streams.
    #[allow(clippy::too_many_arguments)]
    async fn write_campaign_chunk(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        preview: &Campaign,
        (position, chunk): (usize, ContinueChunk),
        attachments: &Attachments,
        board: &CampaignChunkBoard,
        show: &Arc<dyn Fn() + Send + Sync>,
        log: &LogSink,
    ) -> Result<(), String> {
        let campaign_json = serde_json::to_string(preview).map_err(|error| error.to_string())?;
        let messages = vec![
            serde_json::json!({ "role": "system", "content": campaign_system_prompt() }),
            self.user_message(
                &campaign_continue_prompt(&context.request, preview, &campaign_json, chunk),
                attachments,
            ),
        ];
        let original = preview.clone();
        let written = preview.ads.len();
        let live_board = Arc::clone(board);
        let live_show = Arc::clone(show);
        let request = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: format!("continue chunk {}", position + 1),
            parse: Box::new(move |content| apply_campaign_continuation(&original, content)),
            progress: None,
            live: Some(Arc::new(move |text: &str| {
                let ads = partial_continuation_ads(written, text);
                if let Ok(mut board) = live_board.lock()
                    && ads.len() > board[position].len()
                {
                    board[position] = ads;
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
        let ads: Vec<Ad> = continued.ads[written..].to_vec();
        if let Ok(mut board) = board.lock() {
            board[position] = ads;
        }
        show();
        Ok(())
    }

    /// Reviews a valid campaign as a campaign designer, one round per
    /// effort level. An improved campaign that validates replaces the
    /// original; anything else keeps the original and logs why.
    async fn polish_campaign(
        &self,
        client: &reqwest::Client,
        mut campaign: Campaign,
        context: &ArtifactRequest<'_, Campaign>,
        log: &LogSink,
    ) -> Result<Campaign, String> {
        let label = &context.label;
        // Without Chrome nothing can be measured, and a round would
        // ask the model to fix findings that were never taken.
        if !crate::polish::can_audit() {
            log(&format!(
                "{label}: {}",
                crate::polish::PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(campaign);
        }
        let limit = crate::polish::polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = campaign.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings =
                crate::campaign_polish::dom_findings(&campaign, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = campaign.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a campaign that is already good.
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
            let images = self.ad_images(&campaign, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} ad images)",
                findings.len(),
                images.len()
            ));
            let campaign_json =
                serde_json::to_string(&campaign).map_err(|error| error.to_string())?;
            let prompt =
                crate::campaign_polish::polish_prompt(&campaign_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": campaign_system_prompt() }),
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
            let improved = crate::campaign_patch::parse_patch(&content)
                .and_then(|patch| crate::campaign_patch::apply_patch(&campaign, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => campaign = improved,
                Ok(_) => log(&format!(
                    "{label}: polished campaign failed validation; keeping the previous version"
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

    /// Measures the touched ads of an edited campaign and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// ads measure clean, a round does not help, or the effort's round
    /// limit runs out. Returns the best version measured.
    async fn fix_edited_campaign(
        &self,
        client: &reqwest::Client,
        mut campaign: Campaign,
        fix: &EditFix<'_, Campaign>,
        log: &LogSink,
    ) -> Result<Campaign, String> {
        let label = &fix.context.label;
        if fix.indexes.is_empty() || !crate::polish::can_audit() {
            fix.context.report(1.0);
            return Ok(campaign);
        }
        let limit = crate::polish::polish_round_limit(&fix.context.effort);
        let mut best = campaign.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = crate::polish::PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured =
                crate::campaign_polish::dom_findings(&campaign, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "ads", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = campaign.clone();
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
                "{label}: fix round {round} of at most {limit} ({} findings on the touched ads)",
                findings.len()
            ));
            let campaign_json = focused_campaign_json(&campaign, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("ad", "ads", &fix.indexes, campaign.ads.len());
            let instruction = fix_instruction("ads");
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &campaign_json,
                note: &note,
                findings: &findings,
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": campaign_system_prompt() }),
                serde_json::json!({ "role": "user", "content": campaign_edit_prompt(fix.request, &input) }),
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
            let improved = crate::campaign_patch::parse_patch(&content)
                .and_then(|patch| crate::campaign_patch::apply_patch(&campaign, patch));
            match improved {
                Ok(improved) if improved.validate().is_empty() => campaign = improved,
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

    /// PNG screenshots of the campaign's ads for the polish pass, at
    /// most `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images
    /// or no Chrome is installed.
    async fn ad_images(&self, campaign: &Campaign, label: &str, log: &LogSink) -> Vec<Vec<u8>> {
        if !crate::screenshots::supports_vision(self.model.model()) {
            return Vec::new();
        }
        if crate::screenshots::find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for ad images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = campaign
            .ads
            .len()
            .min(crate::screenshots::POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let campaign = campaign.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = crate::screenshots::screenshot_ad(&campaign, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: ad {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}

/// Saves a campaign while it streams in, so the canvas shows the ads
/// appear. A save happens only when the caller's rank grows, and saves
/// land in order.
#[derive(Clone)]
struct CampaignLiveSaver {
    campaigns: CampaignStore,
    notifier: ChangeNotifier,
    campaign_id: String,
    saved_rank: Arc<std::sync::Mutex<Option<usize>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    /// True once `finish` has written the final campaign. A partial save
    /// spawned earlier can still be waiting for the write lock, and it
    /// must not put a half-written draft back over the final one.
    is_finished: Arc<std::sync::atomic::AtomicBool>,
}

impl CampaignLiveSaver {
    fn new(campaigns: &CampaignStore, notifier: &ChangeNotifier, campaign_id: &str) -> Self {
        Self {
            campaigns: campaigns.clone(),
            notifier: notifier.clone(),
            campaign_id: campaign_id.to_owned(),
            saved_rank: Arc::new(std::sync::Mutex::new(None)),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            is_finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Offers a partial campaign. It is saved when it validates and its
    /// `rank` is above the last saved rank.
    fn offer(&self, campaign: Campaign, rank: usize) {
        if !campaign.validate().is_empty() {
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
                .campaigns
                .save(&saver.campaign_id, &campaign)
                .await
                .is_ok()
            {
                saver.notifier.notify();
            }
        });
    }

    /// Saves the final campaign after every partial save landed.
    async fn finish(&self, campaign: &Campaign) -> Result<(), String> {
        let _guard = self.write_lock.lock().await;
        self.is_finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.campaigns
            .save(&self.campaign_id, campaign)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }
}

/// The campaign a streaming reply has written so far: everything before
/// the ads plus every complete ad. `None` until the first ad is
/// complete, or when the text before the ads is not a campaign.
fn partial_campaign(text: &str) -> Option<Campaign> {
    let start = text.find('{')?;
    let (array_start, items) = complete_array_items(text, "ads")?;
    if items.is_empty() || array_start < start {
        return None;
    }
    let json = format!("{}[{}]}}", &text[start..array_start], items.join(","));
    serde_json::from_str(&json).ok()
}

/// The new ads a streaming continuation reply has completed so far.
fn partial_continuation_ads(written: usize, text: &str) -> Vec<Ad> {
    let Some((_, items)) = complete_array_items(text, "ads") else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }
    let json = format!("{{\"ads\":[{}]}}", items.join(","));
    continuation_ads(written, &json).unwrap_or_default()
}

/// The campaign to show while the chunks run: the preview, then every
/// chunk up to the last one that has ads, with placeholders for the
/// ads an earlier chunk still owes.
fn shown_campaign(preview: &Campaign, chunks: &[ContinueChunk], board: &[Vec<Ad>]) -> Campaign {
    let mut shown = preview.clone();
    let Some(last) = board.iter().rposition(|ads| !ads.is_empty()) else {
        return shown;
    };
    for (chunk, ads) in chunks.iter().zip(board).take(last) {
        shown.ads.extend(ads.iter().cloned());
        for offset in ads.len()..chunk.count {
            let title = preview
                .outline
                .get(chunk.first + offset)
                .map(String::as_str)
                .unwrap_or_default();
            shown.ads.push(placeholder_ad(title));
        }
    }
    shown.ads.extend(board[last].iter().cloned());
    shown
}

/// An ad that holds the place of one the model has not written yet.
/// It must validate, because the live saver drops a campaign that does
/// not.
fn placeholder_ad(title: &str) -> Ad {
    Ad {
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

/// The new ads in a continuation reply, in order. Accepts a patch
/// (the ads of its operations at or past the existing ads) and, as
/// a fallback, a whole campaign (its ads past the existing ones).
fn continuation_ads(written: usize, content: &str) -> Result<Vec<Ad>, String> {
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
        .get("ads")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "the reply has no ads array".to_owned())?;
    let is_patch = items
        .iter()
        .any(|item| item.get("ad").is_some() || item.get("index").is_some());
    let candidates: Vec<&serde_json::Value> = if is_patch {
        items
            .iter()
            .filter(|item| {
                item.get("index")
                    .and_then(serde_json::Value::as_u64)
                    .is_none_or(|index| index as usize >= written)
            })
            .filter_map(|item| item.get("ad"))
            .filter(|ad| ad.is_object())
            .collect()
    } else {
        items.iter().skip(written).collect()
    };
    candidates
        .into_iter()
        .enumerate()
        .map(|(position, ad)| {
            serde_json::from_value::<Ad>(ad.clone()).map_err(|error| {
                format!(
                    "new ad {} is invalid: {error}: give it html, css, and notes",
                    position + 1
                )
            })
        })
        .collect()
}

/// Appends the reply's new ads to the campaign in progress. The
/// outline stays until every title has an ad, so a short reply
/// leaves the campaign continuable.
fn apply_campaign_continuation(original: &Campaign, content: &str) -> Result<Campaign, String> {
    let new_ads = continuation_ads(original.ads.len(), content)?;
    if new_ads.is_empty() {
        return Err(
            "the reply adds no ads: reply with a patch of inserts, one per new ad".to_owned(),
        );
    }
    let mut continued = original.clone();
    continued.ads.extend(new_ads);
    if continued.ads.len() >= continued.outline.len() {
        continued.outline.clear();
    }
    Ok(continued)
}

/// The campaign system prompt: role, campaign rules, the campaign
/// schema, the clarification protocol, and one example campaign.
fn campaign_system_prompt() -> String {
    let schema = serde_json::to_string(&schemars::schema_for!(Campaign)).unwrap_or_default();
    format!(
        "You build display ad campaigns as JSON campaigns: banners and display units read on a page. \
         Each ad is one HTML fragment plus its own CSS, for the px canvas of the campaign's size: \
         300 by 250 px for medium_rectangle, 728 by 90 px for leaderboard, 300 by 600 px for \
         half_page, 160 by 600 px for skyscraper, 320 by 100 px for mobile_banner. \
         One ad is a single placement. Two or more ads are A/B variants of the same size, in priority order.\n\
         Follow these rules:\n{rules}\n\
         The campaign must conform to this JSON Schema:\n{schema}\n\
         Example campaign:\n{example}\n\
         The request and the answers are authoritative. Do not override an answer. Decide the rest yourself.\n\
         If they lack a detail you cannot design without, do not guess. Reply with only this JSON instead:\n\
         {{\"needs_clarification\":{{\"title\":\"...\",\"message\":\"...\",\"questions\":[{{\"id\":\"...\",\"label\":\"...\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"...\",\"label\":\"...\"}}]}}],\"can_proceed_with_assumptions\":true}}}}\n\
         Ask at most {limit} questions. Otherwise reply with only one campaign JSON. No prose, no code fences.",
        rules = CAMPAIGN_RULES.join("\n"),
        example = include_str!("../../../fixtures/sample-campaign.json"),
        limit = design_model::QUESTIONS_PER_TURN_LIMIT,
    )
}

/// The prompt lines for a preview candidate: write `count` ads and
/// the full outline.
fn campaign_preview_note(count: usize) -> String {
    format!(
        "Write a preview: only the first {count} ads of the campaign, in order, starting with \
         the first ad. Put the ad titles of the complete campaign in `outline`, in order, \
         every ad title of the complete campaign. The app asks you for the remaining ads \
         later. Make these {count} ads show the theme, the layout language, and the text \
         density of the whole campaign.\n"
    )
}

/// The prompt line for the app's size choice. Empty when the agent
/// decides it, or when the user typed a value the JSON does not carry.
fn size_note(size: Option<&str>) -> String {
    match size.and_then(AdSize::from_name) {
        Some(size) => {
            let viewport = size.viewport();
            format!(
                "Lay the ads out on the {} size: {} by {} px. Set `size` to `{}`.\n",
                size.as_str(),
                viewport.width,
                viewport.height,
                size.as_str()
            )
        }
        None => String::new(),
    }
}

/// The prompt line that holds the campaign to the length the user
/// asked for. Empty when the user set no length. A preview writes fewer
/// ads than the length, so the count goes to the outline instead.
fn ad_count_note(ad_count: Option<u32>, preview_ads: Option<usize>) -> String {
    let Some(count) = ad_count else {
        return String::new();
    };
    match preview_ads {
        Some(_) => {
            format!("The user asked for {count} ads. Put exactly {count} titles in `outline`.\n")
        }
        None => format!("The user asked for {count} ads. Write exactly {count} ads.\n"),
    }
}

/// The user prompt for one campaign candidate: the request and the
/// answers are authoritative, plus the template, preview, concept, and
/// effort notes.
fn campaign_candidate_prompt(request: &CampaignCandidateRequest<'_>) -> String {
    let options = &request.context.options;
    let candidate_number = request.candidate_number;
    let mut prompt = format!(
        "Build a campaign for this request. The request and the answers are \
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
    if let Some(count) = request.preview_ads {
        prompt.push_str(&campaign_preview_note(count));
    }
    prompt.push_str(&ad_count_note(options.ad_count, request.preview_ads));
    prompt.push_str(&size_note(options.ad_size.as_deref()));
    if let Some(merge) = request.merge {
        prompt.push_str(&merge_note("campaign", merge));
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
        "low" => prompt.push_str("Keep the campaign concise: fewer ads, short text.\n"),
        "high" => {
            prompt.push_str("Work carefully: complete content, strong structure, clear notes.\n")
        }
        _ => {}
    }
    prompt.push_str("Reply with only the campaign JSON.");
    prompt
}

/// The user prompt for a campaign edit: the campaign as it is, the
/// request, and the change the user asked for.
fn campaign_edit_prompt(request: &SessionRequest, input: &EditInput<'_>) -> String {
    format!(
        "Here is the campaign to change:\n{campaign_json}\n{note}\
         The campaign is for this request:\n{request}\n\
         Apply this change: {critique}\n{findings}\
         A reference like [ad 3, node 0/1 <h2.title>: What changed] names an ad \
         (1-based) and one element in that ad's html by its index path from the ad root \
         (zero-based child indexes, element children only), its tag and first class, and the \
         start of its text. A reference like [ad 3, nodes 0/1 <h2>; 0/2 <p>] names several \
         elements of one ad the same way, without their text. A reference like [ad 3] \
         names the ad alone: the change is about that ad. Change only what the critique asks for. Keep every other ad and \
         value as it is. Return every changed ad complete: html, css, and notes.\n{format}",
        campaign_json = input.artifact_json,
        note = input.note,
        request = request_input(request),
        critique = input.instruction.trim(),
        findings = findings_note(input.findings),
        format = crate::campaign_patch::PATCH_FORMAT
    )
}

/// The campaign as a focused edit sees it: the title, the theme, the
/// size, the ad count, and only the ads at `indexes`, each
/// with its index.
fn focused_campaign_json(
    campaign: &Campaign,
    indexes: &[usize],
    is_fresh: bool,
) -> Result<String, serde_json::Error> {
    let ads: Vec<serde_json::Value> = indexes
        .iter()
        .filter_map(|index| {
            campaign.ads.get(*index).map(|ad| {
                let ad = if is_fresh { fresh_ad(ad) } else { ad.clone() };
                serde_json::json!({ "index": index, "ad": ad })
            })
        })
        .collect();
    serde_json::to_string(&serde_json::json!({
        "title": campaign.title,
        "theme": campaign.theme,
        "size": campaign.size,
        "ad_count": campaign.ads.len(),
        "ads": ads,
    }))
}

/// The ad as a regenerate shows it: its notes, without its markup, so
/// the model writes it anew instead of tweaking it.
fn fresh_ad(ad: &Ad) -> Ad {
    Ad {
        html: String::new(),
        css: None,
        ..ad.clone()
    }
}

/// The user prompt for one campaign continuation chunk: the preview
/// campaign and the chunk's ads to add, as a patch of inserts.
fn campaign_continue_prompt(
    request: &SessionRequest,
    campaign: &Campaign,
    campaign_json: &str,
    chunk: ContinueChunk,
) -> String {
    let written = campaign.ads.len();
    let planned = campaign.outline.len();
    let first = chunk.first.max(written);
    let last = (first + chunk.count).min(planned);
    let next_titles: Vec<String> = campaign
        .outline
        .iter()
        .enumerate()
        .skip(first)
        .take(last.saturating_sub(first))
        .map(|(index, title)| format!("{}. {title}", index + 1))
        .collect();
    let mut prompt = format!(
        "Here is a campaign in progress: its theme, its size, its first {written} \
         ads, and `outline`, the ad titles of the complete campaign:\n{campaign_json}\n\
         The campaign is for this request:\n{}\n",
        request_input(request)
    );
    prompt.push_str(&format!(
        "Write {} ads: outline titles {} to {last} of {planned}, in order, one ad per \
         title:\n{}\n\
         Keep the theme. Match the existing ads in CSS style, font sizes, spacing, colors, \
         and visual language, so the campaign reads as one piece. Do not change or repeat \
         the existing ads.\n",
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
        "Reply with only a JSON patch that appends the new ads, not the whole campaign:\n\
         {{\"ads\":[{{\"index\":{written},\"insert\":true,\"ad\":{{\"html\":\"...\",\"css\":\"...\",\"notes\":\"...\"}}}}]}}\n\
         Give every new ad index {written} and insert true, in priority order. Each ad \
         carries html, css, and notes. Omit title, theme, size, outline, and the existing ads."
    ));
    prompt
}

/// Extracts and parses the campaign JSON from a model reply.
fn parse_campaign(content: &str) -> Result<Campaign, String> {
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
        .map_err(|error| format!("invalid campaign: {error}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use design_model::{ArtifactKind, WorkflowState};

    use super::{
        ad_count_note, apply_campaign_continuation, campaign_edit_prompt, campaign_system_prompt,
        continuation_ads, focused_campaign_json, parse_campaign, partial_campaign, placeholder_ad,
        shown_campaign, size_note,
    };
    use crate::campaigns::CampaignStore;
    use crate::designs::DesignStore;
    use crate::edit_focus::EditInput;
    use crate::events::ChangeNotifier;
    use crate::generation::{ContinueChunk, GenerationEngine, GenerationOutcome};
    use crate::model_client::LogSink;
    use crate::request::SessionRequest;
    use crate::sessions::{ChatMessage, NewSession, SessionStore};
    use crate::test_support::{
        FakeModelServer, SAMPLE_CAMPAIGN, low_effort_options, sample_campaign,
    };

    /// The planner reply that writes candidates.
    const WRITE_PLAN: &str = r#"{"reply":"Writing it now.","generate":true}"#;

    #[test]
    fn a_focused_campaign_edit_shows_only_the_named_ads_and_their_findings() {
        let campaign = sample_campaign();
        let focused = focused_campaign_json(&campaign, &[1], false).unwrap();
        let value: serde_json::Value = serde_json::from_str(&focused).unwrap();
        assert_eq!(value["ad_count"], campaign.ads.len());
        assert_eq!(value["size"], "medium_rectangle");
        assert_eq!(value["ads"].as_array().unwrap().len(), 1);
        assert_eq!(value["ads"][0]["index"], 1);
        let request = SessionRequest {
            request: "A launch ad.".to_owned(),
            kind: ArtifactKind::Campaign,
            answers: Vec::new(),
            options: low_effort_options(),
        };
        let findings = vec!["ads[1] p (0/2): overflow: shorten".to_owned()];
        let input = EditInput {
            instruction: "[ad 2, node 0/2 <p>: x] Fix the overflow.",
            artifact_json: &focused,
            note: "Only ad 2 is shown.\n",
            findings: &findings,
        };
        let prompt = campaign_edit_prompt(&request, &input);
        assert!(prompt.contains("Only ad 2 is shown."));
        assert!(prompt.contains("Chrome measured these layout problems"));
        assert!(prompt.contains("- ads[1] p (0/2): overflow: shorten"));
        assert!(prompt.contains("Apply this change: [ad 2, node 0/2 <p>: x] Fix the overflow."));
        assert!(!prompt.contains("slide"));
    }

    #[test]
    fn the_ad_count_and_the_size_hold_the_campaign_to_the_asked_shape() {
        assert_eq!(size_note(None), "");
        assert_eq!(size_note(Some("tall")), "");
        assert_eq!(
            size_note(Some("leaderboard")),
            "Lay the ads out on the leaderboard size: 728 by 90 px. Set `size` to `leaderboard`.\n"
        );
        assert_eq!(
            size_note(Some("skyscraper")),
            "Lay the ads out on the skyscraper size: 160 by 600 px. Set `size` to `skyscraper`.\n"
        );
        assert_eq!(ad_count_note(None, None), "");
        assert_eq!(ad_count_note(None, Some(1)), "");
        assert_eq!(
            ad_count_note(Some(2), None),
            "The user asked for 2 ads. Write exactly 2 ads.\n"
        );
        // A preview writes one ad, so the length goes to the outline.
        assert_eq!(
            ad_count_note(Some(2), Some(1)),
            "The user asked for 2 ads. Put exactly 2 titles in `outline`.\n"
        );
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    struct Stores {
        designs: DesignStore,
        campaigns: CampaignStore,
        sessions: SessionStore,
    }

    fn stores(directory: &tempfile::TempDir) -> Stores {
        Stores {
            designs: DesignStore::new(directory.path().join("designs")),
            campaigns: CampaignStore::new(directory.path().join("campaigns")),
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
        .with_campaigns(stores.campaigns.clone())
    }

    /// A fresh one-candidate, low-effort campaign session, still in
    /// intake.
    async fn campaign_session(sessions: &SessionStore) {
        sessions
            .create(
                NewSession::demo("report", "Launch", "A launch ad.")
                    .with_kind(ArtifactKind::Campaign)
                    .with_options(low_effort_options()),
            )
            .await
            .unwrap();
    }

    /// A campaign session past its setup card: the app's own questions
    /// were asked, so the next planner turn is free to write.
    async fn set_up_campaign_session(sessions: &SessionStore) {
        campaign_session(sessions).await;
        sessions
            .apply("report", design_model::WorkflowEvent::QuestionsAsked)
            .await
            .unwrap();
    }

    #[test]
    fn campaign_system_prompt_carries_campaign_rules_the_schema_and_the_example() {
        let prompt = campaign_system_prompt();
        assert!(prompt.contains("display units read on a page"));
        assert!(prompt.contains("300 by 250 px for medium_rectangle"));
        assert!(prompt.contains("\"ads\""));
        assert!(prompt.contains("\"size\""));
        assert!(prompt.contains("Swift Design launch ads"));
        assert!(prompt.contains("needs_clarification"));
        assert!(!prompt.contains("\"viewport\""));
        assert!(!prompt.contains("\"slides\""));
    }

    #[test]
    fn partial_campaign_returns_complete_ads_only() {
        let text = r##"{"title":"T","theme":{"name":"m","colors":{"background":"#ffffff","text":"#1a1d21","accent":"#2f6fdd","muted":"#6b7480"},"fonts":{"heading":"Inter","body":"Inter","mono":"Inter"}},"size":"leaderboard","ads":[{"html":"<h1>One</h1>"},{"html":"<h1>Tw"##;
        let campaign = partial_campaign(text).unwrap();
        assert_eq!(campaign.ads.len(), 1);
        assert_eq!(campaign.size, design_model::AdSize::Leaderboard);
        assert!(partial_campaign("{\"title\":\"T\"").is_none());
    }

    #[test]
    fn continuation_ads_reject_a_short_reply() {
        let mut preview = sample_campaign();
        preview.outline = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        assert!(apply_campaign_continuation(&preview, "{\"ads\":[]}").is_err());
        let patch = r#"{"ads":[{"index":2,"insert":true,"ad":{"html":"<h2>C</h2>"}}]}"#;
        let continued = apply_campaign_continuation(&preview, patch).unwrap();
        assert_eq!(continued.ads.len(), 3);
        assert!(continued.outline.is_empty());
        assert_eq!(continuation_ads(2, patch).unwrap().len(), 1);
        assert!(continuation_ads(2, "{\"slides\":[]}").is_err());
        assert!(parse_campaign("no json").is_err());
    }

    #[test]
    fn shown_campaigns_pad_earlier_chunks_with_placeholders() {
        let mut preview = sample_campaign();
        preview.outline = (1..=4).map(|number| format!("Ad {number}")).collect();
        let chunks = [
            ContinueChunk { first: 2, count: 1 },
            ContinueChunk { first: 3, count: 1 },
        ];
        let board = vec![Vec::new(), vec![placeholder_ad("x")]];
        let shown = shown_campaign(&preview, &chunks, &board);
        assert_eq!(shown.ads.len(), 4);
        assert!(shown.ads[2].html.contains("Ad 3"));
        assert!(shown.validate().is_empty());
    }

    #[tokio::test]
    async fn a_campaign_run_asks_the_apps_own_questions_before_it_writes() {
        let server = FakeModelServer::start().await;
        // The planner wants to write at once and asks nothing.
        server.push_text(WRITE_PLAN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        campaign_session(&stores.sessions).await;
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
    async fn a_valid_campaign_reply_is_saved_as_a_candidate() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_CAMPAIGN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_campaign_session(&stores.sessions).await;
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        assert!(
            stores
                .campaigns
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
        assert!(planner.contains("You plan ads"));
        let request = server.requests()[1].to_string();
        assert!(request.contains("campaigns as JSON campaigns"));
        assert!(request.contains("Build a campaign"));
        let runs = stores.sessions.runs("report").await.unwrap();
        assert_eq!(runs[0].artifacts, vec!["report-candidate-1"]);
    }

    #[tokio::test]
    async fn a_chat_request_with_a_campaign_open_patches_that_campaign() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Tightening the title.","edit":true}"#);
        server.push_text(
            r#"{"ads":[{"index":0,"ad":{"html":"<h1 class='title'>Tighter</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        stores
            .campaigns
            .save("report-candidate-1", &sample_campaign())
            .await
            .unwrap();
        campaign_session(&stores.sessions).await;
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
            .campaigns
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.ads[0].html.contains("Tighter"));
        assert_eq!(
            stores.sessions.read("report").await.unwrap().unwrap().state,
            WorkflowState::Generating
        );
    }

    /// A reviewing campaign session with `count` saved candidates.
    async fn reviewing_campaign_session_with(stores: &Stores, count: usize) {
        for number in 1..=count {
            stores
                .campaigns
                .save(&format!("report-candidate-{number}"), &sample_campaign())
                .await
                .unwrap();
        }
        set_up_campaign_session(&stores.sessions).await;
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
    async fn a_regenerated_ad_is_written_without_its_old_markup() {
        let server = FakeModelServer::start().await;
        // No planner turn: the request names its ad itself.
        server.push_text(
            r#"{"ads":[{"index":0,"ad":{"html":"<h1 class='title'>Fresh</h1>","css":".title{font-size:40px;}"}}]}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        let mut campaign = sample_campaign();
        campaign.ads[0].html = "<h1>Old title markup</h1>".to_owned();
        stores
            .campaigns
            .save("report-candidate-1", &campaign)
            .await
            .unwrap();
        reviewing_campaign_session_with(&stores, 0).await;
        stores
            .sessions
            .append_message(
                "report",
                ChatMessage::regenerate_request("[ad 1] Write this ad anew.", "report-candidate-1"),
            )
            .await
            .unwrap();
        let outcome = engine(&server, &stores)
            .run("report", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, GenerationOutcome::Wrote { .. }));
        let text = server.requests()[0].to_string();
        assert!(text.contains("Write ad 1 of"));
        assert!(!text.contains("Old title markup"));
        let edited = stores
            .campaigns
            .load("report-candidate-1")
            .await
            .unwrap()
            .unwrap();
        assert!(edited.ads[0].html.contains("Fresh"));
        assert_eq!(edited.ads.len(), campaign.ads.len());
    }

    #[tokio::test]
    async fn a_merge_of_two_pinned_campaigns_writes_a_new_one() {
        let server = FakeModelServer::start().await;
        server.push_text(r#"{"reply":"Merging the two.","merge":true}"#);
        server.push_text(SAMPLE_CAMPAIGN);
        // The polish round, when Chrome can measure: no change.
        server.push_text(r#"{"ads":[]}"#);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        reviewing_campaign_session_with(&stores, 2).await;
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
                .campaigns
                .load("report-candidate-3")
                .await
                .unwrap()
                .is_some()
        );
        let text = server.requests()[1].to_string();
        assert!(text.contains("Combine these candidates into one campaign"));
        assert!(text.contains("Candidate 2:"));
        assert!(!text.contains("This is candidate"));
    }

    #[tokio::test]
    async fn a_campaign_session_without_a_campaign_store_fails_plainly() {
        let server = FakeModelServer::start().await;
        server.push_text(WRITE_PLAN);
        server.push_text(SAMPLE_CAMPAIGN);
        let directory = tempfile::tempdir().unwrap();
        let stores = stores(&directory);
        set_up_campaign_session(&stores.sessions).await;
        let engine = GenerationEngine::new(
            server.configuration(),
            stores.designs.clone(),
            stores.sessions.clone(),
            None,
            "http://127.0.0.1:3000".to_owned(),
            ChangeNotifier::new(),
        );
        let error = engine.run("report", silent_log()).await.unwrap_err();
        assert!(error.contains("no campaign store"));
    }
}
