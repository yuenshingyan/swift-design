use super::*;

use crate::design::generation::live_preview::{PreviewBoard, worker_sink};
use crate::parallel_edit::PARALLEL_EDIT_LIMIT;

impl GenerationEngine {
    /// Writes the remaining screens of the preview design `design_id` in
    /// chunks of `CONTINUE_CHUNK_SCREENS`. A separate visual draft shows
    /// progress while the saved preview remains protected until acceptance.
    /// Returns how many screens were added; 0 when the design is complete already.
    pub(crate) async fn continue_design(
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
        let live_preview = self.designs.start_preview(design_id, &self.notifier, None);
        let completed: ChunkBoard = Arc::new(std::sync::Mutex::new(
            chunks.iter().map(|_| Vec::new()).collect(),
        ));
        let preview = design.clone();
        let show = {
            let board = Arc::clone(&board);
            let live_preview = live_preview.clone();
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
                live_preview.show(&shown_design(&preview, &chunks, &board));
            })
        };
        let mut tasks = tokio::task::JoinSet::new();
        for (position, chunk) in chunks.iter().enumerate() {
            let engine = self.clone();
            let client = client.clone();
            let context = context.clone();
            let preview = preview.clone();
            let board = Arc::clone(&board);
            let completed = Arc::clone(&completed);
            let live_preview = live_preview.clone();
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
                        if !live_preview.should_update() {
                            return;
                        }
                        let screens =
                            crate::design::generation::live_preview::preview_continuation_screens(
                                written, text,
                            )
                            .unwrap_or_else(|| partial_continuation_screens(written, text));
                        if let Ok(mut board) = live_board.lock()
                            && !screens.is_empty()
                            && screens != board[position]
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
                    board[position] = screens.clone();
                }
                if let Ok(mut completed) = completed.lock() {
                    completed[position] = screens;
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
        if !failures.is_empty() {
            let reason = failure_message(&failures, "the continuation could not finish");
            let fallback = completed
                .lock()
                .ok()
                .filter(|board| board.iter().any(|screens| !screens.is_empty()))
                .map(|board| shown_design(&preview, &chunks, &board));
            return match fallback {
                Some(fallback) => Err(self
                    .save_review_draft(design_id, &fallback, &reason, false)
                    .await?),
                None => Err(reason),
            };
        }
        let mut continued = preview.clone();
        if let Ok(board) = completed.lock() {
            for screens in board.iter() {
                continued.screens.extend(screens.iter().cloned());
            }
        }
        let added = continued.screens.len().saturating_sub(start);
        if added == 0 {
            // No accepted chunk changed the original preview.
            return Err(failure_message(&failures, "no screens were added"));
        }
        // A short reply leaves the design continuable: the outline stays
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
            live: Some(live_preview.edit_sink(&continued)),
        };
        let fallback = continued.clone();
        let final_design = match async {
            design_model::markup::validate_demo_navigation(&continued, Some(&preview))?;
            self.polish_design(client, continued, &polish_context, log)
                .await
        }
        .await
        {
            Ok(design) => design,
            Err(error) => {
                saver.abort(Some(&preview)).await?;
                return Err(self
                    .save_review_draft(design_id, &fallback, &error, false)
                    .await?);
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
            let live = context.live.as_ref().map(|sink| {
                crate::design::generation::live_preview::patch_sink(&design, Arc::clone(sink))
            });
            let started = std::time::Instant::now();
            let content = self
                .model
                .chat_with(
                    client,
                    self.model
                        .request_body(&messages, writing_effort(&context.effort)),
                    live.as_deref(),
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
        let final_audit = self.audit_demo(&best).await?;
        final_audit.require_layout_clean()?;
        let warnings = final_audit.instructions(None);
        if warnings.is_empty() {
            log(&format!(
                "{label}: final layout verified after {rounds_taken} tweaking round(s)"
            ));
        } else {
            log(&format!(
                "{label}: Needs review — {} advisory findings remain: {}",
                warnings.len(),
                warnings.join("; ")
            ));
        }
        context.report(1.0);
        Ok(best)
    }

    /// Measures the touched screens of an edited design and asks the
    /// model to fix what Chrome finds, round after round: until the
    /// screens measure clean, a round does not help, or the effort's
    /// round limit runs out. Returns the best exact version and its audit so
    /// callers can distinguish completion from a draft that needs review.
    pub(crate) async fn fix_edited_design(
        &self,
        client: &reqwest::Client,
        mut design: Design,
        fix: &EditFix<'_, Design>,
        log: &LogSink,
    ) -> Result<(Design, DemoAudit), String> {
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
                "{label}: tweak round {round} of at most {limit} ({} findings, up to {PARALLEL_EDIT_LIMIT} concurrent screen repairs)",
                findings.len()
            ));
            design = self
                .repair_demo_screens(client, &design, fix, &audit)
                .await
                .map_err(|error| {
                    log(&format!("{label}: fix round {round} failed: {error}"));
                    format!(
                        "The layout repair could not finish: {error}. No verified final version was saved. Remaining layout findings: {}",
                        findings.join("; ")
                    )
                })?;
            fix.context.report(
                (DRAFT_SHARE + (1.0 - DRAFT_SHARE) * round as f32 / limit as f32).min(0.98),
            );
        }
        log(&format!(
            "{label}: {}",
            stop.describe(rounds_taken, best_score.0)
        ));
        let final_audit = self.audit_demo(&best).await?;
        let unedited_errors = final_audit.score(None).0;
        if unedited_errors > 0 {
            log(&format!(
                "{label}: {unedited_errors} layout finding(s) remain on unedited screens; the verification covers only the touched screens"
            ));
        }
        log(&format!(
            "{label}: final layout measured after {rounds_taken} tweaking round(s) on the touched screens"
        ));
        fix.context.report(1.0);
        Ok((best, final_audit))
    }

    /// Repairs independent screens from one measured baseline; the caller re-audits the merge.
    async fn repair_demo_screens(
        &self,
        client: &reqwest::Client,
        design: &Design,
        fix: &EditFix<'_, Design>,
        audit: &DemoAudit,
    ) -> Result<Design, String> {
        let targets: Vec<_> = (0..design.screens.len())
            .filter(|index| fix.indexes.contains(index))
            .filter_map(|index| {
                let findings = audit.layout_instructions(&[index]);
                (!findings.is_empty()).then_some((index, findings))
            })
            .collect();
        let board = PreviewBoard::default();
        let mut merged = design.clone();
        for batch in targets.chunks(PARALLEL_EDIT_LIMIT) {
            let mut workers = tokio::task::JoinSet::new();
            for (index, findings) in batch {
                let index = *index;
                let artifact_json = focused_design_json(design, &[index], false)
                    .map_err(|error| error.to_string())?;
                let instruction = format!(
                    "{} Work only on the assigned screen; preserve all shared settings.",
                    fix_instruction("screens")
                );
                let input = EditInput {
                    instruction: &instruction,
                    artifact_json: &artifact_json,
                    note: &focus_note("screen", "screens", &[index], design.screens.len()),
                    findings,
                    conversation: "",
                };
                let messages = [
                    serde_json::json!({ "role": "system", "content": system_prompt() }),
                    serde_json::json!({ "role": "user", "content": edit_prompt(fix.request, &input) }),
                ];
                let model = self.model.clone();
                let body = model.request_body(&messages, writing_effort(&fix.context.effort));
                let client = client.clone();
                let live = fix.context.live.as_ref().map(|sink| {
                    worker_sink(design.clone(), index, Arc::clone(&board), Arc::clone(sink))
                });
                workers.spawn(async move {
                    (index, model.chat_with(&client, body, live.as_deref()).await)
                });
            }
            while let Some(result) = workers.join_next().await {
                let (index, reply) =
                    result.map_err(|error| format!("layout worker stopped: {error}"))?;
                let content = reply.map_err(|error| format!("screen {}: {error}", index + 1))?;
                match apply_tweak(design, &content, Some(&[index])) {
                    Ok(improved)
                        if improved.title == design.title && improved.theme == design.theme =>
                    {
                        merged.screens[index] = improved.screens[index].clone();
                    }
                    Ok(_) => tracing::warn!(
                        screen = index + 1,
                        "tweak tried to change shared settings; retaining the previous screen"
                    ),
                    Err(error) => {
                        tracing::warn!(screen = index + 1, %error, "tweak reply unusable; retaining the previous screen")
                    }
                }
            }
        }
        design_model::markup::validate_demo_navigation(&merged, Some(design))?;
        if let Some(sink) = &fix.context.live
            && let Ok(text) = serde_json::to_string(&merged)
        {
            sink(&text);
        }
        Ok(merged)
    }

    /// PNG screenshots of the design's screens for the polish pass, at most
    /// `POLISH_IMAGE_LIMIT`. Empty when the model cannot see images or
    /// no Chrome is installed; a failed screenshot is logged and skipped.
    pub(crate) async fn screen_images(
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
