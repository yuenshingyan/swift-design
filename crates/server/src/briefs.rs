//! The design brief: the user's prompt and the conversation that follows.
//!
//! The UI writes the brief with `PUT /briefs` and adds chat turns with
//! `POST /briefs/messages`. The agent reads it with `GET /briefs`.
//! Question answers land in `answers` and, as turns, in `messages`.
//! Storage is one JSON file, internal to the server.

use std::path::PathBuf;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api_error;
use crate::candidates::CANDIDATE_LIMIT;
use crate::events::ChangeNotifier;

/// Effort level when the brief does not state one.
fn default_effort() -> String {
    "medium".to_owned()
}

/// Preview mode when the brief does not state one: on.
fn default_preview() -> bool {
    true
}

/// Screens in a preview candidate. Enough to show the theme, the layout
/// language, and the text density before the user picks one to
/// continue.
pub const PREVIEW_SCREEN_COUNT: usize = 3;

/// The `action` of a user message that asks the engine to continue a
/// preview design to its full length.
pub const CONTINUE_ACTION: &str = "continue";

/// The stored brief, served to the agent as JSON.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Brief {
    /// What the design should cover, in the user's words.
    pub prompt: String,
    /// What scenario the design is for, as the user answered. `None`
    /// until the user answers the scenario question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    /// The target screen count as `min-max`, like `10-15`, or `any`
    /// when the user left it to the model. `None` until the user
    /// answers the length question. A target, not a hard limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<String>,
    /// How many candidate designs the agent should write. `None` until
    /// the user answers the count question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variations: Option<usize>,
    /// Project name: the design id prefix that groups this brief's designs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// How hard to work: `low`, `medium`, or `high`.
    #[serde(default = "default_effort")]
    pub effort: String,
    /// True to write each candidate as a preview: the first
    /// `PREVIEW_SCREEN_COUNT` screens plus the outline of the complete
    /// design. The user continues the candidate they pick. False writes
    /// complete candidates at once.
    #[serde(default = "default_preview")]
    pub preview: bool,
    /// How different the candidates should be: `low`, `medium`, or
    /// `high`. `None` until the user answers the variety question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variety: Option<String>,
    /// The ids of the templates the candidates follow, from
    /// `GET /templates`. Candidates take one look each, in order, and
    /// wrap when there are more candidates than templates. Empty writes
    /// a new look for every candidate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<String>,
    /// The single template id briefs saved before multi-select used.
    /// Read only when `templates` is empty. Never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// The user's answers to the agent's questions, newest last.
    #[serde(default)]
    pub answers: Vec<BriefAnswer>,
    /// The conversation after the prompt: user and assistant turns,
    /// oldest first. Answered questions appear here as turns too.
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

impl Brief {
    /// The template ids the run must use. It reads the legacy single
    /// `template` field only when `templates` is empty, so a brief saved
    /// before multi-select still styles its candidates.
    pub fn template_ids(&self) -> Vec<String> {
        if !self.templates.is_empty() {
            return self.templates.clone();
        }
        self.template.iter().cloned().collect()
    }

    /// The latest user turn, when there is one.
    fn latest_user_message(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
    }

    /// The design the latest user turn was about, when the user sent it
    /// from the editor.
    pub fn editing_design(&self) -> Option<&str> {
        self.latest_user_message()
            .and_then(|message| message.design.as_deref())
    }

    /// The preview designs the user asked to continue and that still
    /// wait: the design ids of the `continue` turns after the last plain
    /// user turn, oldest first, each once. A plain user turn ends the
    /// queue: the user moved on.
    pub fn continue_requests(&self) -> Vec<&str> {
        let queued: Vec<&str> = self
            .messages
            .iter()
            .rev()
            .filter(|message| message.role == "user")
            .take_while(|message| message.action.as_deref() == Some(CONTINUE_ACTION))
            .filter_map(|message| message.design.as_deref())
            .collect();
        let mut requests: Vec<&str> = Vec::new();
        for design in queued.into_iter().rev() {
            if !requests.contains(&design) {
                requests.push(design);
            }
        }
        requests
    }

    /// How many screens a candidate gets when it is a preview: `None`
    /// when the brief asks for complete candidates, or when the target
    /// length is no longer than a preview.
    pub fn preview_screen_count(&self) -> Option<usize> {
        if !self.preview {
            return None;
        }
        match self.length_bounds() {
            Some((_, max)) if max <= PREVIEW_SCREEN_COUNT => None,
            _ => Some(PREVIEW_SCREEN_COUNT),
        }
    }

