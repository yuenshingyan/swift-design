use super::*;

/// Body of `POST /sessions/{id}/messages`.
#[derive(Debug, Deserialize)]
pub(super) struct MessageRequest {
    content: String,
    #[serde(default)]
    design: Option<String>,
    /// Candidate ids of this session the turn edits.
    #[serde(default)]
    pinned: Vec<String>,
    #[serde(default)]
    action: Option<String>,
}

/// Why the pinned candidates of a message are refused: an id that is
/// not a candidate id, or one that belongs to another session.
pub(super) fn pinned_problem(session_id: &str, pinned: &[String]) -> Option<String> {
    pinned.iter().find_map(|id| {
        if !is_valid_design_id(id) {
            return Some(format!("pinned candidate `{id}` is not a valid id"));
        }
        if session_id_of_artifact(id) != session_id {
            return Some(format!(
                "pinned candidate `{id}` does not belong to session `{session_id}`"
            ));
        }
        None
    })
}

/// Why a regenerate message is refused: it names no artifact, or its
/// content names no unit of the session's kind.
pub(super) fn regenerate_problem(session: &Session, request: &MessageRequest) -> Option<String> {
    if request.design.is_none() {
        return Some("a regenerate message needs the design id whose units it rewrites".to_owned());
    }
    let unit = match session.artifact_kind {
        design_model::ArtifactKind::Demo => "screen",
        design_model::ArtifactKind::Deck => "slide",
        design_model::ArtifactKind::Document => "page",
        design_model::ArtifactKind::Social => "frame",
        design_model::ArtifactKind::Print => "sheet",
        design_model::ArtifactKind::Mailing => "email",
        design_model::ArtifactKind::Campaign => "ad",
        design_model::ArtifactKind::Artwork => "cover",
    };
    if referenced_indexes(&request.content, unit).is_empty() {
        return Some(format!(
            "a regenerate message names the {unit}s to rewrite, like `[{unit} 2]`"
        ));
    }
    None
}

/// Appends a conversation turn. A `continue` action on a reviewing
/// session starts a generation run over the named design. A
/// `regenerate` action rewrites the units its content names.
pub(super) async fn post_message(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<MessageRequest>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let content = request.content.trim();
    if content.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "message content is empty",
            Vec::new(),
        );
    }
    if let Some(design) = &request.design
        && !is_valid_design_id(design)
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("design `{design}` is not a valid id"),
            Vec::new(),
        );
    }
    if let Some(problem) = pinned_problem(&id, &request.pinned) {
        return api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &problem, Vec::new());
    }
    let is_continue = request.action.as_deref() == Some("continue");
    let is_regenerate = request.action.as_deref() == Some("regenerate");
    if let Some(action) = &request.action
        && !is_continue
        && !is_regenerate
    {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("action `{action}` is unknown: use continue or regenerate, or omit it"),
            Vec::new(),
        );
    }
    if is_regenerate && let Some(problem) = regenerate_problem(&session, &request) {
        return api_error::error_response(StatusCode::UNPROCESSABLE_ENTITY, &problem, Vec::new());
    }
    if is_continue {
        if request.design.is_none() {
            return api_error::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "a continue message needs the design id to continue",
                Vec::new(),
            );
        }
        // Generating counts: a Finish pressed while a run works joins
        // that run instead of being refused. A halted session counts
        // too: the Finish is the resume.
        if !matches!(
            session.state,
            WorkflowState::Reviewing | WorkflowState::Generating
        ) && !session.state.is_halted()
        {
            return conflict("continue is only allowed while reviewing, generating, or halted");
        }
    }
    // A message after a halted run is the resume: the session returns to
    // the state the run halted in, then takes the turn.
    if session.state.is_halted() {
        let target = match sessions.recovery_target(&id).await {
            Ok(target) => target,
            Err(error) => return session_error_response(&error),
        };
        if let Err(error) = sessions
            .apply(&id, WorkflowEvent::Recovered { to: target })
            .await
        {
            return session_error_response(&error);
        }
    }
    let mut message = match (is_continue, is_regenerate, request.design.as_deref()) {
        (true, _, Some(design)) => ChatMessage::continue_request(content, design),
        (_, true, Some(design)) => ChatMessage::regenerate_request(content, design),
        _ => ChatMessage::user(content, request.design.as_deref()).with_pinned(request.pinned),
    };
    // Each ordinary prompt gets its own turn. Defer link captures until that
    // turn starts, so they cannot affect the run already in flight.
    message.is_queued = !is_continue;
    if let Err(error) = sessions.append_message(&id, message).await {
        return session_error_response(&error);
    }
    if is_continue && let Err(error) = sessions.apply(&id, WorkflowEvent::ContinueRequested).await {
        return session_error_response(&error);
    }
    notifier.notify();
    // Finish joins the active turn. Ordinary prompts wait for their own turn.
    if is_continue {
        if let Err(error) = runner.start_continue(&id).await {
            tracing::debug!(%error, session_id = %id, "continue waits for the active run");
        }
    } else {
        try_start(&runner, &id).await;
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Saves a question set the agent asks. Moves the session to
/// clarifying.
pub(super) async fn put_question_set(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(mut set): Json<BriefQuestionSet>,
) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    let problems = validate_question_set(&set);
    if !problems.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "question set failed validation",
            problems.iter().map(ToString::to_string).collect(),
        );
    }
    // Every set asks the Swift Deck way: the user can always skip.
    relax_question_set(&mut set);
    let number = match sessions.write_question_set(&id, &set).await {
        Ok(number) => number,
        Err(error) => return session_error_response(&error),
    };
    let message = ChatMessage::assistant_questions(&set.message, number);
    if let Err(error) = sessions.append_message(&id, message).await {
        return session_error_response(&error);
    }
    if let Err(error) = sessions.apply(&id, WorkflowEvent::QuestionsAsked).await {
        return session_error_response(&error);
    }
    notifier.notify();
    Json(serde_json::json!({ "number": number })).into_response()
}

