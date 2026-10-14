use super::artifacts::*;
use super::attachments::*;
use super::streaming::*;
use super::*;

impl GenerationEngine {
    /// Creates an engine over the given stores. `settings` enables
    /// login-token refresh for Claude logins.
    pub fn new(
        configuration: ModelConfiguration,
        designs: DesignStore,
        sessions: SessionStore,
        settings: Option<SettingsStore>,
        address: String,
        notifier: ChangeNotifier,
    ) -> Self {
        Self {
            model: ModelClient::new(configuration, settings),
            designs,
            decks: None,
            documents: None,
            socials: None,
            prints: None,
            mailings: None,
            campaigns: None,
            artworks: None,
            sessions,
            address,
            notifier,
            progress_sink: None,
            design_progress_sink: None,
            templates: None,
            uploads: None,
            demo_dom_runner: chrome_dom_runner(),
        }
    }

    /// Replaces the real browser boundary in engine tests.
    #[cfg(test)]
    pub(crate) fn with_demo_dom_runner(mut self, runner: DemoDomRunner) -> Self {
        self.demo_dom_runner = runner;
        self
    }

    /// Measures a complete demo and rejects unavailable or incomplete browser reports.
    pub(super) async fn audit_demo(&self, design: &Design) -> Result<DemoAudit, String> {
        let dom = (self.demo_dom_runner)(design.clone(), self.base_url()).await?;
        DemoAudit::from_dom(&dom, design.screens.len())
    }

    /// Lets the engine write decks, for deck sessions.
    pub fn with_decks(mut self, decks: DeckStore) -> Self {
        self.decks = Some(decks);
        self
    }

    /// Lets the engine write documents, for document sessions.
    pub fn with_documents(mut self, documents: DocumentStore) -> Self {
        self.documents = Some(documents);
        self
    }

    /// Lets the engine write socials, for social sessions.
    pub fn with_socials(mut self, socials: SocialStore) -> Self {
        self.socials = Some(socials);
        self
    }

    /// Lets the engine write prints, for print sessions.
    pub fn with_prints(mut self, prints: PrintStore) -> Self {
        self.prints = Some(prints);
        self
    }

    /// Lets the engine write mailings, for mailing sessions.
    pub fn with_mailings(mut self, mailings: MailingStore) -> Self {
        self.mailings = Some(mailings);
        self
    }

    /// Lets the engine write campaigns, for campaign sessions.
    pub fn with_campaigns(mut self, campaigns: CampaignStore) -> Self {
        self.campaigns = Some(campaigns);
        self
    }

    /// Lets the engine write artworks, for artwork sessions.
    pub fn with_artworks(mut self, artworks: ArtworkStore) -> Self {
        self.artworks = Some(artworks);
        self
    }

    /// Lets the engine read the saved templates, so a brief that names
    /// one can style its candidates from it.
    pub fn with_templates(mut self, templates: TemplateStore) -> Self {
        self.templates = Some(templates);
        self
    }

    /// Lets the engine attach the user's uploads to the requests that
    /// write content.
    pub fn with_uploads(mut self, uploads: UploadStore) -> Self {
        self.uploads = Some(uploads);
        self
    }

