use super::*;

impl GenerationEngine {
    /// Asks the model for one candidate, repairs it through fix rounds
    /// until it validates, and polishes it. The design is saved under
    /// `request.design_id` while it streams in, when the draft validates,
    /// and once more after the polish.
    pub(crate) async fn generate_candidate(
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
        let preview = self
            .designs
            .start_preview(&request.design_id, &self.notifier, None);
        let live_preview = preview.clone();
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
                if live_preview.should_update()
                    && let Some(design) =
                        crate::design::generation::live_preview::preview_design(text)
                    && (!needs_app_window || design.app_window.is_some())
                {
                    live_preview.show(&design);
                }
                if let Some(design) = partial_design(text)
                    && (!needs_app_window || design.app_window.is_some())
                {
                    let rank = design.screens.len();
                    live_saver.offer(design, rank);
                }
            })),
        };
        let draft = self.request_valid(client, messages, &context, log).await?;
        preview.show(&draft);
        saver.offer(draft.clone(), draft.screens.len());
        let fallback = draft.clone();
        let polished = match self.polish_design(client, draft, &context, log).await {
            Ok(design) => design,
            Err(error) => {
                saver.abort(None).await.map_err(GenerationStop::Failed)?;
                let notice = self
                    .save_review_draft(&request.design_id, &fallback, &error, true)
                    .await
                    .map_err(GenerationStop::Failed)?;
                return Err(GenerationStop::Failed(notice));
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
}
