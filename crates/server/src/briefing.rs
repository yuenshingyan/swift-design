//! The briefing engine: it asks material questions and drafts the
//! brief. It holds no `DesignStore`, so it cannot write a design. That
//! is the mode boundary in code, not only in the prompt.

use design_model::text::repeats;
use design_model::{
    AnsweredQuestion, ArtifactKind, BriefQuestion, BriefQuestionSet, DesignBrief, QuestionAnswer,
    QuestionSetError, RevisionSource, WorkflowEvent, validate_question_set,
};

use crate::events::ChangeNotifier;
use crate::model_client::{LogSink, ModelClient};
use crate::sessions::{ChatMessage, RunMode, RunRecord, SessionStore};

/// How many times a reply that cannot be read is sent back for a
/// correction before the run fails.
const REPAIR_ROUND_LIMIT: usize = 2;

/// The reply the model must send in briefing mode.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BriefingReply {
    /// Text for the user.
    #[serde(default)]
    pub reply: String,
    /// A question set, when the agent needs answers.
    #[serde(default)]
    pub question_set: Option<BriefQuestionSet>,
    /// A brief draft, when the agent is ready to present one.
    #[serde(default)]
    pub brief: Option<DesignBrief>,
    /// True when the brief is complete enough to present.
    #[serde(default)]
    pub is_complete: bool,
}

/// What a briefing turn did.
#[derive(Debug, PartialEq, Eq)]
pub enum BriefingOutcome {
    /// The agent asked a question set.
    AskedQuestions {
        /// The set number.
        question_set: u32,
    },
    /// The agent presented a brief revision.
    BriefPresented {
        /// The revision number.
        revision: u32,
    },
    /// The agent only replied.
    Replied,
}

/// Why a briefing reply could not be used.
#[derive(Debug, thiserror::Error)]
pub enum BriefingReplyError {
    /// The reply held no JSON object.
    #[error("the reply is not JSON")]
    NotJson,
    /// The JSON did not match the reply shape.
    #[error("the reply has the wrong shape: {0}")]
    Shape(String),
    /// The question set failed validation.
    #[error("the question set is invalid: {}", join_problems(.0))]
    InvalidQuestionSet(Vec<QuestionSetError>),
    /// The reply said complete but sent no brief.
    #[error("is_complete is true but no brief was sent")]
    BriefWithoutCompletion,
    /// The reply asked and presented at once.
    #[error("send a question set or a complete brief, not both")]
    AskedAndPresented,
}