/// Body of `POST /sessions/{id}/answers`.
#[derive(Debug, Deserialize)]
pub(super) struct AnswersRequest {
    question_set: u32,
    answers: Vec<QuestionAnswer>,
}

/// Records answers and resumes their turn before its queued follow-ups.
pub(super) async fn post_answers(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
    Json(request): Json<AnswersRequest>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.state != WorkflowState::Clarifying {
        return conflict("answers are only accepted while clarifying");
    }
    if session.latest_question_set != Some(request.question_set) {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "answers must be for the latest question set",
            Vec::new(),
        );
    }
    let set = match sessions.read_question_set(&id, request.question_set).await {
        Ok(Some(set)) => set,
        Ok(None) => return session_error_response(&SessionError::NotFound { id }),
        Err(error) => return session_error_response(&error),
    };
    let problems = validate_answers(&set, &request.answers);
    if !problems.is_empty() {
        return api_error::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "answers failed validation",
            problems.iter().map(ToString::to_string).collect(),
        );
    }
    let records = match sessions.answers(&id).await {
        Ok(records) => records,
        Err(error) => return session_error_response(&error),
    };
    // Retrying identical answers must not enqueue another copy of an active turn.
    if records
        .iter()
        .rev()
        .find(|record| record.question_set == request.question_set)
        .is_some_and(|record| record.answers == request.answers)
    {
        return StatusCode::NO_CONTENT.into_response();
    }
    if let Err(error) = sessions
        .record_answers(&id, request.question_set, request.answers)
        .await
    {
        return session_error_response(&error);
    }
    // Resume the prompt that asked these questions before its queued follow-ups.
    if let Err(error) = sessions.queue_answered_turn(&id).await {
        return session_error_response(&error);
    }
    notifier.notify();
    try_start(&runner, &id).await;
    StatusCode::NO_CONTENT.into_response()
}

/// Writes candidates now, without a planner turn: the user skipped the
/// questions. Moves the session to generating and starts a run.
pub(super) async fn generate_now(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !session.state.can_take_turn() {
        return conflict("cannot generate while generating or in error");
    }
    // A skip closes the open card. Without a record the set stays open,
    // and the workbench keeps asking over the candidates it wrote.
    let answers = match sessions.answers(&id).await {
        Ok(answers) => answers,
        Err(error) => return session_error_response(&error),
    };
    if let Some(number) = open_question_set(&session, &answers)
        && let Err(error) = sessions.record_answers(&id, number, Vec::new()).await
    {
        return session_error_response(&error);
    }
    if let Err(error) = sessions.apply(&id, WorkflowEvent::GenerationStarted).await {
        return session_error_response(&error);
    }
    notifier.notify();
    try_start(&runner, &id).await;
    StatusCode::OK.into_response()
}

/// Marks a generation run complete, for external runtimes and tests.
pub(super) async fn complete(
    State(sessions): State<SessionStore>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_session(&sessions, &id).await {
        return response;
    }
    match sessions
        .apply(&id, WorkflowEvent::GenerationSucceeded)
        .await
    {
        Ok(session) => {
            notifier.notify();
            Json(session).into_response()
        }
        Err(error) => session_error_response(&error),
    }
}

/// Recovers a failed session into the state it was in before the error.
pub(super) async fn retry(
    State(sessions): State<SessionStore>,
    State(runner): State<AgentRunner>,
    State(notifier): State<ChangeNotifier>,
    Path(id): Path<String>,
) -> Response {
    let session = match require_session(&sessions, &id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if !session.state.is_halted() {
        return conflict("resume is only allowed after a stop or an error");
    }
    let target = match sessions.recovery_target(&id).await {
        Ok(target) => target,
        Err(error) => return session_error_response(&error),
    };
    match sessions
        .apply(&id, WorkflowEvent::Recovered { to: target })
        .await
    {
        Ok(session) => {
            notifier.notify();
            try_start(&runner, &id).await;
            Json(session).into_response()
        }
        Err(error) => session_error_response(&error),
    }
}