    /// The uploads to attach to a content request: the files of
    /// `session_id` under the size caps. Empty without an upload store.
    /// A file that cannot be read is logged and skipped.
    ///
    /// The session decides the list. A file attached to one project must
    /// never reach another project's prompt.
    pub(crate) async fn load_attachments(&self, session_id: &str, log: &LogSink) -> Attachments {
        let mut attachments = Attachments::default();
        let Some(uploads) = &self.uploads else {
            return attachments;
        };
        let summaries = match uploads.list(session_id).await {
            Ok(summaries) => summaries,
            Err(error) => {
                log(&format!("listing uploads failed: {error}"));
                return attachments;
            }
        };
        let mut total = 0usize;
        for summary in summaries {
            let size = summary.size_bytes as usize;
            if size > ATTACHMENT_FILE_LIMIT_BYTES || total + size > ATTACHMENT_TOTAL_LIMIT_BYTES {
                attachments
                    .skipped
                    .push(format!("{} ({})", summary.name, describe_size(size)));
                continue;
            }
            match uploads.read(&summary.name).await {
                Ok(Some(bytes)) => {
                    total += bytes.len();
                    attachments.files.push(UploadAttachment {
                        name: summary.name,
                        content_type: summary.content_type.to_owned(),
                        bytes,
                    });
                }
                Ok(None) => {}
                Err(error) => log(&format!(
                    "reading upload `{}` failed: {error}",
                    summary.name
                )),
            }
        }
        if !attachments.files.is_empty() || !attachments.skipped.is_empty() {
            log(&format!(
                "attaching {} upload{} ({} skipped over the size caps)",
                attachments.files.len(),
                if attachments.files.len() == 1 {
                    ""
                } else {
                    "s"
                },
                attachments.skipped.len()
            ));
        }
        attachments
    }

    /// A user message with `text` and the attachments as content parts.
    /// Image parts go only to models that can see them.
    pub(crate) fn user_message(&self, text: &str, attachments: &Attachments) -> serde_json::Value {
        let can_see_images = supports_vision(self.model.model());
        serde_json::json!({
            "role": "user",
            "content": user_content_with_attachments(text, attachments, can_see_images),
        })
    }

    /// The templates the options name, in order. A template that was
    /// deleted is skipped, so the run still writes the rest.
    pub(crate) async fn brief_templates(
        &self,
        options: &RunOptions,
        log: &LogSink,
    ) -> Vec<Template> {
        let ids = options.templates.clone();
        if ids.is_empty() {
            return Vec::new();
        }
        let Some(templates) = self.templates.as_ref() else {
            return Vec::new();
        };
        let mut loaded = Vec::new();
        for id in ids {
            match templates.load(&id).await {
                Ok(Some(template)) => {
                    log(&format!("styling candidates from template `{id}`"));
                    loaded.push(template);
                }
                Ok(None) => log(&format!("template `{id}` no longer exists; ignoring it")),
                Err(error) => log(&format!("reading template `{id}` failed: {error}")),
            }
        }
        loaded
    }

    /// Renews an expiring login and persists the new tokens.
    pub(super) async fn refresh_login_if_needed(&mut self, log: &LogSink) -> Result<(), String> {
        self.model.refresh_login_if_needed(log).await
    }

    /// Short label for the studio: `google/gemini-2.5-flash`.
    pub fn label(&self) -> String {
        self.model.label()
    }

    /// The context window of the configured model, in tokens.
    pub fn context_window(&self) -> u64 {
        self.model.context_window()
    }