/// The question set problems on one line, separated by `; `.
fn join_problems(problems: &[QuestionSetError]) -> String {
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The message that sends a reply error back to the model.
fn repair_request(error: &BriefingReplyError) -> String {
    format!(
        "Your reply could not be read: {error}.\n\
         Send one corrected JSON object only. Use the field names from the JSON Schema in the system prompt. No prose, no code fences."
    )
}

/// Parses a briefing reply from the model text. Slices the outermost
/// JSON object, then checks the fields and the question set.
pub fn parse_briefing_reply(content: &str) -> Result<BriefingReply, BriefingReplyError> {
    let start = content.find('{');
    let end = content.rfind('}');
    let (start, end) = match (start, end) {
        (Some(start), Some(end)) if end > start => (start, end),
        _ => return Err(BriefingReplyError::NotJson),
    };
    let reply: BriefingReply = serde_json::from_str(&content[start..=end])
        .map_err(|error| BriefingReplyError::Shape(error.to_string()))?;
    if reply.question_set.is_some() && reply.is_complete {
        return Err(BriefingReplyError::AskedAndPresented);
    }
    if reply.is_complete && reply.brief.is_none() {
        return Err(BriefingReplyError::BriefWithoutCompletion);
    }
    if let Some(set) = &reply.question_set {
        let problems = validate_question_set(set);
        if !problems.is_empty() {
            return Err(BriefingReplyError::InvalidQuestionSet(problems));
        }
    }
    Ok(reply)
}

/// Removes the lines a reader has already read.
///
/// The model restates the answers and the structured fields as
/// "confirmed facts": `The audience is developers.` next to the answer
/// `Developers` and the `audience` field. The prompt forbids it; this
/// enforces it, because a prompt is not a guarantee. The match is
/// lexical and deliberately conservative: a fact goes only when it
/// contains a value the panel already shows.
pub fn tidy_brief(brief: &mut DesignBrief) {
    let mut shown: Vec<String> = brief
        .answered_questions
        .iter()
        .map(|entry| entry.answer.clone())
        .collect();
    for field in [
        &brief.target_artifact,
        &brief.target_platform,
        &brief.audience,
        &brief.user_problem,
        &brief.primary_job,
        &brief.success_criterion,
        &brief.visual_direction,
    ] {
        shown.push(field.clone());
    }
    brief
        .confirmed_facts
        .retain(|fact| !is_answer_line(fact) && !shown.iter().any(|value| repeats(fact, value)));
    brief.assumptions.retain(|line| !is_answer_line(line));
    drop_repeats(&mut brief.confirmed_facts);
    drop_repeats(&mut brief.assumptions);
    drop_repeats(&mut brief.open_questions);
}

/// True when a line restates a question and its answer instead of
/// stating one thing: `Which platform?: Web`, or `Assumed for `x`: y`.
/// Briefs written before the app recorded the answers are full of them.
fn is_answer_line(line: &str) -> bool {
    line.starts_with("Assumed for ") || line.contains("?: ")
}

/// Drops later lines that say the same as an earlier one, keeping order.
fn drop_repeats(lines: &mut Vec<String>) {
    let mut kept: Vec<String> = Vec::new();
    lines.retain(|line| {
        let key = design_model::text::normalized(line);
        if key.trim().is_empty() || kept.contains(&key) {
            return false;
        }
        kept.push(key);
        true
    });
}

/// The assumptions the skipped answers imply: one sentence per question
/// the user asked the agent to decide. Used when the user generates
/// with assumptions.
pub fn assumptions_from_skipped_answers(
    answered: &[(BriefQuestion, QuestionAnswer)],
) -> Vec<String> {
    answered
        .iter()
        .filter(|(_, answer)| answer.skipped)
        .map(|(question, _)| format!("The agent chooses the answer to: {}", question.label.trim()))
        .collect()
}

/// The answered questions as brief entries: the question wording and
/// the answer wording, kept apart.
///
/// The brief keeps these out of `confirmed_facts`. A run of
/// `question: answer` lines inside a prose fact list is what makes a
/// brief hard to read: the reader cannot tell a question from a fact.
pub fn answered_questions_from_answers(
    answered: &[(BriefQuestion, QuestionAnswer)],
) -> Vec<AnsweredQuestion> {
    answered
        .iter()
        .map(|(question, answer)| AnsweredQuestion {
            question: question.label.clone(),
            answer: if answer.skipped {
                String::new()
            } else {
                answer_text(question, answer)
            },
            is_assumed: answer.skipped,
        })
        .collect()
}

/// The readable text of one answer: option labels joined, plus any
/// other text.
fn answer_text(question: &BriefQuestion, answer: &QuestionAnswer) -> String {
    let mut parts: Vec<String> = answer
        .values
        .iter()
        .map(|value| {
            question
                .options
                .iter()
                .find(|option| &option.value == value)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| value.clone())
        })
        .collect();
    if let Some(other) = &answer.other_text
        && !other.trim().is_empty()
    {
        parts.push(other.trim().to_owned());
    }
    parts.join(", ")
}

/// The briefing engine: the model client plus the session store. No
/// design store, by construction.
#[derive(Clone)]
pub struct BriefingEngine {
    model: ModelClient,
    sessions: SessionStore,
    notifier: ChangeNotifier,
}

impl BriefingEngine {
    /// Creates a briefing engine.
    pub fn new(model: ModelClient, sessions: SessionStore, notifier: ChangeNotifier) -> Self {
        Self {
            model,
            sessions,
            notifier,
        }
    }

    /// Short label for the studio: `google/gemini-2.5-flash`.
    pub fn label(&self) -> String {
        self.model.label()
    }

    /// The context window of the configured model, in tokens.
    pub fn context_window(&self) -> u64 {
        self.model.context_window()
    }

