use super::*;

/// One recorded set of answers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnswerRecord {
    /// The question set answered.
    pub question_set: u32,
    /// The answers.
    pub answers: Vec<QuestionAnswer>,
    /// When the answers arrived, as an RFC 3339 UTC string.
    pub at: String,
}

/// One generation or briefing run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique id: `{unix_seconds}-{n}`.
    pub run_id: String,
    /// Which engine ran.
    pub mode: RunMode,
    /// `built-in` or `custom`.
    pub runtime: String,
    /// Provider name for a built-in run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model id for a built-in run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// When it started, as an RFC 3339 UTC string.
    pub started_at: String,
    /// When it finished, when it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// The outcome: `succeeded`, `failed`, `asked_questions`,
    /// `brief_presented`, `needs_clarification`, or `stopped`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The failure message, when the run failed. Ids only, never paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The design ids the run wrote.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

/// The persisted session record.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    /// The session id, which is the project slug.
    pub id: String,
    /// A short title, from the request.
    pub title: String,
    /// The user's request, in their words.
    pub request: String,
    /// What the session builds: a software demo (designs) or a deck.
    /// Set at creation; the brief carries the same value.
    #[serde(default)]
    pub artifact_kind: ArtifactKind,
    /// Where the session is in the workflow.
    pub state: WorkflowState,
    /// The state to return to when the session resumes from `stopped`
    /// or `error`. Records written before the stop state existed name
    /// this `state_before_error`.
    #[serde(
        default,
        alias = "state_before_error",
        skip_serializing_if = "Option::is_none"
    )]
    pub resume_state: Option<WorkflowState>,
    /// The failure message shown in the error state. Ids only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the session was created, as an RFC 3339 UTC string.
    pub created_at: String,
    /// When it last changed, as an RFC 3339 UTC string.
    pub updated_at: String,
    /// The design the user chose from the candidates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen_design: Option<String>,
    /// The number of the newest question set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_question_set: Option<u32>,
    /// The run options for this session.
    #[serde(default)]
    pub options: RunOptions,
}

/// One row of `GET /sessions`.
#[derive(Clone, Debug, Serialize)]
pub struct SessionSummary {
    /// The session id.
    pub id: String,
    /// The title.
    pub title: String,
    /// What the session builds.
    pub artifact_kind: ArtifactKind,
    /// The workflow state.
    pub state: WorkflowState,
    /// When it was created, RFC 3339.
    pub created_at: String,
    /// When it last changed, RFC 3339.
    pub updated_at: String,
    /// The chosen design or deck, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_design: Option<String>,
}

/// What a new session is made from.
#[derive(Clone, Debug)]
pub struct NewSession<'a> {
    /// The session id, which is the project slug.
    pub id: &'a str,
    /// A short title.
    pub title: &'a str,
    /// The user's request, in their words.
    pub request: &'a str,
    /// The run options.
    pub options: RunOptions,
    /// What the session builds.
    pub artifact_kind: ArtifactKind,
}

impl<'a> NewSession<'a> {
    /// A demo session with default options.
    pub fn demo(id: &'a str, title: &'a str, request: &'a str) -> Self {
        Self {
            id,
            title,
            request,
            options: RunOptions::default(),
            artifact_kind: ArtifactKind::Demo,
        }
    }

    /// The same session with these run options.
    pub fn with_options(mut self, options: RunOptions) -> Self {
        self.options = options;
        self
    }

    /// The same session building this kind of artifact.
    pub fn with_kind(mut self, artifact_kind: ArtifactKind) -> Self {
        self.artifact_kind = artifact_kind;
        self
    }
}

