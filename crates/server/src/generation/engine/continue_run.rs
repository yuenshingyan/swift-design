use super::*;

impl GenerationEngine {
    /// Continues every requested preview at the same time, and every
    /// preview whose Finish is pressed while they run. Returns one
    /// outcome per artifact, in start order: the screens added, or the
    /// error.
    ///
    /// A late Finish would otherwise wait for the first continue to end.
    /// The loop wakes on every store change and on a timer, reads the
    /// trailing continue requests again, and starts the new ones.
    pub(crate) async fn continue_artifacts(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        ids: Vec<String>,
        log: &LogSink,
    ) -> Vec<(String, Result<usize, String>)> {
        let shared = ContinueShared {
            client,
            context,
            attachments: Arc::new(self.load_attachments(&context.session_id, log).await),
            group: self.progress_group(5, 95),
            log,
        };
        let mut batch = ContinueBatch::default();
        let mut outcomes: Vec<(String, Result<usize, String>)> = Vec::new();
        let mut changes = self.notifier.subscribe();
        self.start_continues(&mut batch, ids, &shared);
        loop {
            if batch.tasks.is_empty() {
                let late = self.late_continue_ids(context, &batch.started).await;
                if late.is_empty() {
                    break;
                }
                self.start_continues(&mut batch, late, &shared);
                continue;
            }
            tokio::select! {
                joined = batch.tasks.join_next() => match joined {
                    Some(Ok((id, outcome))) => outcomes.push((id, outcome)),
                    Some(Err(error)) => log(&format!("continue task failed: {error}")),
                    None => {}
                },
                changed = changes.changed() => {
                    if changed.is_err() {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
                () = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
            let late = self.late_continue_ids(context, &batch.started).await;
            self.start_continues(&mut batch, late, &shared);
        }
        outcomes.sort_by_key(|(id, _)| batch.started.iter().position(|known| known == id));
        outcomes
    }

    /// Starts a continue for each id and records it as started.
    pub(super) fn start_continues(
        &self,
        batch: &mut ContinueBatch,
        ids: Vec<String>,
        shared: &ContinueShared<'_>,
    ) {
        if ids.is_empty() {
            return;
        }
        if !batch.started.is_empty() {
            (shared.log)(&format!(
                "continuing {} more: {}",
                ids.len(),
                ids.join(", ")
            ));
        }
        for id in ids {
            let share = shared.group.share(&id);
            self.spawn_continue(&mut batch.tasks, &id, share, shared);
            batch.started.push(id);
        }
    }

    /// The trailing continue requests that are not running yet: a
    /// Finish pressed after this turn read its task.
    pub(crate) async fn late_continue_ids(
        &self,
        context: &GenerationContext,
        started: &[String],
    ) -> Vec<String> {
        match self.pick_task(context).await {
            Ok(Some(GenerationTask::Continue(ids))) => {
                ids.into_iter().filter(|id| !started.contains(id)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Starts one continue on the session's kind of artifact.
    pub(super) fn spawn_continue(
        &self,
        tasks: &mut tokio::task::JoinSet<(String, Result<usize, String>)>,
        id: &str,
        share: ShareSink,
        shared: &ContinueShared<'_>,
    ) {
        let engine = self.clone();
        let client = shared.client.clone();
        let context = shared.context.clone();
        let id = id.to_owned();
        let attachments = Arc::clone(&shared.attachments);
        let log = Arc::clone(shared.log);
        tasks.spawn(async move {
            let outcome = match context.request.kind {
                design_model::ArtifactKind::Demo => {
                    engine
                        .continue_design(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Deck => {
                    engine
                        .continue_deck(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Document => {
                    engine
                        .continue_document(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Social => {
                    engine
                        .continue_social(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Print => {
                    engine
                        .continue_print(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Mailing => {
                    engine
                        .continue_mailing(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Campaign => {
                    engine
                        .continue_campaign(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
                design_model::ArtifactKind::Artwork => {
                    engine
                        .continue_artwork(&client, &context, &id, &attachments, &share, &log)
                        .await
                }
            };
            (id, outcome)
        });
    }

    /// Writes the remaining screens of the preview design `design_id` in
    /// chunks of `CONTINUE_CHUNK_SCREENS`. The design is saved after every
    /// chunk, so the canvas shows it grow, then polished once it is
    /// complete. Returns how many screens were added; 0 when the design is
    /// complete already.
    pub(super) async fn continue_design(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        design_id: &str,
        attachments: &Arc<Attachments>,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<usize, String> {
        let mut design = self
            .designs
            .load(design_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("design `{design_id}` does not exist"))?;
        // A run that stopped may have left placeholder screens behind.
        // They hold no content, so they do not count as written.
        design.screens.retain(|screen| !is_pending_screen(screen));
        if !design.is_preview() {
            log(&format!(
                "continue {design_id}: the design is complete already"
            ));
            return Ok(0);
        }
        let label = format!("continue {design_id}");
        let start = design.screens.len();
        let planned = design.outline.len();
        let chunks = continue_chunks(start, planned);
        log(&format!(
            "{label}: {start} of {planned} screens written; writing {} more in {} chunks",
            planned - start,
            chunks.len()
        ));
        // The card shows `writing` from the first moment, not from the
        // first chunk: a chunk takes a minute or more.
        progress(0.0);
        // Every chunk runs at the same time from the same preview. The
        // board keeps what each chunk has written so far; `shown_design`
        // turns the board into the design the canvas shows.
        let board: ChunkBoard = Arc::new(std::sync::Mutex::new(
            chunks.iter().map(|_| Vec::new()).collect(),
        ));
        let saver = LiveSaver::new(self, design_id);
        let preview = design.clone();
        let show = {
            let board = Arc::clone(&board);
            let saver = saver.clone();
            let preview = preview.clone();
            let progress = Arc::clone(progress);
            let chunks = chunks.clone();
            Arc::new(move || {
                let Ok(board) = board.lock() else {
                    return;
                };
                let written: usize = board.iter().map(Vec::len).sum();
                let done = written as f32 / (planned - start).max(1) as f32;
                progress(CONTINUE_DRAFT_SHARE * done);
                saver.offer(shown_design(&preview, &chunks, &board), written);
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
            let label = label.clone();
            let chunk = *chunk;
            let attachments = Arc::clone(attachments);
            tasks.spawn(async move {
                let design_json =
                    serde_json::to_string(&preview).map_err(|error| error.to_string())?;
                let messages = vec![
                    serde_json::json!({ "role": "system", "content": system_prompt() }),
                    engine.user_message(
                        &continue_prompt(&context.request, &preview, &design_json, chunk),
                        &attachments,
                    ),
                ];
                let original = preview.clone();
                let written = preview.screens.len();
                let live_board = Arc::clone(&board);
                let live_show = Arc::clone(&show);
                let request = ArtifactRequest {
                    effort: context.effort().to_owned(),
                    label: format!("{label} chunk {}", position + 1),
                    parse: Box::new(move |content| apply_continuation(&original, content)),
                    progress: None,
                    live: Some(Arc::new(move |text: &str| {
                        let screens = partial_continuation_screens(written, text);
                        if let Ok(mut board) = live_board.lock()
                            && screens.len() > board[position].len()
                        {
                            board[position] = screens;
                        } else {
                            return;
                        }
                        live_show();
                    })),
                };
                let continued = engine
                    .request_valid(&client, messages, &request, &log)
                    .await
                    .map_err(stop_to_string)?;
                let screens: Vec<design_model::Screen> = continued.screens[written..].to_vec();
                if let Ok(mut board) = board.lock() {
                    board[position] = screens;
                }
                show();
                Ok::<(), String>(())
            });
        }
        let mut failures = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(format!("chunk task failed: {error}")),
            }
        }
        for failure in &failures {
            log(&format!("{label}: {failure}"));
        }
        let mut continued = preview.clone();
        if let Ok(board) = board.lock() {
            for screens in board.iter() {
                continued.screens.extend(screens.iter().cloned());
            }
        }
        let added = continued.screens.len().saturating_sub(start);
        if added == 0 {
            // The board only held placeholders; put the preview back so
            // the design stays continuable.
            if let Err(error) = saver.finish(&preview).await {
                log(&format!("{label}: restoring the preview failed: {error}"));
            }
            return Err(failure_message(&failures, "no screens were added"));
        }
        // A failed chunk leaves the design continuable: the outline stays
        // until every title has a screen.
        if continued.screens.len() >= planned {
            continued.outline.clear();
        }
        // Keep this as a streaming draft until the exact final version passes the audit.
        // The polish rounds fill the last share.
        let share = Arc::clone(progress);
        let polish_context = ArtifactRequest {
            effort: context.effort().to_owned(),
            label: label.clone(),
            parse: Box::new(parse_design),
            progress: Some(Arc::new(move |fraction: f32| {
                let polished = ((fraction - DRAFT_SHARE) / (1.0 - DRAFT_SHARE)).clamp(0.0, 1.0);
                share(CONTINUE_DRAFT_SHARE + (1.0 - CONTINUE_DRAFT_SHARE) * polished);
            })),
            live: None,
        };
        let final_design = match self
            .polish_design(client, continued, &polish_context, log)
            .await
        {
            Ok(design) => design,
            Err(error) => {
                saver.abort(Some(&preview)).await?;
                return Err(error);
            }
        };
        saver.finish(&final_design).await?;
        progress(1.0);
        log(&format!("{label}: saved with {added} new screens"));
        Ok(added)
    }

    /// Tweaks a rendered design within the effort budget and verifies the final version.
    /// Unavailable audits and unresolved hard layout errors block completion.
    pub(crate) async fn polish_design(
        &self,
        client: &reqwest::Client,
        mut design: Design,
        context: &ArtifactRequest<'_, Design>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &context.label;
        let limit = polish_round_limit(&context.effort);
        // Hard layout errors outrank cosmetic warnings when choosing a fallback.
        let mut best = design.clone();
        let mut best_score = (usize::MAX, usize::MAX);
        let mut previous_score: Option<(usize, usize)> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        // The extra measurement evaluates the last allowed repair, not its predecessor.
        for round in 0..=limit {
            log(&format!(
                "{label}: checking layout before tweak {}",
                round + 1
            ));
            let audit = self.audit_demo(&design).await?;
            let findings = audit.instructions(None);
            let score = audit.score(None);
            if score < best_score {
                best_score = score;
                best = design.clone();
            }
            // Nothing measures wrong: another round would spend a model
            // call to change a page that is already good.
            if findings.is_empty() {
                stop = PolishStop::Clean;
                break;
            }
            // The last round did not reduce the findings, so the next
            // will not either. This also breaks a fix-one-break-one
            // loop that would otherwise run to the limit.
            if previous_score.is_some_and(|before| score >= before) {
                stop = PolishStop::NoImprovement;
                break;
            }
            if round == limit {
                break;
            }
            previous_score = Some(score);
            let round = round + 1;
            rounds_taken = round;
            let images = self.screen_images(&design, label, log).await;
            log(&format!(
                "{label}: tweak round {round} of at most {limit} ({} layout findings, {} screen images)",
                findings.len(),
                images.len()
            ));
            let design_json = serde_json::to_string(&design).map_err(|error| error.to_string())?;
            let prompt = polish_prompt(&design_json, &findings, images.len());
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                serde_json::json!({
                    "role": "user",
                    "content": user_content_with_images(&prompt, &images),
                }),
            ];
            let started = std::time::Instant::now();
            let content = self
                .model
                .chat_with(
                    client,
                    self.model
                        .request_body(&messages, writing_effort(&context.effort)),
                    None,
                )
                .await?;
            log(&format!(
                "{label}: tweak reply in {:.0}s ({} chars)",
                started.elapsed().as_secs_f32(),
                content.len()
            ));
            match apply_tweak(&design, &content, None) {
                Ok(improved) => design = improved,
                Err(error) => log(&format!(
                    "{label}: tweak reply unusable ({error}); keeping the previous version"
                )),
            }
            context.report(
                (DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32).min(0.98),
            );
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_score.1)
        ));
        self.audit_demo(&best).await?.require_layout_clean()?;
        log(&format!(
            "{label}: final layout verified after {rounds_taken} tweaking round(s)"
        ));
        context.report(1.0);
        Ok(best)
    }

    /// Measures the touched screens of an edited design and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// screens measure clean, a round does not help, or the effort's
    /// round limit runs out. Verifies the best exact version before it can replace the saved design.
    pub(crate) async fn fix_edited_design(
        &self,
        client: &reqwest::Client,
        mut design: Design,
        fix: &EditFix<'_, Design>,
        log: &LogSink,
    ) -> Result<Design, String> {
        let label = &fix.context.label;
        let limit = polish_round_limit(&fix.context.effort);
        let mut best = design.clone();
        let mut best_score = (usize::MAX, usize::MAX);
        let mut previous_score: Option<(usize, usize)> = None;
        let mut stop = PolishStop::OutOfRounds;
        let mut rounds_taken = 0usize;
        for round in 0..=limit {
            log(&format!(
                "{label}: checking layout before tweak {}",
                round + 1
            ));
            let audit = self.audit_demo(&design).await?;
            let findings = audit.layout_instructions(&fix.indexes);
            let score = audit.score(Some(&fix.indexes));
            if score < best_score {
                best_score = score;
                best = design.clone();
            }
            if findings.is_empty() {
                stop = PolishStop::Clean;
                break;
            }
            if previous_score.is_some_and(|before| score >= before) {
                stop = PolishStop::NoImprovement;
                break;
            }
            if round == limit {
                break;
            }
            previous_score = Some(score);
            let round = round + 1;
            rounds_taken = round;
            log(&format!(
                "{label}: tweak round {round} of at most {limit} ({} findings on the touched screens)",
                findings.len()
            ));
            let design_json = focused_design_json(&design, &fix.indexes, false)
                .map_err(|error| error.to_string())?;
            let note = focus_note("screen", "screens", &fix.indexes, design.screens.len());
            let instruction = fix_instruction("screens");
            // A fix round repairs the layout; history is noise there.
            let input = EditInput {
                instruction: &instruction,
                artifact_json: &design_json,
                note: &note,
                findings: &findings,
                conversation: "",
            };
            let messages = vec![
                serde_json::json!({ "role": "system", "content": system_prompt() }),
                serde_json::json!({ "role": "user", "content": edit_prompt(fix.request, &input) }),
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
            // A failed repair must report its cause, not masquerade as exhausted layout rounds.
            let content = match reply {
                Ok(content) => content,
                Err(error) => {
                    log(&format!("{label}: fix round {round} failed: {error}"));
                    return Err(format!(
                        "The layout repair could not finish: {error}. No verified final version was saved. Remaining layout findings: {}",
                        findings.join("; ")
                    ));
                }
            };
            match apply_tweak(&design, &content, Some(&fix.indexes)) {
                Ok(improved) => design = improved,
                Err(error) => log(&format!(
                    "{label}: tweak reply unusable ({error}); keeping the previous version"
                )),
            }
            fix.context.report(
                (DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32).min(0.98),
            );
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_score.0)
        ));
        let final_audit = self.audit_demo(&best).await?;
        final_audit.require_layout_clean_on(Some(&fix.indexes))?;
        let unedited_errors = final_audit.score(None).0;
        if unedited_errors > 0 {
            log(&format!(
                "{label}: {unedited_errors} layout finding(s) remain on unedited screens; the verification covers only the touched screens"
            ));
        }
        log(&format!(
            "{label}: final layout verified after {rounds_taken} tweaking round(s) on the touched screens"
        ));
        fix.context.report(1.0);
        Ok(best)
    }

    /// PNG screenshots of the design's screens for the polish pass, at most
    /// `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images or
    /// no Chrome is installed; a failed screenshot is logged and skipped.
    pub(super) async fn screen_images(
        &self,
        design: &Design,
        label: &str,
        log: &LogSink,
    ) -> Vec<Vec<u8>> {
        if !supports_vision(self.model.model()) {
            return Vec::new();
        }
        if find_chrome().is_none() {
            log(&format!(
                "{label}: no Chrome found for screen images; reviewing from JSON only"
            ));
            return Vec::new();
        }
        let base_url = self.base_url();
        let count = design.screens.len().min(POLISH_IMAGE_LIMIT);
        // One Chrome per screen, all at once.
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let design = design.clone();
            let base_url = base_url.clone();
            tasks.spawn(async move {
                let shot = screenshot_screen(&design, index, &base_url).await;
                (index, shot)
            });
        }
        let mut images: Vec<Option<Vec<u8>>> = (0..count).map(|_| None).collect();
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok((index, Ok(bytes))) => images[index] = Some(bytes),
                Ok((index, Err(error))) => log(&format!(
                    "{label}: screen {} screenshot failed: {error}",
                    index + 1
                )),
                Err(error) => log(&format!("{label}: screenshot task failed: {error}")),
            }
        }
        images.into_iter().flatten().collect()
    }
}
