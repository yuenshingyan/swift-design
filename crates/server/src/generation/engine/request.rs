use super::*;

impl GenerationEngine {
    /// The server's own URL, for relative image paths in screenshots.
    pub(crate) fn base_url(&self) -> String {
        self.address.clone()
    }

    /// Asks the model for one candidate, repairs it through fix rounds
    /// until it validates, and polishes it. The design is saved under
    /// `request.design_id` while it streams in, when the draft validates,
    /// and once more after the polish.
    pub(super) async fn generate_candidate(
        &self,
        client: &reqwest::Client,
        request: &CandidateRequest<'_>,
        attachments: &Attachments,
        progress: &ShareSink,
        log: &LogSink,
    ) -> Result<Design, GenerationStop> {
        let messages = vec![
            serde_json::json!({ "role": "system", "content": system_prompt() }),
            self.user_message(&candidate_prompt(request), attachments),
        ];
        let saver = LiveSaver::new(self, &request.design_id);
        let live_saver = saver.clone();
        let needs_app_window = request.context.options.should_have_app_window();
        let context = ArtifactRequest {
            effort: request.context.effort().to_owned(),
            label: format!("candidate {}", request.candidate_number),
            parse: Box::new(move |content| {
                let design = parse_design(content)?;
                if needs_app_window && design.app_window.is_none() {
                    return Err("This app candidate needs shared app_window bounds. Mark one top-level data-swift-design-window shell in every screen.".to_owned());
                }
                Ok(design)
            }),
            progress: Some(Arc::clone(progress)),
            live: Some(Arc::new(move |text: &str| {
                if let Some(design) = partial_design(text)
                    && (!needs_app_window || design.app_window.is_some())
                {
                    let rank = design.screens.len();
                    live_saver.offer(design, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        saver.offer(draft.clone(), draft.screens.len());
        let polished = match self.polish_design(client, draft, &context, log).await {
            Ok(design) => design,
            Err(error) => {
                saver.abort(None).await.map_err(GenerationStop::Failed)?;
                return Err(GenerationStop::Failed(error));
            }
        };
        saver
            .finish(&polished)
            .await
            .map_err(GenerationStop::Failed)?;
        self.designs
            .clear_user_paths(&request.design_id)
            .await
            .map_err(|error| GenerationStop::Failed(error.to_string()))?;
        Ok(polished)
    }

    /// Sends `messages`, parses the artifact reply, and repairs it through
    /// fix rounds until it validates. A reply that asks for a blocking
    /// detail stops the run with a clarification.
    pub(crate) async fn request_valid<T: Validated>(
        &self,
        client: &reqwest::Client,
        mut messages: Vec<serde_json::Value>,
        context: &ArtifactRequest<'_, T>,
        log: &LogSink,
    ) -> Result<T, GenerationStop> {
        let label = &context.label;
        let fix_round_limit = fix_round_limit(&context.effort);
        let effort = writing_effort(&context.effort);
        for round in 0..=fix_round_limit {
            log(&format!("{label}: requesting (round {})", round + 1));
            let started = std::time::Instant::now();
            let content = self
                .model
                .chat_with(
                    client,
                    self.model.request_body(&messages, effort),
                    context.live.as_deref(),
                )
                .await?;
            log(&format!(
                "{label}: reply in {:.0}s ({} chars)",
                started.elapsed().as_secs_f32(),
                content.len()
            ));
            // A valid clarification request stops the run at once.
            if let Some(result) = clarification_request(&content) {
                match result {
                    Ok(set) => {
                        log(&format!("{label}: the model asked for clarification"));
                        return Err(GenerationStop::NeedsClarification(set));
                    }
                    Err(problems) => {
                        log(&format!(
                            "{label}: the clarification request was invalid ({} problems); retrying",
                            problems.len()
                        ));
                    }
                }
            }
            match (context.parse)(&content) {
                Ok(artifact) => {
                    let errors = artifact.problems();
                    if errors.is_empty() {
                        context.report(DRAFT_SHARE);
                        return Ok(artifact);
                    }
                    let error_lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                    log(&format!("{label}: {} validation errors", error_lines.len()));
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    }));
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "The result failed validation:\n{}\n\
                             Fix every error. Reply with only the corrected JSON in the same format as before.",
                            error_lines.join("\n")
                        ),
                    }));
                }
                Err(parse_error) => {
                    log(&format!("{label}: {parse_error}"));
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    }));
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": format!(
                            "That reply could not be applied ({parse_error}). \
                             Reply with only the JSON in the same format as before.",
                        ),
                    }));
                }
            }
        }
        Err(GenerationStop::Failed(format!(
            "{label} still fails after {fix_round_limit} fix rounds"
        )))
    }

    /// Reports transient provider retries to the active run.
    pub fn with_retry_log(mut self, log: LogSink) -> Self {
        self.model = self.model.with_retry_log(log);
        self
    }

    /// Reports each request's token usage to `sink`.
    pub fn with_usage_sink(mut self, sink: UsageSink) -> Self {
        self.model = self.model.with_usage_sink(sink);
        self
    }

    /// Reports how far each turn is to `sink`, 0 to 100.
    pub fn with_progress_sink(mut self, sink: ProgressSink) -> Self {
        self.progress_sink = Some(sink);
        self
    }

    /// Reports how far each design a turn writes is, by design id.
    pub fn with_design_progress_sink(mut self, sink: DesignProgressSink) -> Self {
        self.design_progress_sink = Some(sink);
        self
    }

    /// Reports the turn's progress, when a sink is set.
    pub(crate) fn report_progress(&self, percent: u8) {
        if let Some(sink) = &self.progress_sink {
            sink(percent.min(100));
        }
    }

    /// One share sink per design in `design_ids`, for designs written at the
    /// same time. Each design reports its own 0.0 to 1.0: the design sink
    /// gets it as a percent under its id, and the turn progress becomes
    /// `base` plus `span` times the mean of all shares.
    /// A progress group that takes artifacts as they start: each share
    /// reports its own percentage, and the run's bar shows the mean.
    pub(crate) fn progress_group(&self, base: u8, span: u8) -> ProgressGroup {
        ProgressGroup {
            shares: Arc::new(std::sync::Mutex::new(Vec::new())),
            sink: self.progress_sink.clone(),
            design_sink: self.design_progress_sink.clone(),
            base,
            span,
        }
    }

    pub(crate) fn shared_progress(
        &self,
        design_ids: &[String],
        base: u8,
        span: u8,
    ) -> Vec<ShareSink> {
        let count = design_ids.len().max(1);
        let shares = Arc::new(std::sync::Mutex::new(vec![0.0f32; count]));
        (0..count)
            .map(|index| {
                let shares = Arc::clone(&shares);
                let sink = self.progress_sink.clone();
                let design_sink = self.design_progress_sink.clone();
                let design_id = design_ids.get(index).cloned().unwrap_or_default();
                let share: ShareSink = Arc::new(move |fraction: f32| {
                    let fraction = fraction.clamp(0.0, 1.0);
                    if let Some(design_sink) = &design_sink {
                        design_sink(&design_id, (fraction * 100.0) as u8);
                    }
                    let Ok(mut shares) = shares.lock() else {
                        return;
                    };
                    shares[index] = fraction;
                    let mean = shares.iter().sum::<f32>() / shares.len() as f32;
                    if let Some(sink) = &sink {
                        sink(base.saturating_add((f32::from(span) * mean) as u8));
                    }
                });
                share
            })
            .collect()
    }
}