    /// Runs one briefing turn: read the session, ask the model, and act
    /// on the typed reply.
    pub async fn run(mut self, session_id: &str, log: LogSink) -> Result<BriefingOutcome, String> {
        self.model.refresh_login_if_needed(&log).await?;
        let session = self
            .sessions
            .read(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no session `{session_id}`"))?;
        if !session.state.is_briefing() {
            return Err(format!(
                "the session is in state `{}`, not a briefing state",
                session.state
            ));
        }
        let run_id = self
            .sessions
            .start_run(
                session_id,
                RunRecord {
                    run_id: String::new(),
                    mode: RunMode::Briefing,
                    runtime: "built-in".to_owned(),
                    provider: None,
                    model: Some(self.model.model().to_owned()),
                    brief_revision: None,
                    started_at: crate::time::rfc3339_now(),
                    finished_at: None,
                    result: None,
                    error: None,
                    artifacts: Vec::new(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        match self.take_turn(session_id, &log).await {
            Ok(outcome) => {
                let result = match &outcome {
                    BriefingOutcome::AskedQuestions { .. } => "asked_questions",
                    BriefingOutcome::BriefPresented { .. } => "brief_presented",
                    BriefingOutcome::Replied => "replied",
                };
                self.finish_run(session_id, &run_id, result, None).await;
                Ok(outcome)
            }
            Err(error) => {
                self.finish_run(session_id, &run_id, "failed", Some(error.clone()))
                    .await;
                Err(error)
            }
        }
    }

    /// Asks the model one turn and acts on the reply.
    async fn take_turn(&self, session_id: &str, log: &LogSink) -> Result<BriefingOutcome, String> {
        let session = self
            .sessions
            .read(session_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no session `{session_id}`"))?;
        let messages = self
            .sessions
            .messages(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let answered = self
            .sessions
            .answered_questions(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let latest_brief = self
            .sessions
            .latest_brief(session_id)
            .await
            .map_err(|error| error.to_string())?;
        let client = ModelClient::build_http_client()?;
        let request = vec![
            serde_json::json!({ "role": "system", "content": briefing_prompt(session.artifact_kind) }),
            serde_json::json!({
                "role": "user",
                "content": briefing_input(&session.request, &messages, &answered, latest_brief.as_ref()),
            }),
        ];
        log("briefing: asking the model");
        let reply = self
            .ask_for_reply(&client, request, &session.options.effort, log)
            .await?;
        if let Some(set) = reply.question_set {
            return self.ask(session_id, &set, &reply.reply, log).await;
        }
        if reply.is_complete
            && let Some(brief) = reply.brief
        {
            return self
                .present(session_id, brief, &reply.reply, &answered)
                .await;
        }
        // A plain reply: keep the state and record the turn.
        self.say(session_id, &reply.reply).await?;
        Ok(BriefingOutcome::Replied)
    }

    /// Asks the model for a reply. When the reply cannot be read, sends
    /// the error back and asks for a corrected reply, up to
    /// `REPAIR_ROUND_LIMIT` times.
    async fn ask_for_reply(
        &self,
        client: &reqwest::Client,
        mut request: Vec<serde_json::Value>,
        effort: &str,
        log: &LogSink,
    ) -> Result<BriefingReply, String> {
        let mut round = 0;
        loop {
            let content = self.model.chat(client, &request, effort).await?;
            let error = match parse_briefing_reply(&content) {
                Ok(reply) => return Ok(reply),
                Err(error) => error,
            };
            if round >= REPAIR_ROUND_LIMIT {
                return Err(format!(
                    "the model reply could not be read ({error}). Your answers are kept; run again to retry."
                ));
            }
            round += 1;
            log(&format!(
                "briefing: the reply could not be read ({error}); asking for a correction, round {round}"
            ));
            request.push(serde_json::json!({ "role": "assistant", "content": content }));
            request.push(serde_json::json!({ "role": "user", "content": repair_request(&error) }));
        }
    }

    /// Saves a question set and moves to clarifying.
    async fn ask(
        &self,
        session_id: &str,
        set: &BriefQuestionSet,
        reply: &str,
        log: &LogSink,
    ) -> Result<BriefingOutcome, String> {
        let number = self
            .sessions
            .write_question_set(session_id, set)
            .await
            .map_err(|error| error.to_string())?;
        let text = if reply.trim().is_empty() {
            set.message.clone()
        } else {
            reply.to_owned()
        };
        self.sessions
            .append_message(session_id, ChatMessage::assistant_questions(&text, number))
            .await
            .map_err(|error| error.to_string())?;
        self.sessions
            .apply(session_id, WorkflowEvent::QuestionsAsked)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        log(&format!("briefing: asked question set {number}"));
        Ok(BriefingOutcome::AskedQuestions {
            question_set: number,
        })
    }

    /// Merges the answers into the brief, drafts a revision, and
    /// presents it.
    async fn present(
        &self,
        session_id: &str,
        mut brief: DesignBrief,
        reply: &str,
        answered: &[(BriefQuestion, QuestionAnswer)],
    ) -> Result<BriefingOutcome, String> {
        brief.answers = answered.iter().map(|(_, answer)| answer.clone()).collect();
        brief.answered_questions = answered_questions_from_answers(answered);
        tidy_brief(&mut brief);
        let revision = self
            .sessions
            .write_brief_revision(
                session_id,
                brief,
                RevisionSource::Agent,
                "Drafted from the conversation",
            )
            .await
            .map_err(|error| error.to_string())?;
        self.sessions
            .apply(session_id, WorkflowEvent::BriefDrafted)
            .await
            .map_err(|error| error.to_string())?;
        let text = if reply.trim().is_empty() {
            "I drafted the brief. Review it, then approve or edit it.".to_owned()
        } else {
            reply.to_owned()
        };
        self.say(session_id, &text).await?;
        self.sessions
            .apply(session_id, WorkflowEvent::BriefPresented)
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(BriefingOutcome::BriefPresented { revision })
    }

    /// Appends an assistant turn and wakes the studio.
    async fn say(&self, session_id: &str, content: &str) -> Result<(), String> {
        self.sessions
            .append_message(session_id, ChatMessage::assistant(content))
            .await
            .map_err(|error| error.to_string())?;
        self.notifier.notify();
        Ok(())
    }

    /// Records the end of the run and wakes the studio.
    async fn finish_run(
        &self,
        session_id: &str,
        run_id: &str,
        result: &str,
        error: Option<String>,
    ) {
        if let Err(failure) = self
            .sessions
            .finish_run(session_id, run_id, result, error, Vec::new())
            .await
        {
            tracing::warn!(%failure, "recording the briefing run failed");
        }
        self.notifier.notify();
    }
}

/// The briefing system prompt. Simplified Technical English.
pub fn briefing_prompt(kind: ArtifactKind) -> String {
    let (role, order, owned) = match kind {
        ArtifactKind::Demo => (
            "This session builds a software demo: a landing page, app screens, or a similar layout on a device viewport.",
            "1. the artifact type;\n\
             2. the audience and the primary user goal;\n\
             3. the primary action or conversion goal;\n\
             4. required content, features, and constraints;\n\
             5. brand assets, visual direction, and accessibility needs;\n\
             6. technical constraints, when they matter.",
            "the artifact kind, the canvases to build for (desktop, phone, or tablet, one design each), and the number of variations",
        ),
        ArtifactKind::Deck => (
            "This session builds a deck: a slide presentation on a 1920 by 1080 px canvas.",
            "1. the scenario: a talk, a pitch, a lesson, a report, or a read-only deck;\n\
             2. the audience and what they must take away;\n\
             3. required content, data, and constraints;\n\
             4. brand assets, visual direction, and accessibility needs;\n\
             5. technical constraints, when they matter.",
            "the artifact kind, the number of slides, and the number of variations",
        ),
    };
    let schema = serde_json::to_string(&schemars::schema_for!(BriefingReply)).unwrap_or_default();
    format!(
        "You are a design partner. You turn a vague request into a clear design brief.\n\
         {role}\n\
         Ask only questions that change the result. Ask in this order of importance:\n\
         {order}\n\
         The app asks the user for {owned}. Never ask about these. Read them from the brief.\n\
         Ask at most {limit} questions per turn. Offer concise choices and set allow_other when free text helps.\n\
         Set required to false for a question the user may skip. The app adds a skip choice itself.\n\
         Never invent a brand, an audience, or a conversion goal. Ask, or leave it as an open question.\n\
         Reply with only one JSON object in this shape:\n\
         {{\"reply\":\"text for the user\",\"question_set\":{{...}}|null,\"brief\":{{...}}|null,\"is_complete\":false}}\n\
         The reply must conform to this JSON Schema:\n{schema}\n\
         Each question needs id, label, and kind. kind is one of single_select, multi_select, short_text, or long_text. \
         A select question needs options, each with value and label.\n\
         Example question: {{\"id\":\"platform\",\"label\":\"Which platform?\",\"kind\":\"single_select\",\"required\":true,\"options\":[{{\"value\":\"web\",\"label\":\"Web\"}}],\"allow_other\":true}}\n\
         Send question_set when you need answers. Do not set is_complete then.\n\
         Send brief and set is_complete to true when the brief is ready to present.\n\
         Keep confirmed_facts for what the user stated. Keep assumptions for what you decided. Keep open_questions for what is still unknown.\n\
         Write a confirmed fact only when no answer and no brief field already states it. The app shows the answers and the fields next to your facts. A fact that repeats one is deleted.\n\
         Write each fact and each assumption as one sentence of at most 12 words. Never write a fact as `question: answer`.\n\
         Ask instead of assuming when the choice changes the layout, the content, or the visual direction. Assume only the details a design must fill in either way.\n\
         When the request is already specific, skip the questions and send a complete brief.\n\
         Keep reply to 1 to 3 sentences.",
        limit = design_model::QUESTIONS_PER_TURN_LIMIT
    )
}

/// The briefing user input: request, conversation, answers, and the
/// brief so far.
fn briefing_input(
    request: &str,
    messages: &[ChatMessage],
    answered: &[(BriefQuestion, QuestionAnswer)],
    latest_brief: Option<&DesignBrief>,
) -> String {
    let mut input = format!("Request:\n{request}\n\n");
    if answered.is_empty() {
        input.push_str("Answers so far: none.\n\n");
    } else {
        input.push_str("Answers so far:\n");
        for (question, answer) in answered {
            if answer.skipped {
                input.push_str(&format!("- {}: (use your best judgment)\n", question.label));
            } else {
                input.push_str(&format!(
                    "- {}: {}\n",
                    question.label,
                    answer_text(question, answer)
                ));
            }
        }
        input.push('\n');
    }
    if messages.is_empty() {
        input.push_str("Conversation: none yet.\n\n");
    } else {
        input.push_str("Conversation, oldest first:\n");
        for message in messages {
            input.push_str(&format!("{}: {}\n", message.role, message.content));
        }
        input.push('\n');
    }
    if let Some(brief) = latest_brief
        && let Ok(json) = serde_json::to_string_pretty(brief)
    {
        input.push_str(&format!("The brief so far:\n{json}\n\n"));
    }
    input.push_str("Reply with only the JSON object.");
    input
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::{
        BriefingEngine, BriefingOutcome, BriefingReplyError, answered_questions_from_answers,
        assumptions_from_skipped_answers, briefing_prompt, parse_briefing_reply, tidy_brief,
    };
    use crate::events::ChangeNotifier;
    use crate::model_client::{LogSink, ModelClient};
    use crate::sessions::{NewSession, SessionStore};
    use crate::test_support::FakeModelServer;
    use design_model::{
        BriefQuestion, QuestionAnswer, QuestionKind, QuestionOption, WorkflowState,
    };

    fn engine(server: &FakeModelServer, sessions: &SessionStore) -> BriefingEngine {
        BriefingEngine::new(
            ModelClient::new(server.configuration(), None),
            sessions.clone(),
            ChangeNotifier::new(),
        )
    }

    fn silent_log() -> LogSink {
        Arc::new(|_line: &str| {})
    }

    async fn store() -> (tempfile::TempDir, SessionStore) {
        let directory = tempfile::tempdir().unwrap();
        let store = SessionStore::new(directory.path().join("sessions"));
        store
            .create(NewSession::demo("talk", "Talk", "Design a landing page."))
            .await
            .unwrap();
        (directory, store)
    }

    #[tokio::test]
    async fn a_vague_request_yields_a_question_set_and_clarifying() {
        let server = FakeModelServer::start().await;
        server.push_text(
            r#"{"reply":"Two things.","question_set":{"title":"Before I draft","message":"Two things.","questions":[{"id":"platform","label":"Which platform?","kind":"single_select","required":true,"options":[{"value":"web","label":"Web"},{"value":"app","label":"App"}]}]}}"#,
        );
        let (_directory, sessions) = store().await;
        let outcome = engine(&server, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, BriefingOutcome::AskedQuestions { .. }));
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Clarifying);
        assert_eq!(sessions.question_sets("talk").await.unwrap().len(), 1);
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("asked_questions"));
    }

    #[tokio::test]
    async fn a_specific_request_yields_a_brief_and_awaiting_approval() {
        let server = FakeModelServer::start().await;
        server.push_text(
            r#"{"reply":"Drafted.","is_complete":true,"brief":{"target_artifact":"landing page","audience":"retail investors","confirmed_facts":["Platform: web"]}}"#,
        );
        let (_directory, sessions) = store().await;
        let outcome = engine(&server, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            BriefingOutcome::BriefPresented { revision: 1 }
        ));
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::AwaitingApproval);
        let brief = sessions.latest_brief("talk").await.unwrap().unwrap();
        assert_eq!(brief.audience, "retail investors");
        assert_eq!(brief.request, "Design a landing page.");
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("brief_presented"));
    }

    const BAD_SHAPE: &str = r#"{"reply":"Two things.","question_set":{"title":"T","message":"m","questions":[{"id":"platform","label":"Which platform?"}]}}"#;

    #[tokio::test]
    async fn a_reply_with_the_wrong_shape_is_sent_back_for_a_correction() {
        let server = FakeModelServer::start().await;
        server.push_text(BAD_SHAPE);
        server.push_text(
            r#"{"reply":"Two things.","question_set":{"title":"T","message":"m","questions":[{"id":"platform","label":"Which platform?","kind":"single_select","options":[{"value":"web","label":"Web"}]}]}}"#,
        );
        let (_directory, sessions) = store().await;
        let outcome = engine(&server, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        assert!(matches!(outcome, BriefingOutcome::AskedQuestions { .. }));
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let messages = requests[1]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2]["content"].as_str().unwrap(), BAD_SHAPE);
        let correction = messages[3]["content"].as_str().unwrap();
        assert!(correction.contains("could not be read"));
        assert!(correction.contains("missing field `kind`"));
    }