/// The full view of one session for `GET /sessions/{id}`.
#[derive(Clone, Debug, Serialize)]
pub struct SessionView {
    /// The session record.
    pub session: Session,
    /// Every question set asked, in order.
    pub question_sets: Vec<BriefQuestionSet>,
    /// The number of the question set still open, when one is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_question_set: Option<u32>,
    /// Every recorded answer set.
    pub answers: Vec<AnswerRecord>,
    /// The conversation.
    pub messages: Vec<ChatMessage>,
    /// The run records.
    pub runs: Vec<RunRecord>,
    /// The designs that belong to this session. Empty unless the session
    /// is a demo session.
    pub designs: Vec<DesignSummary>,
    /// The decks that belong to this session. Empty unless the session
    /// is a deck session.
    pub decks: Vec<DeckSummary>,
    /// The documents that belong to this session. Empty unless the
    /// session is a document session.
    pub documents: Vec<DocumentSummary>,
    /// The socials that belong to this session. Empty unless the
    /// session is a social session.
    pub socials: Vec<SocialSummary>,
    /// The prints that belong to this session. Empty unless the
    /// session is a print session.
    pub prints: Vec<PrintSummary>,
    /// The mailings that belong to this session. Empty unless the
    /// session is a mailing session.
    pub mailings: Vec<MailingSummary>,
    /// The campaigns that belong to this session. Empty unless the
    /// session is a campaign session.
    pub campaigns: Vec<CampaignSummary>,
    /// The artworks that belong to this session. Empty unless the
    /// session is an artwork session.
    pub artworks: Vec<ArtworkSummary>,
}

/// What went wrong in a session operation.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// No session with that id.
    #[error("no session `{id}`: create it with POST /sessions")]
    NotFound {
        /// The missing id.
        id: String,
    },
    /// A session already exists with that id.
    #[error("session `{id}` already exists")]
    AlreadyExists {
        /// The id in use.
        id: String,
    },
    /// The workflow refused the event.
    #[error(transparent)]
    Workflow(#[from] WorkflowError),
    /// A storage failure. The message names no path.
    #[error("session storage failed: {0}")]
    Io(String),
}

impl SessionError {
    /// Wraps a storage error without leaking a path into the message.
    fn io(error: impl std::fmt::Display) -> Self {
        SessionError::Io(error.to_string())
    }
}

/// Filesystem-backed session storage. A per-store lock serializes the
/// read-modify-write of a session record.
#[derive(Clone)]
pub struct SessionStore {
    directory: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl SessionStore {
    /// Creates a store over `directory`. The directory is created on
    /// the first write.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(super) fn session_directory(&self, id: &str) -> PathBuf {
        self.directory.join(id)
    }

    pub(super) fn session_path(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("session.json")
    }

    fn question_set_path(&self, id: &str, number: u32) -> PathBuf {
        self.session_directory(id)
            .join("question-sets")
            .join(format!("{number}.json"))
    }

    fn answers_path(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("answers.json")
    }

    fn messages_path(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("messages.json")
    }

    fn runs_directory(&self, id: &str) -> PathBuf {
        self.session_directory(id).join("runs")
    }

    /// Creates a session in the intake state. Fails when one exists.
    pub async fn create(&self, new: NewSession<'_>) -> Result<Session, SessionError> {
        let _guard = self.write_lock.lock().await;
        let id = new.id;
        if self.session_path(id).exists() {
            return Err(SessionError::AlreadyExists { id: id.to_owned() });
        }
        let now = rfc3339_now();
        let session = Session {
            id: id.to_owned(),
            title: new.title.to_owned(),
            request: new.request.to_owned(),
            artifact_kind: new.artifact_kind,
            state: WorkflowState::Intake,
            resume_state: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
            chosen_design: None,
            latest_question_set: None,
            options: new.options,
        };
        self.write_session(&session).await?;
        Ok(session)
    }

    /// Reads a session record. `Ok(None)` means none exists.
    pub async fn read(&self, id: &str) -> Result<Option<Session>, SessionError> {
        match tokio::fs::read_to_string(self.session_path(id)).await {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(SessionError::io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::io(error)),
        }
    }

    /// Reads a session or fails with `NotFound`.
    async fn require(&self, id: &str) -> Result<Session, SessionError> {
        self.read(id)
            .await?
            .ok_or_else(|| SessionError::NotFound { id: id.to_owned() })
    }

