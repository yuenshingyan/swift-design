//! Agent questions: choices the agent hands back to the user.
//!
//! Flow: the agent reads the brief, PUTs open questions to
//! `/questions`, and ends its turn. The user answers them in the UI;
//! the answers are appended to the brief and the questions are closed.
//! The user re-runs the agent, which reads the updated brief from
//! `GET /briefs`.

use std::path::PathBuf;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api_error;
use crate::briefs::{BriefAnswer, BriefStore};
use crate::events::ChangeNotifier;

/// Most questions one round may ask. Keeps the form short.
pub const QUESTION_LIMIT: usize = 5;

/// Filesystem-backed question storage: one JSON file.
#[derive(Clone)]
pub struct QuestionStore {
    path: PathBuf,
}

/// One question with its preset answer options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Question {
    /// The question text.
    pub question: String,
    /// Preset answers the user can pick. The UI always adds a
    /// "You decide" option, so this list may be empty.
    #[serde(default)]
    pub options: Vec<String>,
}

/// Body of `PUT /questions`.
#[derive(Debug, Deserialize)]
struct QuestionsRequest {
    /// The questions to show the user.
    questions: Vec<Question>,
}

/// One answered question in `POST /questions/answers`.
#[derive(Debug, Deserialize)]
struct Answer {
    /// The question text, repeated for the brief.
    question: String,
    /// The chosen answer, or "You decide".
    answer: String,
}

/// Body of `POST /questions/answers`.
#[derive(Debug, Deserialize)]
struct AnswersRequest {
    /// One entry per answered question.
    answers: Vec<Answer>,
}

impl QuestionStore {
    /// Creates a store over `path`. Parent directories are created on
    /// the first save.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads the open questions. `Ok(None)` means none are open.
    pub async fn read(&self) -> anyhow::Result<Option<Vec<Question>>> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => Ok(Some(serde_json::from_str(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes the open questions, creating parent directories when
    /// needed.
    pub async fn write(&self, questions: &[Question]) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.path, serde_json::to_string_pretty(questions)?).await?;
        Ok(())
    }

    /// Removes the questions file. Missing files are fine.
    pub async fn clear(&self) -> anyhow::Result<()> {
        match tokio::fs::remove_file(&self.path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// The `/questions` route table.
pub fn routes() -> Router<crate::AppState> {
    Router::new()
        .route("/questions", get(get_questions).put(put_questions))
        .route("/questions/answers", post(post_answers))
}

/// Returns the open questions.
async fn get_questions(State(store): State<QuestionStore>) -> Response {
    match store.read().await {
        Ok(Some(questions)) => Json(questions).into_response(),
        Ok(None) => api_error::error_response(
            StatusCode::NOT_FOUND,
            "no open questions: PUT /questions to ask some",
            Vec::new(),
        ),
        Err(error) => api_error::internal_error(&error),
    }
}

/// Validates and saves the agent's questions for the user.
async fn put_questions(
    State(store): State<QuestionStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<QuestionsRequest>,
) -> Response {
    if request.questions.is_empty() || request.questions.len() > QUESTION_LIMIT {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!(
                "send between 1 and {QUESTION_LIMIT} questions, got {}",
                request.questions.len()
            ),
            Vec::new(),
        );
    }
    let empty_questions = request
        .questions
        .iter()
        .filter(|question| question.question.trim().is_empty())
        .count();
    if empty_questions > 0 {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "every question needs text",
            Vec::new(),
        );
    }
    match store.write(&request.questions).await {
        Ok(()) => {
            notifier.notify();
            tracing::info!(count = request.questions.len(), "questions saved");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => api_error::internal_error(&error),
    }
}

/// Appends the user's answers to the brief and closes the questions.
async fn post_answers(
    State(questions): State<QuestionStore>,
    State(briefs): State<BriefStore>,
    State(notifier): State<ChangeNotifier>,
    Json(request): Json<AnswersRequest>,
) -> Response {
    if request.answers.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no answers in request",
            Vec::new(),
        );
    }
    let count = request.answers.len();
    let answers = request
        .answers
        .into_iter()
        .map(|answer| BriefAnswer {
            question: answer.question,
            answer: answer.answer,
        })
        .collect();
    if let Err(error) = briefs.append_answers(answers).await {
        return api_error::internal_error(&error);
    }
    if let Err(error) = questions.clear().await {
        return api_error::internal_error(&error);
    }
    notifier.notify();
    tracing::info!(count, "answers appended to brief");
    StatusCode::NO_CONTENT.into_response()
}