    #[tokio::test]
    async fn correction_rounds_stop_after_the_limit() {
        let server = FakeModelServer::start().await;
        for _ in 0..=super::REPAIR_ROUND_LIMIT {
            server.push_text(BAD_SHAPE);
        }
        let (_directory, sessions) = store().await;
        let message = engine(&server, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap_err();
        assert!(message.contains("could not be read"));
        assert_eq!(server.requests().len(), super::REPAIR_ROUND_LIMIT + 1);
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("failed"));
    }

    #[tokio::test]
    async fn a_prose_reply_keeps_the_state_and_prior_answers() {
        let server = FakeModelServer::start().await;
        server.push_text("I need to think about this.");
        let (_directory, sessions) = store().await;
        let result = engine(&server, &sessions).run("talk", silent_log()).await;
        assert!(result.is_err());
        let session = sessions.read("talk").await.unwrap().unwrap();
        assert_eq!(session.state, WorkflowState::Intake);
    }

    #[tokio::test]
    async fn the_briefing_run_writes_no_design_files() {
        let server = FakeModelServer::start().await;
        server.push_text(
            r#"{"is_complete":true,"brief":{"target_artifact":"page","audience":"devs"}}"#,
        );
        let directory = tempfile::tempdir().unwrap();
        let sessions = SessionStore::new(directory.path().join("sessions"));
        sessions
            .create(NewSession::demo("talk", "Talk", "A page."))
            .await
            .unwrap();
        engine(&server, &sessions)
            .run("talk", silent_log())
            .await
            .unwrap();
        // The briefing engine holds no design store, so nothing under a
        // designs directory can exist.
        assert!(!directory.path().join("designs").exists());
    }