    /// The target screen range `(min, max)`, when the user chose one.
    pub fn length_bounds(&self) -> Option<(usize, usize)> {
        self.length
            .as_deref()
            .and_then(crate::candidate_questions::length_bounds)
    }

    /// The candidate count to write: the chosen count, or the default
    /// when the user has not chosen one.
    pub fn variation_count(&self) -> usize {
        self.variations
            .unwrap_or(crate::candidate_questions::DEFAULT_VARIATIONS)
            .max(1)
    }
}

/// One conversation turn kept with the brief.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    /// `user` or `assistant`.
    pub role: String,
    /// The turn text.
    pub content: String,
    /// The design open in the editor when the user sent this turn. A
    /// request about that design is applied to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,
    /// What the turn asks the engine to do besides chat. `continue`
    /// asks it to write the remaining screens of the preview design named
    /// in `design`. Absent for a plain message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl ChatMessage {
    /// A user turn, optionally about the design open in the editor.
    pub fn user(content: &str, design: Option<&str>) -> Self {
        Self {
            role: "user".to_owned(),
            content: content.to_owned(),
            design: design.map(str::to_owned),
            action: None,
        }
    }

    /// A user turn that asks the engine to continue the preview design
    /// `design` to its full length.
    pub fn continue_design(content: &str, design: &str) -> Self {
        Self {
            role: "user".to_owned(),
            content: content.to_owned(),
            design: Some(design.to_owned()),
            action: Some(CONTINUE_ACTION.to_owned()),
        }
    }

    /// An assistant turn.
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_owned(),
            content: content.to_owned(),
            design: None,
            action: None,
        }
    }
}

/// Body of `POST /briefs/messages`.
#[derive(Debug, Deserialize)]
struct MessageRequest {
    /// `user` (default) or `assistant`.
    #[serde(default = "default_role")]
    role: String,
    /// The turn text.
    content: String,
    /// The design open in the editor, when the message is about one.
    #[serde(default)]
    design: Option<String>,
    /// `continue` to ask the engine to write the remaining screens of
    /// the preview design in `design`. Absent for a plain message.
    #[serde(default)]
    action: Option<String>,
}

/// The role when a message request does not state one.
fn default_role() -> String {
    "user".to_owned()
}

/// One answered question, kept with the brief.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BriefAnswer {
    /// The question text.
    pub question: String,
    /// The chosen answer, or `You decide`.
    pub answer: String,
}

/// Body of `PUT /briefs`.
#[derive(Debug, Deserialize)]
struct BriefRequest {
    /// What the design should cover, in the user's words.
    prompt: String,
    /// How many candidate designs the agent should write. Optional: the
    /// app asks the user when absent.
    #[serde(default)]
    variations: Option<usize>,
    /// Project name for this brief's designs.
    #[serde(default)]
    project: Option<String>,
    /// How hard to work: `low`, `medium`, or `high`.
    #[serde(default = "default_effort")]
    effort: String,
    /// True to write preview candidates first. On when absent.
    #[serde(default = "default_preview")]
    preview: bool,
    /// The templates the candidates follow, from `GET /templates`.
    /// Empty writes a new look for every candidate.
    #[serde(default)]
    templates: Vec<String>,
}

/// Filesystem-backed brief storage: one JSON file.
#[derive(Clone)]
pub struct BriefStore {
    path: PathBuf,
}