    /// Runs one turn for `session_id`, as Swift Deck did: a continue
    /// request continues; otherwise the planner reads the request, the
    /// answers, and the conversation, then asks, writes, edits, or
    /// replies. The session may be in any state but error.
    pub async fn run(
        mut self,
        session_id: &str,
        log: LogSink,
    ) -> Result<GenerationOutcome, String> {
        self.refresh_login_if_needed(&log).await?;
        let session = self
            .sessions
            .read(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no session `{session_id}`"))?;
        if session.state.is_halted() {
            return Err(format!(
                "the session is `{}`: resume it first",
                session.state
            ));
        }
        let answered = self
            .sessions
            .answered_questions(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let messages = self
            .sessions
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let context = GenerationContext {
            request: SessionRequest {
                request: session.request.clone(),
                conversation: conversation_note(recent_conversation(&messages)),
                kind: session.artifact_kind,
                answers: answered_questions_from_answers(&answered),
                options: session.options.clone(),
            },
            options: session.options.clone(),
            session_id: session_id.to_owned(),
        };
        let run_id = self
            .sessions
            .start_run(
                session_id,
                RunRecord {
                    run_id: String::new(),
                    mode: RunMode::Generation,
                    runtime: "built-in".to_owned(),
                    provider: None,
                    model: Some(self.model.model().to_owned()),
                    started_at: rfc3339_now(),
                    finished_at: None,
                    result: None,
                    error: None,
                    artifacts: Vec::new(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        log(&format!(
            "turn · {} · effort {}",
            self.label(),
            context.effort(),
        ));
        let client = ModelClient::build_http_client()?;
        // A session already generating asked to write now: no planner.
        let write_now = session.state == design_model::WorkflowState::Generating;
        let task = match self.pick_task(&context).await? {
            Some(task) => task,
            None if write_now => GenerationTask::Candidates,
            None => match self.plan_turn(&client, &session, &context, &log).await {
                Ok(PlanStep::Task(task)) => task,
                Ok(PlanStep::Asked(question_set)) => {
                    self.finish_run(session_id, &run_id, "asked_questions", None, Vec::new())
                        .await;
                    return Ok(GenerationOutcome::NeedsClarification { question_set });
                }
                Ok(PlanStep::Replied) => {
                    self.finish_run(session_id, &run_id, "replied", None, Vec::new())
                        .await;
                    return Ok(GenerationOutcome::Replied);
                }
                Err(message) => {
                    return self
                        .settle(
                            session_id,
                            &run_id,
                            Err(GenerationStop::Failed(message)),
                            &log,
                        )
                        .await;
                }
            },
        };
        if session.state != design_model::WorkflowState::Generating {
            self.sessions
                .apply(session_id, design_model::WorkflowEvent::GenerationStarted)
                .await
                .map_err(|error| error.to_string())?;
            self.notifier.notify();
        }
        self.report_progress(0);
        let outcome = self.execute(&client, &context, task, &log).await;
        let outcome = self
            .run_late_continues(&client, &context, outcome, &log)
            .await;
        self.report_progress(100);
        self.settle(session_id, &run_id, outcome, &log).await
    }

    /// Runs the continue requests that arrived while this run worked and
    /// folds what they wrote into `outcome`.
    ///
    /// Pressing Finish on a second candidate appends a request the
    /// running turn has already read past. Without this pass it would
    /// wait for a turn that nothing starts.
    ///
    /// Each id is tried once. A continue that fails leaves a preview
    /// behind, and trying it again would never end the run.
    pub(super) async fn run_late_continues(
        &self,
        client: &reqwest::Client,
        context: &GenerationContext,
        outcome: Result<GenerationOutcome, GenerationStop>,
        log: &LogSink,
    ) -> Result<GenerationOutcome, GenerationStop> {
        let Ok(GenerationOutcome::Wrote { mut design_ids }) = outcome else {
            return outcome;
        };
        let mut tried = design_ids.clone();
        loop {
            let pending = match self.pick_task(context).await {
                Ok(Some(GenerationTask::Continue(ids))) => ids,
                _ => return Ok(GenerationOutcome::Wrote { design_ids }),
            };
            let fresh: Vec<String> = pending
                .into_iter()
                .filter(|id| !tried.contains(id))
                .collect();
            if fresh.is_empty() {
                return Ok(GenerationOutcome::Wrote { design_ids });
            }
            log(&format!(
                "continuing {} more: {}",
                fresh.len(),
                fresh.join(", ")
            ));
            tried.extend(fresh.clone());
            let task = GenerationTask::Continue(fresh);
            match self.execute(client, context, task, log).await {
                Ok(GenerationOutcome::Wrote { design_ids: more }) => design_ids.extend(more),
                Ok(_) => {}
                Err(stop) => {
                    // The first task wrote, so the run is not a failure.
                    let reason = match &stop {
                        GenerationStop::Failed(message) => message.clone(),
                        GenerationStop::NeedsClarification(_) => "it asked a question".to_owned(),
                    };
                    log(&format!("a later continue stopped: {reason}"));
                    return Ok(GenerationOutcome::Wrote { design_ids });
                }
            }
        }
    }

    /// A continue or a regenerate request names its artifacts; no
    /// planning is needed.
    pub(super) async fn pick_task(
        &self,
        context: &GenerationContext,
    ) -> Result<Option<GenerationTask>, String> {
        let messages = self
            .sessions
            .messages_for_run(&context.session_id)
            .await
            .map_err(|error| error.to_string())?;
        if let Some((design, instruction)) = trailing_regenerate(&messages) {
            let conversation = conversation_note(recent_conversation(&messages));
            return Ok(Some(GenerationTask::Regenerate {
                design,
                instruction,
                conversation,
            }));
        }
        let continues = match context.request.kind {
            design_model::ArtifactKind::Demo => self.continue_requests(&context.session_id).await?,
            design_model::ArtifactKind::Deck => {
                self.continue_deck_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Document => {
                self.continue_document_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Social => {
                self.continue_social_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Print => {
                self.continue_print_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Mailing => {
                self.continue_mailing_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Campaign => {
                self.continue_campaign_requests(&context.session_id).await?
            }
            design_model::ArtifactKind::Artwork => {
                self.continue_artwork_requests(&context.session_id).await?
            }
        };
        Ok((!continues.is_empty()).then_some(GenerationTask::Continue(continues)))
    }

    /// Fills the blank app questions from what the planner read in the
    /// request, before the card opens. The card then shows them as
    /// picked and marked as suggested. Only the setup turn does this:
    /// a later turn has no card, so a suggestion would land unseen.
    pub(super) async fn suggest_options(
        &self,
        context: &GenerationContext,
        suggestions: &[(String, String)],
        log: &LogSink,
    ) -> Result<(), String> {
        if suggestions.is_empty() {
            return Ok(());
        }
        let kind = context.request.kind;
        let mut filled = Vec::new();
        self.sessions
            .update(&context.session_id, |session| {
                filled = session.options.suggest(kind, suggestions);
            })
            .await
            .map_err(|error| error.to_string())?;
        if filled.is_empty() {
            return Ok(());
        }
        tracing::info!(
            session_id = %context.session_id,
            axes = ?filled,
            "app questions suggested from the request"
        );
        log(&format!("suggested {}", filled.join(", ")));
        self.notifier.notify();
        Ok(())
    }

    /// Asks the planner what this turn does: questions, a write, an
    /// edit of the open artifact, or a reply.
    pub(super) async fn plan_turn(
        &self,
        client: &reqwest::Client,
        session: &Session,
        context: &GenerationContext,
        log: &LogSink,
    ) -> Result<PlanStep, String> {
        let session_id = context.session_id.as_str();
        let messages = self
            .sessions
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let candidate_count = self.candidate_count(context).await;
        let targets = edit_targets(&messages, session.chosen_design.as_deref());
        // The planner reads the user's source files, as the writing
        // turns do. Planning blind is what produced questions the files
        // already answered.
        let attachments = self.load_attachments(&context.session_id, log).await;
        let input = planner_input(&context.request, &messages, candidate_count, &targets);
        let request = vec![
            serde_json::json!({ "role": "system", "content": planner_prompt(context.request.kind) }),
            self.user_message(&input, &attachments),
        ];
        log("planning the turn");
        let reply = self.model.chat(client, &request, context.effort()).await?;
        let plan = parse_plan(&reply);
        // The first turn always asks. The app owns a fixed list of
        // questions and asks it before anything is written; the planner
        // only adds to that card. Without this, a planner that asks
        // nothing writes a session the user never set up.
        let is_setup_turn = session.state == design_model::WorkflowState::Intake;
        if is_setup_turn {
            self.suggest_options(context, &plan.suggestions, log)
                .await?;
        }
        let asked = plan
            .question_set
            .clone()
            .or_else(|| is_setup_turn.then(app_question_set));
        // The limit is per request: answers given before the last write
        // belong to an earlier request, and a new change may need its own
        // questions.
        let records = self
            .sessions
            .answers(session_id)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(set) = &asked
            && answers_since_last_write(&messages, &records) < ANSWERED_QUESTION_LIMIT
        {
            let number = self
                .sessions
                .write_question_set(session_id, set)
                .await
                .map_err(|error| error.to_string())?;
            // The setup turn never writes, so a planner reply that
            // promises a write would contradict the card it opens.
            let is_promise_to_write = is_setup_turn && plan.should_generate;
            let spoken = match plan.reply.trim() {
                reply if !reply.is_empty() && !is_promise_to_write => reply.to_owned(),
                _ => set.message.clone(),
            };
            self.sessions
                .append_message(
                    session_id,
                    ChatMessage::assistant_questions(&spoken, number),
                )
                .await
                .map_err(|error| error.to_string())?;
            self.sessions
                .apply(session_id, design_model::WorkflowEvent::QuestionsAsked)
                .await
                .map_err(|error| error.to_string())?;
            self.notifier.notify();
            log(&format!("asked question set {number}"));
            return Ok(PlanStep::Asked(number));
        }
        if !plan.reply.is_empty() {
            self.say(session_id, &plan.reply).await?;
        }
        // A merge needs two sources. With one, the change is an edit.
        if plan.should_merge && targets.len() >= 2 {
            let instruction = latest_user_text(&messages).unwrap_or_default();
            return Ok(PlanStep::Task(GenerationTask::Merge {
                sources: targets,
                instruction,
            }));
        }
        if (plan.should_edit || plan.should_merge) && !targets.is_empty() {
            let instruction = latest_user_text(&messages).unwrap_or_default();
            // The window is built from the messages read before `say`,
            // so the planner's own reply stays out of its edit context.
            let conversation = conversation_note(recent_conversation(&messages));
            return Ok(PlanStep::Task(GenerationTask::Edit {
                designs: targets,
                instruction,
                conversation,
            }));
        }
        // Past the question limit the planner writes instead of asking.
        if plan.should_generate || plan.question_set.is_some() {
            return Ok(PlanStep::Task(GenerationTask::Candidates));
        }
        log("replied; waiting for the user");
        Ok(PlanStep::Replied)
    }

    /// How many candidates of this session are on the canvas.
    pub(super) async fn candidate_count(&self, context: &GenerationContext) -> usize {
        let session_id = context.session_id.as_str();
        match context.request.kind {
            design_model::ArtifactKind::Demo => self
                .designs
                .list()
                .await
                .map(|rows| {
                    rows.iter()
                        .filter(|row| session_id_of_artifact(&row.id) == session_id)
                        .count()
                })
                .unwrap_or(0),
            design_model::ArtifactKind::Deck => match &self.decks {
                Some(decks) => decks
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Document => match &self.documents {
                Some(documents) => documents
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Social => match &self.socials {
                Some(socials) => socials
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Print => match &self.prints {
                Some(prints) => prints
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Mailing => match &self.mailings {
                Some(mailings) => mailings
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Campaign => match &self.campaigns {
                Some(campaigns) => campaigns
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
            design_model::ArtifactKind::Artwork => match &self.artworks {
                Some(artworks) => artworks
                    .list()
                    .await
                    .map(|rows| {
                        rows.iter()
                            .filter(|row| session_id_of_artifact(&row.id) == session_id)
                            .count()
                    })
                    .unwrap_or(0),
                None => 0,
            },
        }
    }

    /// The preview designs the latest user turn asked to continue:
    /// every design named by a trailing continue request that still is
    /// a preview. Pressing Finish on several candidates continues them
    /// all.
    pub(super) async fn continue_requests(&self, session_id: &str) -> Result<Vec<String>, String> {
        let messages = self
            .sessions
            .messages_for_run(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let mut previews = Vec::new();
        for design_id in trailing_continue_ids(&messages) {
            // A design that is no longer a preview was finished
            // already, by this run or an earlier one.
            if let Ok(Some(design)) = self.designs.load(&design_id).await
                && design.is_preview()
            {
                previews.push(design_id);
            }
        }
        Ok(previews)
    }
}

mod continue_run;
mod request;
mod work;