    async fn write_session(&self, session: &Session) -> Result<(), SessionError> {
        let directory = self.session_directory(&session.id);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(SessionError::io)?;
        let json = serde_json::to_string_pretty(session).map_err(SessionError::io)?;
        write_atomically(&self.session_path(&session.id), &json)
            .await
            .map_err(SessionError::io)
    }

    /// Lists every session, newest change first.
    pub async fn list(&self) -> Result<Vec<SessionSummary>, SessionError> {
        let mut entries = match tokio::fs::read_dir(&self.directory).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SessionError::io(error)),
        };
        let mut summaries = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(SessionError::io)? {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if let Some(session) = self.read(&id).await? {
                summaries.push(SessionSummary {
                    id: session.id,
                    title: session.title,
                    artifact_kind: session.artifact_kind,
                    state: session.state,
                    created_at: session.created_at,
                    updated_at: session.updated_at,
                    chosen_design: session.chosen_design,
                });
            }
        }
        summaries.sort_by(|first, second| second.updated_at.cmp(&first.updated_at));
        Ok(summaries)
    }

    /// Removes a session and everything under it.
    pub async fn delete(&self, id: &str) -> Result<bool, SessionError> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::remove_dir_all(self.session_directory(id)).await {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(SessionError::io(error)),
        }
    }

    /// Renames a session directory, so its id keeps matching its
    /// designs. `Ok(false)` when there is no session `old`.
    pub async fn rename(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        let _guard = self.write_lock.lock().await;
        let Some(mut session) = self.read(old).await? else {
            return Ok(false);
        };
        tokio::fs::rename(self.session_directory(old), self.session_directory(new))
            .await
            .map_err(SessionError::io)?;
        session.id = new.to_owned();
        session.updated_at = rfc3339_now();
        self.write_session(&session).await?;
        Ok(true)
    }

    /// Applies a workflow event through `design_model::transition`, the
    /// only place the state changes. Records the previous state on an
    /// error, and clears the error on recovery.
    pub async fn apply(&self, id: &str, event: WorkflowEvent) -> Result<Session, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut session = self.require(id).await?;
        let next = transition(session.state, event)?;
        // A halt records where to come back to; a resume clears both
        // the marker and any failure message.
        if matches!(event, WorkflowEvent::RunFailed | WorkflowEvent::RunStopped) {
            session.resume_state = Some(session.state);
        }
        if matches!(event, WorkflowEvent::Recovered { .. }) {
            session.resume_state = None;
            session.error = None;
        }
        session.state = next;
        session.updated_at = rfc3339_now();
        self.write_session(&session).await?;
        Ok(session)
    }

    /// Edits the non-state fields of a session under the lock.
    /// Clears `artifact_id` off the session that chose it.
    ///
    /// A deleted candidate must not stay the session's choice: the
    /// planner reads that field to decide which artifact an edit turn
    /// changes.
    pub async fn forget_artifact(&self, artifact_id: &str) {
        let session_id = session_id_of_artifact(artifact_id).to_owned();
        let _ = self
            .update(&session_id, |session| {
                if session.chosen_design.as_deref() == Some(artifact_id) {
                    session.chosen_design = None;
                }
            })
            .await;
    }

    pub async fn update(
        &self,
        id: &str,
        edit: impl FnOnce(&mut Session),
    ) -> Result<Session, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut session = self.require(id).await?;
        edit(&mut session);
        session.updated_at = rfc3339_now();
        self.write_session(&session).await?;
        Ok(session)
    }

    /// The state to resume into after a stop or an error: the state the
    /// run halted in, or intake. A halt in `generating` resumes in
    /// `reviewing`: a run that starts in `generating` writes candidates
    /// without a planner turn, and that overwrote finished candidates
    /// when the user's next message was an edit.
    pub async fn recovery_target(&self, id: &str) -> Result<WorkflowState, SessionError> {
        let session = self.require(id).await?;
        Ok(match session.resume_state {
            Some(WorkflowState::Generating) => WorkflowState::Reviewing,
            Some(state) => state,
            None => WorkflowState::Intake,
        })
    }

    /// Writes the next question set. Returns its number.
    pub async fn write_question_set(
        &self,
        id: &str,
        set: &BriefQuestionSet,
    ) -> Result<u32, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut session = self.require(id).await?;
        let number = session.latest_question_set.unwrap_or(0) + 1;
        let directory = self.session_directory(id).join("question-sets");
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(SessionError::io)?;
        let json = serde_json::to_string_pretty(set).map_err(SessionError::io)?;
        write_atomically(&self.question_set_path(id, number), &json)
            .await
            .map_err(SessionError::io)?;
        session.latest_question_set = Some(number);
        session.updated_at = rfc3339_now();
        self.write_session(&session).await?;
        Ok(number)
    }

    /// Reads one question set.
    pub async fn read_question_set(
        &self,
        id: &str,
        number: u32,
    ) -> Result<Option<BriefQuestionSet>, SessionError> {
        match tokio::fs::read_to_string(self.question_set_path(id, number)).await {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(SessionError::io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::io(error)),
        }
    }

    /// Every question set, in order.
    pub async fn question_sets(&self, id: &str) -> Result<Vec<BriefQuestionSet>, SessionError> {
        let session = self.require(id).await?;
        let mut sets = Vec::new();
        for number in 1..=session.latest_question_set.unwrap_or(0) {
            if let Some(set) = self.read_question_set(id, number).await? {
                sets.push(set);
            }
        }
        Ok(sets)
    }

    /// Records a set of answers.
    pub async fn record_answers(
        &self,
        id: &str,
        question_set: u32,
        answers: Vec<QuestionAnswer>,
    ) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut records = self.answers(id).await?;
        records.push(AnswerRecord {
            question_set,
            answers,
            at: rfc3339_now(),
        });
        self.write_json(&self.answers_path(id), &records).await
    }

    /// Every recorded answer set.
    pub async fn answers(&self, id: &str) -> Result<Vec<AnswerRecord>, SessionError> {
        self.read_json(&self.answers_path(id)).await
    }

    /// The questions and the user's latest answer for each, joined
    /// across every set. A later answer for the same question wins.
    pub async fn answered_questions(
        &self,
        id: &str,
    ) -> Result<Vec<(BriefQuestion, QuestionAnswer)>, SessionError> {
        let sets = self.question_sets(id).await?;
        let records = self.answers(id).await?;
        let mut joined: Vec<(BriefQuestion, QuestionAnswer)> = Vec::new();
        for record in records {
            let Some(set) = sets.get((record.question_set as usize).wrapping_sub(1)) else {
                continue;
            };
            for answer in record.answers {
                let Some(question) = set
                    .questions
                    .iter()
                    .find(|question| question.id == answer.question_id)
                else {
                    continue;
                };
                joined.retain(|(existing, _)| existing.id != question.id);
                joined.push((question.clone(), answer));
            }
        }
        Ok(joined)
    }

    /// Appends one conversation turn.
    pub async fn append_message(
        &self,
        id: &str,
        mut message: ChatMessage,
    ) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut messages = self.messages(id).await?;
        if message.at.is_none() {
            message.at = Some(rfc3339_now());
        }
        messages.push(message);
        self.write_json(&self.messages_path(id), &messages).await
    }

    /// The conversation.
    pub async fn messages(&self, id: &str) -> Result<Vec<ChatMessage>, SessionError> {
        self.read_json(&self.messages_path(id)).await
    }

    /// The conversation visible to a run, without waiting prompts.
    pub async fn messages_for_run(&self, id: &str) -> Result<Vec<ChatMessage>, SessionError> {
        Ok(self
            .messages(id)
            .await?
            .into_iter()
            .filter(|message| !message.is_queued)
            .collect())
    }

    /// True when at least one prompt waits for a turn.
    pub async fn has_queued_messages(&self, id: &str) -> Result<bool, SessionError> {
        Ok(self
            .messages(id)
            .await?
            .iter()
            .any(|message| message.is_queued))
    }

    /// True when validated answers requested a paused turn to resume.
    pub async fn has_requested_resume(&self, id: &str) -> Result<bool, SessionError> {
        Ok(self
            .messages(id)
            .await?
            .iter()
            .any(|message| message.is_queued && message.is_resume))
    }

    /// Places the current prompt ahead of waiting follow-ups after its answers arrive.
    pub async fn queue_answered_turn(&self, id: &str) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut messages = self.messages(id).await?;
        if messages
            .iter()
            .any(|message| message.is_queued && message.is_resume)
        {
            return Ok(());
        }
        let index = messages.iter().rposition(|message| {
            message.role == "user" && !message.is_queued && !message.is_continue
        });
        let mut message = if let Some(index) = index {
            messages.remove(index)
        } else {
            let session = self
                .read(id)
                .await?
                .ok_or_else(|| SessionError::NotFound { id: id.to_owned() })?;
            let mut message = ChatMessage::user(&session.request, session.chosen_design.as_deref());
            message.at = Some(session.created_at);
            message
        };
        message.is_queued = true;
        message.is_resume = true;
        let index = messages
            .iter()
            .position(|message| message.is_queued)
            .unwrap_or(messages.len());
        messages.insert(index, message);
        self.write_json(&self.messages_path(id), &messages).await
    }

    /// Activates the oldest waiting prompt and places it after the completed turn.
    /// Keeps its original timestamp, candidate targets, and regeneration action.
    pub async fn activate_next_message(
        &self,
        id: &str,
    ) -> Result<Option<ChatMessage>, SessionError> {
        let _guard = self.write_lock.lock().await;
        let mut messages = self.messages(id).await?;
        let Some(index) = messages.iter().position(|message| message.is_queued) else {
            return Ok(None);
        };
        let mut message = messages.remove(index);
        message.is_queued = false;
        message.is_resume = false;
        messages.push(message.clone());
        self.write_json(&self.messages_path(id), &messages).await?;
        Ok(Some(message))
    }

    /// Records the start of a run. Returns its id.
    pub async fn start_run(&self, id: &str, mut record: RunRecord) -> Result<String, SessionError> {
        let _guard = self.write_lock.lock().await;
        let directory = self.runs_directory(id);
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(SessionError::io)?;
        let existing = self.runs(id).await?.len();
        record.run_id = format!("{}-{}", unix_now_seconds(), existing + 1);
        let json = serde_json::to_string_pretty(&record).map_err(SessionError::io)?;
        write_atomically(&directory.join(format!("{}.json", record.run_id)), &json)
            .await
            .map_err(SessionError::io)?;
        Ok(record.run_id)
    }

    /// Records the end of a run.
    pub async fn finish_run(
        &self,
        id: &str,
        run_id: &str,
        result: &str,
        error: Option<String>,
        artifacts: Vec<String>,
    ) -> Result<(), SessionError> {
        let _guard = self.write_lock.lock().await;
        let path = self.runs_directory(id).join(format!("{run_id}.json"));
        let Some(mut record) = self.read_json_file::<RunRecord>(&path).await? else {
            return Ok(());
        };
        record.finished_at = Some(rfc3339_now());
        record.result = Some(result.to_owned());
        record.error = error;
        record.artifacts = artifacts;
        let json = serde_json::to_string_pretty(&record).map_err(SessionError::io)?;
        write_atomically(&path, &json)
            .await
            .map_err(SessionError::io)
    }

    /// Stops every run the last process left unfinished. A session in
    /// `generating` whose latest run has no end was killed with the
    /// server: nothing will finish it, and the session refuses messages
    /// until it halts. Returns the ids stopped.
    pub async fn stop_orphaned_runs(&self) -> Result<Vec<String>, SessionError> {
        let mut stopped = Vec::new();
        for summary in self.list().await? {
            if summary.state != WorkflowState::Generating {
                continue;
            }
            let orphans: Vec<RunRecord> = self
                .runs(&summary.id)
                .await?
                .into_iter()
                .filter(|record| record.finished_at.is_none())
                .collect();
            for record in &orphans {
                self.finish_run(
                    &summary.id,
                    &record.run_id,
                    "stopped",
                    Some("the server stopped while the run worked".to_owned()),
                    Vec::new(),
                )
                .await?;
            }
            self.apply(&summary.id, WorkflowEvent::RunStopped).await?;
            stopped.push(summary.id);
        }
        Ok(stopped)
    }

    /// Every run record, oldest first.
    pub async fn runs(&self, id: &str) -> Result<Vec<RunRecord>, SessionError> {
        let mut entries = match tokio::fs::read_dir(self.runs_directory(id)).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SessionError::io(error)),
        };
        let mut records = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(SessionError::io)? {
            if let Some(record) = self.read_json_file::<RunRecord>(&entry.path()).await? {
                records.push(record);
            }
        }
        records.sort_by(|first, second| {
            run_sort_key(&first.run_id)
                .cmp(&run_sort_key(&second.run_id))
                .then_with(|| first.run_id.cmp(&second.run_id))
        });
        Ok(records)
    }

    async fn write_json<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), SessionError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(SessionError::io)?;
        }
        let json = serde_json::to_string_pretty(value).map_err(SessionError::io)?;
        write_atomically(path, &json)
            .await
            .map_err(SessionError::io)
    }

    async fn read_json<T: for<'de> Deserialize<'de> + Default>(
        &self,
        path: &Path,
    ) -> Result<T, SessionError> {
        Ok(self.read_json_file(path).await?.unwrap_or_default())
    }

    async fn read_json_file<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
    ) -> Result<Option<T>, SessionError> {
        match tokio::fs::read_to_string(path).await {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(SessionError::io),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SessionError::io(error)),
        }
    }
}

