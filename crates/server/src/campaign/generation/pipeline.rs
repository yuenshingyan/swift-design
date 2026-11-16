use super::*;

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
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for campaign_id in trailing_continue_ids(&messages) {
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
                conversation,
            } => {
                let order = EditOrder {
                    artifact_ids: &designs,
                    instruction: &instruction,
                    is_fresh: false,
                    conversation: &conversation,
                };
                let edits = self.edit_campaigns(client, context, &order, log).await?;
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
                let edits = self.edit_campaigns(client, context, &order, log).await?;
                Ok(GenerationOutcome::edited(edits))
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
    ) -> Result<Vec<(String, String)>, GenerationStop> {
        let mut saved = Vec::new();
        let mut last_error = None;
        for campaign_id in order.artifact_ids {
            match self
                .edit_campaign(client, context, campaign_id, order, log)
                .await
            {
                Ok(report) => saved.push((campaign_id.clone(), report)),
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
    ) -> Result<String, GenerationStop> {
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
        let measured = dom_findings(&campaign, &self.base_url(), &label, log).await;
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
            conversation: order.conversation,
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
            parse: Box::new(move |content| apply_patch(&original, parse_patch(content)?)),
            progress: self.shared_progress(&[campaign_id.to_owned()], 5, 95).pop(),
            live: None,
        };
        let edited = self
            .request_edit(
                client,
                EditExecution {
                    original: &campaign,
                    messages,
                    input: &input,
                    indexes: &indexes,
                    is_fresh: order.is_fresh,
                    request: &request,
                },
                log,
            )
            .await?;
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
        let report = saved_edit_report(
            campaign_id,
            context.request.kind,
            &campaign,
            &final_campaign,
        )
        .map_err(GenerationStop::Failed)?;
        campaigns
            .save(campaign_id, &final_campaign)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        self.notifier.notify();
        log(&format!("edit {campaign_id}: saved"));
        Ok(report)
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
        if !can_audit() {
            log(&format!(
                "{label}: {}",
                PolishStop::NotMeasured.describe(0, 0)
            ));
            context.report(1.0);
            return Ok(campaign);
        }
        let limit = polish_round_limit(&context.effort);
        // `limit` is at least 1, so the loop always measures once and
        // `best_count` is always set before it is read.
        let mut best = campaign.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let findings = dom_findings(&campaign, &self.base_url(), label, log).await;
            if findings.len() < best_count {
                best_count = findings.len();
                best = campaign.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a campaign that is already good.
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
            let images = self.ad_images(&campaign, label, log).await;
            log(&format!(
                "{label}: polish round {round} of at most {limit} ({} layout findings, {} ad images)",
                findings.len(),
                images.len()
            ));
            let campaign_json =
                serde_json::to_string(&campaign).map_err(|error| error.to_string())?;
            let prompt = polish_prompt(&campaign_json, &findings, images.len());
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
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&campaign, patch));
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
        if fix.indexes.is_empty() || !can_audit() {
            fix.context.report(1.0);
            return Ok(campaign);
        }
        let limit = polish_round_limit(&fix.context.effort);
        let mut best = campaign.clone();
        let mut best_count = usize::MAX;
        let mut previous_count: Option<usize> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 1..=limit {
            let measured = dom_findings(&campaign, &self.base_url(), label, log).await;
            let findings = findings_for(&measured, "ads", &fix.indexes);
            if findings.len() < best_count {
                best_count = findings.len();
                best = campaign.clone();
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
                conversation: "",
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
            let improved = parse_patch(&content).and_then(|patch| apply_patch(&campaign, patch));
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
        if !supports_vision(self.model.model()) {
            return Vec::new();
        }
        if find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for ad images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = campaign.ads.len().min(POLISH_IMAGE_LIMIT);
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let campaign = campaign.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = screenshot_ad(&campaign, index, &base_url).await;
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