    #[tokio::test]
    async fn a_provider_failure_names_the_runtime_and_keeps_answers() {
        let server = FakeModelServer::start().await;
        server.push_status(500, "quota exceeded");
        let (_directory, sessions) = store().await;
        let result = engine(&server, &sessions).run("talk", silent_log()).await;
        let message = result.unwrap_err();
        assert!(message.contains("fake"));
        let runs = sessions.runs("talk").await.unwrap();
        assert_eq!(runs[0].result.as_deref(), Some("failed"));
    }

    fn question(id: &str, label: &str) -> BriefQuestion {
        BriefQuestion {
            id: id.to_owned(),
            label: label.to_owned(),
            rationale: None,
            kind: QuestionKind::SingleSelect,
            required: true,
            options: vec![QuestionOption {
                value: "web".to_owned(),
                label: "Web".to_owned(),
            }],
            allow_other: false,
        }
    }

    #[test]
    fn a_question_set_reply_parses() {
        let reply = parse_briefing_reply(
            r#"Sure. {"reply":"Two things.","question_set":{"title":"T","message":"m","questions":[{"id":"platform","label":"Which platform?","kind":"single_select","required":true,"options":[{"value":"web","label":"Web"}]}]}}"#,
        )
        .unwrap();
        assert!(reply.question_set.is_some());
        assert!(!reply.is_complete);
    }