/// Checks that a design or deck write is allowed now: the owning session
/// must be generating (agent writes), or generating or reviewing (user
/// writes from the editor). The message names the session state so the
/// caller can report it.
pub async fn write_access(
    sessions: &SessionStore,
    artifact_id: &str,
    is_user: bool,
) -> Result<(), String> {
    let session_id = session_id_of_artifact(artifact_id);
    let session = sessions
        .read(session_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no session `{session_id}`: create it with POST /sessions"))?;
    let allowed = session.state == WorkflowState::Generating
        || (is_user && session.state == WorkflowState::Reviewing);
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "writes to `{artifact_id}` are not allowed while session `{session_id}` is in state `{}`",
            session.state
        ))
    }
}

/// The run mode for a workflow state: generation in every state that
/// can take a turn or is writing, and nothing in a halted one. A
/// stopped or failed session resumes before it runs again.
pub fn run_mode_for(state: WorkflowState) -> Option<RunMode> {
    match state {
        state if state.is_halted() => None,
        _ => Some(RunMode::Generation),
    }
}

/// True for ids that are valid session ids: a valid design id that is
/// not a candidate id, so a session and a candidate never collide.
pub fn is_valid_session_id(id: &str) -> bool {
    is_valid_design_id(id) && !id.contains(CANDIDATE_MARKER)
}

/// The session id a design or deck belongs to: the artifact id with
/// any `-candidate-N` suffix removed.
pub fn session_id_of_artifact(artifact_id: &str) -> &str {
    match artifact_id.find(CANDIDATE_MARKER) {
        Some(position) => &artifact_id[..position],
        None => artifact_id,
    }
}

/// A fresh session id: a random version 4 UUID.
///
/// The id is a key, not a name. The name a user reads is `title`, and
/// the times are `created_at` and `updated_at`. An id built from the
/// request text made the same request produce the same id, so two
/// sessions with the same wording collided, and a deleted session left
/// its files for the next one to adopt.
pub fn new_session_id() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| anyhow::anyhow!("no randomness source: {error}"))?;
    // The version and variant bits of RFC 9562 version 4.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        id.push_str(&format!("{byte:02x}"));
    }
    Ok(id)
}