impl BriefStore {
    /// Creates a store over `path`. Parent directories are created on
    /// the first save.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the current brief. `Ok(None)` means no brief exists.
    pub async fn read(&self) -> anyhow::Result<Option<Brief>> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes the brief, creating parent directories when needed.
    pub async fn write(&self, brief: &Brief) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.path, serde_json::to_string_pretty(brief)?).await?;
        Ok(())
    }

    /// Appends answers to the stored brief, as answers and as
    /// question-and-answer turns. Creates a brief with an empty prompt
    /// when none exists, so answers are never lost.
    pub async fn append_answers(&self, answers: Vec<BriefAnswer>) -> anyhow::Result<()> {
        let mut brief = self.read().await?.unwrap_or_default();
        for answer in &answers {
            if crate::candidate_questions::is_scenario_question(&answer.question) {
                brief.scenario = Some(crate::candidate_questions::scenario_from_answer(
                    &answer.answer,
                ));
            }
            if crate::candidate_questions::is_length_question(&answer.question) {
                brief.length = Some(crate::candidate_questions::length_from_answer(
                    &answer.answer,
                ));
            }
            if crate::candidate_questions::is_variation_question(&answer.question) {
                brief.variations = Some(crate::candidate_questions::count_from_answer(
                    &answer.answer,
                ));
            }
            if crate::candidate_questions::is_variety_question(&answer.question) {
                brief.variety = Some(crate::candidate_questions::level_from_answer(
                    &answer.answer,
                ));
            }
            brief
                .messages
                .push(ChatMessage::assistant(&answer.question));
            brief.messages.push(ChatMessage::user(&answer.answer, None));
        }
        brief.answers.extend(answers);
        self.write(&brief).await
    }

    /// Appends one conversation turn. Creates a brief with an empty
    /// prompt when none exists.
    pub async fn append_message(&self, message: ChatMessage) -> anyhow::Result<()> {
        let mut brief = self.read().await?.unwrap_or_default();
        brief.messages.push(message);
        self.write(&brief).await
    }

    /// Points the brief at `new` when it belongs to project `old`.
    pub async fn rename_project(&self, old: &str, new: &str) -> anyhow::Result<()> {
        let Some(mut brief) = self.read().await? else {
            return Ok(());
        };
        if brief.project.as_deref() != Some(old) {
            return Ok(());
        }
        brief.project = Some(new.to_owned());
        self.write(&brief).await
    }

    /// Removes the brief. Missing files are fine.
    pub async fn clear(&self) -> anyhow::Result<()> {
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// The `/briefs` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route(
            "/briefs",
            get(get_brief).put(put_brief).delete(delete_brief),
        )
        .route("/briefs/messages", post(post_message))
}

/// Appends a conversation turn. A user turn closes any open questions:
/// the user chose to type instead of answering.
async fn post_message(
    State(store): State<BriefStore>,
    State(questions): State<crate::questions::QuestionStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<MessageRequest>,
) -> Response {
    if !matches!(request.role.as_str(), "user" | "assistant") {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("role `{}` is unknown: use user or assistant", request.role),
            Vec::new(),
        );
    }
    let content = request.content.trim();
    if content.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "message content is empty: write the message text",
            Vec::new(),
        );
    }
    if let Some(design) = &request.design
        && !crate::designs::is_valid_design_id(design)
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("design `{design}` is not a valid design id"),
            Vec::new(),
        );
    }
    if let Some(action) = &request.action
        && action != CONTINUE_ACTION
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("action `{action}` is unknown: use {CONTINUE_ACTION}, or omit it"),
            Vec::new(),
        );
    }
    if request.action.is_some() && (request.role != "user" || request.design.is_none()) {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "a continue message needs role user and the design id to continue",
            Vec::new(),
        );
    }
    if store.read().await.ok().flatten().is_none() {
        return api_error::error_response(
            StatusCode::NOT_FOUND,
            "no brief exists: PUT /briefs before you send messages",
            Vec::new(),
        );
    }
    let message = match (request.role.as_str(), &request.action, &request.design) {
        ("user", Some(_), Some(design)) => ChatMessage::continue_design(content, design),
        ("user", _, design) => ChatMessage::user(content, design.as_deref()),
        _ => ChatMessage::assistant(content),
    };
    if let Err(error) = store.append_message(message).await {
        return api_error::internal_error(&error);
    }
    if request.role == "user"
        && let Err(error) = questions.clear().await
    {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!(role = %request.role, size = content.len(), "message appended");
    StatusCode::NO_CONTENT.into_response()
}