    #[test]
    fn a_complete_brief_reply_parses() {
        let reply = parse_briefing_reply(
            r#"{"reply":"Drafted.","brief":{"request":"x","audience":"devs"},"is_complete":true}"#,
        )
        .unwrap();
        assert!(reply.is_complete);
        assert_eq!(reply.brief.unwrap().audience, "devs");
    }

    #[test]
    fn a_prose_reply_is_not_json() {
        assert!(matches!(
            parse_briefing_reply("no json here"),
            Err(BriefingReplyError::NotJson)
        ));
    }

    #[test]
    fn is_complete_without_a_brief_is_rejected() {
        assert!(matches!(
            parse_briefing_reply(r#"{"is_complete":true}"#),
            Err(BriefingReplyError::BriefWithoutCompletion)
        ));
    }

    #[test]
    fn a_four_question_set_is_rejected_with_every_error() {
        let raw = r#"{"question_set":{"title":"T","message":"m","questions":[
            {"id":"a","label":"A?","kind":"short_text"},
            {"id":"b","label":"B?","kind":"short_text"},
            {"id":"c","label":"C?","kind":"short_text"},
            {"id":"d","label":"D?","kind":"short_text"}]}}"#;
        match parse_briefing_reply(raw) {
            Err(BriefingReplyError::InvalidQuestionSet(problems)) => {
                assert!(!problems.is_empty());
            }
            other => panic!("expected an invalid question set, got {other:?}"),
        }
    }

    #[test]
    fn tidy_brief_drops_the_facts_the_panel_already_shows() {
        let mut brief = design_model::DesignBrief {
            audience: "developers".to_owned(),
            target_platform: "desktop web app".to_owned(),
            primary_job: "organize project or coding work".to_owned(),
            answered_questions: vec![
                design_model::AnsweredQuestion {
                    question: "What platform?".to_owned(),
                    answer: "Desktop web app".to_owned(),
                    is_assumed: false,
                },
                design_model::AnsweredQuestion {
                    question: "Primary action?".to_owned(),
                    answer: "All.".to_owned(),
                    is_assumed: false,
                },
            ],
            confirmed_facts: vec![
                "What should be the primary action?: All.".to_owned(),
                "The artifact is a TODO app demo.".to_owned(),
                "The target platform is desktop web app.".to_owned(),
                "The audience is developers.".to_owned(),
                "The main goal is to organize project or coding work.".to_owned(),
                // `All.` is too short to match, so this one survives.
                "The primary action covers all core task actions.".to_owned(),
                "the artifact is a todo app demo".to_owned(),
            ],
            assumptions: vec![
                "Use sample tasks.".to_owned(),
                "Use sample tasks!".to_owned(),
                "Assumed for `Which features?`: best judgment".to_owned(),
                String::new(),
            ],
            ..design_model::DesignBrief::default()
        };
        tidy_brief(&mut brief);
        assert_eq!(
            brief.confirmed_facts,
            vec![
                "The artifact is a TODO app demo.",
                "The primary action covers all core task actions.",
            ]
        );
        assert_eq!(brief.assumptions, vec!["Use sample tasks."]);
    }

    #[test]
    fn a_brief_with_nothing_to_drop_is_left_alone() {
        let mut brief = design_model::DesignBrief {
            confirmed_facts: vec!["The deadline is March.".to_owned()],
            ..design_model::DesignBrief::default()
        };
        let original = brief.clone();
        tidy_brief(&mut brief);
        assert_eq!(brief, original);
    }

    #[test]
    fn answered_questions_keep_the_question_and_the_answer_apart() {
        let answered = vec![
            (
                question("platform", "Which platform?"),
                QuestionAnswer {
                    question_id: "platform".to_owned(),
                    values: vec!["web".to_owned()],
                    ..QuestionAnswer::default()
                },
            ),
            (
                question("tone", "Which tone?"),
                QuestionAnswer {
                    question_id: "tone".to_owned(),
                    skipped: true,
                    ..QuestionAnswer::default()
                },
            ),
        ];
        let entries = answered_questions_from_answers(&answered);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].question, "Which platform?");
        assert_eq!(entries[0].answer, "Web");
        assert!(!entries[0].is_assumed);
        assert_eq!(entries[1].question, "Which tone?");
        assert_eq!(entries[1].answer, "");
        assert!(entries[1].is_assumed);
        // No entry reads as `question: answer`, so nothing lands in the
        // fact list looking like a fact.
        assert!(!entries.iter().any(|entry| entry.answer.contains(':')));
        assert_eq!(
            assumptions_from_skipped_answers(&answered),
            vec!["The agent chooses the answer to: Which tone?"]
        );
    }

    #[test]
    fn briefing_prompts_carry_the_reply_schema_and_the_question_fields() {
        let prompt = briefing_prompt(design_model::ArtifactKind::Demo);
        assert!(prompt.contains("JSON Schema"));
        assert!(prompt.contains("\"single_select\""));
        assert!(prompt.contains("\"allow_other\""));
        assert!(prompt.contains("\"confirmed_facts\""));
        assert!(prompt.contains("Each question needs id, label, and kind."));
    }

    #[test]
    fn an_invalid_question_set_error_names_every_problem() {
        let raw = r#"{"question_set":{"title":"T","message":"m","questions":[]}}"#;
        let error = parse_briefing_reply(raw).unwrap_err();
        assert!(error.to_string().contains("has no questions"));
    }

    #[test]
    fn briefing_prompts_state_the_priorities_and_the_three_question_limit() {
        let prompt = briefing_prompt(design_model::ArtifactKind::Demo);
        assert!(prompt.contains("at most 3 questions"));
        assert!(prompt.contains("1. the artifact type;"));
        assert!(prompt.contains("software demo"));
        assert!(prompt.contains("Never invent a brand"));
        assert!(prompt.contains("is_complete"));
    }

    #[test]
    fn the_deck_briefing_prompt_asks_the_scenario_first() {
        let prompt = briefing_prompt(design_model::ArtifactKind::Deck);
        assert!(prompt.contains("builds a deck"));
        assert!(prompt.contains("1. the scenario"));
        assert!(prompt.contains("2. the audience"));
        assert!(prompt.contains("at most 3 questions"));
    }

    #[test]
    fn the_prompt_caps_the_lines_and_says_when_to_ask_instead_of_assume() {
        let prompt = briefing_prompt(design_model::ArtifactKind::Demo);
        assert!(prompt.contains("one sentence of at most 12 words"));
        assert!(prompt.contains("A fact that repeats one is deleted."));
        assert!(prompt.contains(
            "Ask instead of assuming when the choice changes the layout, the content, or the visual direction."
        ));
    }

    #[test]
    fn each_prompt_names_what_the_app_asks_so_the_model_does_not() {
        let demo = briefing_prompt(design_model::ArtifactKind::Demo);
        assert!(demo.contains(
            "The app asks the user for the artifact kind, the canvases to build for (desktop, phone, or tablet, one design each), and the number of variations. Never ask about these."
        ));
        assert!(demo.contains("Never write a fact as `question: answer`"));
        let deck = briefing_prompt(design_model::ArtifactKind::Deck);
        assert!(deck.contains(
            "The app asks the user for the artifact kind, the number of slides, and the number of variations. Never ask about these."
        ));
        // The deck prompt still must not send the model after a viewport.
        assert!(!deck.contains("1. the artifact type"));
    }
}