/// Clears the brief and any open questions: a clean start.
async fn delete_brief(
    State(store): State<BriefStore>,
    State(questions): State<crate::questions::QuestionStore>,
    State(notifier): State<ChangeNotifier>,
) -> Response {
    if let Err(error) = store.clear().await {
        return api_error::internal_error(&error);
    }
    if let Err(error) = questions.clear().await {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!("brief cleared");
    StatusCode::NO_CONTENT.into_response()
}

/// Returns the current brief as JSON.
async fn get_brief(State(store): State<BriefStore>) -> Response {
    match store.read().await {
        Ok(Some(brief)) => Json(brief).into_response(),
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            "no brief exists: PUT /briefs to write one",
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Validates and saves the brief for the agent to pick up. A new brief
/// starts with no answers, and stale open questions are discarded.
async fn put_brief(
    State(store): State<BriefStore>,
    State(questions): State<crate::questions::QuestionStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<BriefRequest>,
) -> Response {
    if request.prompt.trim().is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "brief prompt is empty: describe what the design should cover",
            Vec::new(),
        );
    }
    if let Some(variations) = request.variations
        && (variations == 0 || variations > CANDIDATE_LIMIT)
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("variations must be between 1 and {CANDIDATE_LIMIT}, got {variations}"),
            Vec::new(),
        );
    }
    let project = request
        .project
        .map(|project| project.trim().to_owned())
        .filter(|project| !project.is_empty());
    if let Some(project) = &project
        && (!crate::designs::is_valid_design_id(project) || project.contains("-candidate-"))
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "project `{project}` is not a valid name: use kebab-case without `-candidate-`"
            ),
            Vec::new(),
        );
    }
    if !matches!(request.effort.as_str(), "low" | "medium" | "high") {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "effort `{}` is unknown: use low, medium, or high",
                request.effort
            ),
            Vec::new(),
        );
    }
    let brief = Brief {
        prompt: request.prompt.trim().to_owned(),
        variations: request.variations,
        project,
        effort: request.effort,
        preview: request.preview,
        variety: None,
        scenario: None,
        length: None,
        templates: request
            .templates
            .into_iter()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect(),
        template: None,
        answers: Vec::new(),
        messages: Vec::new(),
    };
    match store.write(&brief).await {
        Ok(()) => {
            if let Err(error) = questions.clear().await {
                return api_error::internal_error(&error);
            }
            notifier.notify();
            tracing::info!(variations = ?brief.variations, "brief saved");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::briefs::{Brief, BriefAnswer, BriefStore, ChatMessage};

    #[tokio::test]
    async fn append_answers_creates_a_brief_when_none_exists() {
        let directory = tempfile::tempdir().unwrap();
        let store = BriefStore::new(directory.path().join("brief.json"));
        store
            .append_answers(vec![BriefAnswer {
                question: "Who is in the room?".to_owned(),
                answer: "Engineers".to_owned(),
            }])
            .await
            .unwrap();
        let brief = store.read().await.unwrap().unwrap();
        assert_eq!(brief.prompt, "");
        assert_eq!(brief.answers.len(), 1);
        assert_eq!(brief.answers[0].answer, "Engineers");
        assert_eq!(
            brief.messages,
            vec![
                ChatMessage::assistant("Who is in the room?"),
                ChatMessage::user("Engineers", None),
            ]
        );
    }

    #[tokio::test]
    async fn append_message_keeps_turn_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = BriefStore::new(directory.path().join("brief.json"));
        store
            .append_message(ChatMessage::user("Make it shorter.", Some("talk")))
            .await
            .unwrap();
        store
            .append_message(ChatMessage::assistant("Done."))
            .await
            .unwrap();
        let brief = store.read().await.unwrap().unwrap();
        assert_eq!(brief.messages.len(), 2);
        assert_eq!(brief.messages[0].role, "user");
        assert_eq!(brief.messages[1].content, "Done.");
        assert_eq!(brief.messages[0].design.as_deref(), Some("talk"));
        assert_eq!(brief.editing_design(), Some("talk"));
        assert!(brief.continue_requests().is_empty());
    }

    #[test]
    fn continue_turns_queue_until_the_next_plain_user_turn() {
        let mut brief = Brief {
            messages: vec![ChatMessage::continue_design(
                "Continue it.",
                "talk-candidate-2",
            )],
            ..Brief::default()
        };
        assert_eq!(brief.continue_requests(), vec!["talk-candidate-2"]);
        assert_eq!(brief.editing_design(), Some("talk-candidate-2"));
        // More requests queue in order, each design once, across
        // assistant turns.
        brief.messages.push(ChatMessage::continue_design(
            "Continue.",
            "talk-candidate-4",
        ));
        brief.messages.push(ChatMessage::assistant("Done."));
        brief
            .messages
            .push(ChatMessage::continue_design("Again.", "talk-candidate-2"));
        assert_eq!(
            brief.continue_requests(),
            vec!["talk-candidate-2", "talk-candidate-4"]
        );
        brief.messages.push(ChatMessage::user("Thanks.", None));
        assert!(brief.continue_requests().is_empty());
        let json = serde_json::to_string(&brief.messages[0]).unwrap();
        assert!(json.contains("\"action\":\"continue\""));
        assert!(
            !serde_json::to_string(&brief.messages[2])
                .unwrap()
                .contains("action")
        );
    }

    #[test]
    fn preview_screen_counts_follow_the_mode_and_the_length() {
        let brief = Brief {
            preview: true,
            ..Brief::default()
        };
        assert_eq!(
            brief.preview_screen_count(),
            Some(super::PREVIEW_SCREEN_COUNT)
        );
        let long = Brief {
            length: Some("10-15".to_owned()),
            ..brief.clone()
        };
        assert_eq!(
            long.preview_screen_count(),
            Some(super::PREVIEW_SCREEN_COUNT)
        );
        let tiny = Brief {
            length: Some("2-3".to_owned()),
            ..brief.clone()
        };
        assert_eq!(tiny.preview_screen_count(), None);
        let full = Brief {
            preview: false,
            ..brief
        };
        assert_eq!(full.preview_screen_count(), None);
        let parsed: Brief = serde_json::from_str("{\"prompt\":\"x\"}").unwrap();
        assert!(parsed.preview);
    }

    #[tokio::test]
    async fn append_answers_keeps_the_existing_prompt() {
        let directory = tempfile::tempdir().unwrap();
        let store = BriefStore::new(directory.path().join("brief.json"));
        store
            .write(&Brief {
                prompt: "A talk.".to_owned(),
                variations: Some(2),
                project: None,
                effort: "medium".to_owned(),
                preview: true,
                variety: None,
                scenario: None,
                length: None,
                templates: Vec::new(),
                template: None,
                answers: Vec::new(),
                messages: Vec::new(),
            })
            .await
            .unwrap();
        store
            .append_answers(vec![BriefAnswer {
                question: "How long?".to_owned(),
                answer: "10 min".to_owned(),
            }])
            .await
            .unwrap();
        let brief = store.read().await.unwrap().unwrap();
        assert_eq!(brief.prompt, "A talk.");
        assert_eq!(brief.variations, Some(2));
        assert_eq!(brief.answers[0].question, "How long?");
        assert_eq!(brief.variety, None);
        store
            .append_answers(vec![
                BriefAnswer {
                    question: crate::candidate_questions::SCENARIO_QUESTION.to_owned(),
                    answer: "Finance".to_owned(),
                },
                BriefAnswer {
                    question: crate::candidate_questions::LENGTH_QUESTION.to_owned(),
                    answer: "Standard: 10 to 15 screens".to_owned(),
                },
                BriefAnswer {
                    question: crate::candidate_questions::VARIATION_QUESTION.to_owned(),
                    answer: "4 candidates".to_owned(),
                },
                BriefAnswer {
                    question: crate::candidate_questions::VARIETY_QUESTION.to_owned(),
                    answer: "High: new themes, structure, and angle".to_owned(),
                },
            ])
            .await
            .unwrap();
        let brief = store.read().await.unwrap().unwrap();
        assert_eq!(brief.scenario.as_deref(), Some("Finance"));
        assert_eq!(brief.length.as_deref(), Some("10-15"));
        assert_eq!(brief.variations, Some(4));
        assert_eq!(brief.variety.as_deref(), Some("high"));
    }

    #[test]
    fn template_ids_prefer_the_list_over_the_legacy_field() {
        let raw = r#"{"prompt":"x","variations":2,"templates":["a","b"],"template":"old"}"#;
        let brief: Brief = serde_json::from_str(raw).unwrap();
        assert_eq!(brief.template_ids(), ["a", "b"]);
    }

    #[test]
    fn a_brief_saved_before_multi_select_still_names_its_template() {
        let raw = r#"{"prompt":"x","variations":2,"template":"old"}"#;
        let brief: Brief = serde_json::from_str(raw).unwrap();
        assert!(brief.templates.is_empty());
        assert_eq!(brief.template_ids(), ["old"]);
    }

    #[test]
    fn a_brief_naming_no_template_writes_a_new_look() {
        let raw = r#"{"prompt":"x","variations":2}"#;
        let brief: Brief = serde_json::from_str(raw).unwrap();
        assert!(brief.template_ids().is_empty());
    }
}
