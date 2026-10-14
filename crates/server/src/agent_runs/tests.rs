#![allow(clippy::unwrap_used)]

use std::time::Duration;

use tempfile::TempDir;

use design_model::WorkflowState;

use crate::agent_runs::{AgentRunner, StartError};
use crate::designs::DesignStore;
use crate::events::ChangeNotifier;
use crate::sessions::{ChatMessage, NewSession, SessionStore};
use crate::settings::SettingsStore;

fn command_runner(directory: &TempDir, command: &str) -> (AgentRunner, SessionStore) {
    let sessions = SessionStore::new(directory.path().join("sessions"));
    let runner = AgentRunner::new(
        Some(command.to_owned()),
        SettingsStore::new(
            directory.path().join("settings.json"),
            "127.0.0.1:3000".to_owned(),
        ),
        DesignStore::new(directory.path().join("designs")),
        sessions.clone(),
        "http://127.0.0.1:3000".to_owned(),
        ChangeNotifier::new(),
    );
    (runner, sessions)
}

async fn wait_until_finished(runner: &AgentRunner) -> serde_json::Value {
    for _ in 0..100 {
        let status = runner.status();
        if status["is_running"] == false {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    runner.status()
}

fn queued_prompt(content: &str) -> ChatMessage {
    let mut message = ChatMessage::user(content, None);
    message.is_queued = true;
    message
}

async fn generating_session(sessions: &SessionStore) {
    sessions
        .create(NewSession::demo("talk", "Talk", "A workspace."))
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
}

async fn wait_for_turns(runner: &AgentRunner, sessions: &SessionStore, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let runs = sessions.runs("talk").await.unwrap();
            if runs.len() == count
                && runs.iter().all(|run| run.finished_at.is_some())
                && !runner.is_running_session("talk")
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn queued_prompts_start_in_fifo_order_only_after_the_active_turn_finishes() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 0.2");
    generating_session(&sessions).await;
    runner.start("talk").await.unwrap();
    sessions
        .append_message("talk", queued_prompt("First queued prompt"))
        .await
        .unwrap();
    sessions
        .append_message("talk", queued_prompt("Second queued prompt"))
        .await
        .unwrap();
    assert!(matches!(
        runner.start("talk").await,
        Err(StartError::AlreadyRunning)
    ));
    assert!(sessions.messages_for_run("talk").await.unwrap().is_empty());
    assert_eq!(sessions.runs("talk").await.unwrap().len(), 1);
    wait_for_turns(&runner, &sessions, 3).await;
    let messages = sessions.messages("talk").await.unwrap();
    assert!(messages.iter().all(|message| !message.is_queued));
    assert_eq!(messages[0].content, "First queued prompt");
    assert_eq!(messages[1].content, "Second queued prompt");
}

#[tokio::test]
async fn answered_turns_resume_before_their_queued_followups() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 0.2");
    generating_session(&sessions).await;
    sessions
        .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
    sessions
        .append_message("talk", ChatMessage::user("Current prompt", None))
        .await
        .unwrap();
    sessions
        .append_message("talk", queued_prompt("Later prompt"))
        .await
        .unwrap();
    sessions.queue_answered_turn("talk").await.unwrap();
    runner.start_pending().await;
    assert_eq!(
        sessions
            .messages_for_run("talk")
            .await
            .unwrap()
            .last()
            .unwrap()
            .content,
        "Current prompt"
    );
    assert!(sessions.has_queued_messages("talk").await.unwrap());
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    wait_for_turns(&runner, &sessions, 2).await;
    assert_eq!(
        sessions
            .messages_for_run("talk")
            .await
            .unwrap()
            .last()
            .unwrap()
            .content,
        "Later prompt"
    );
}

#[tokio::test]
async fn finish_requests_do_not_consume_an_ordinary_queued_prompt() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 5");
    generating_session(&sessions).await;
    sessions
        .append_message("talk", queued_prompt("Waiting edit"))
        .await
        .unwrap();
    sessions
        .append_message(
            "talk",
            ChatMessage::continue_request("Finish", "talk-candidate-1"),
        )
        .await
        .unwrap();
    runner.start_continue("talk").await.unwrap();
    assert!(sessions.has_queued_messages("talk").await.unwrap());
    assert!(
        sessions
            .messages_for_run("talk")
            .await
            .unwrap()
            .last()
            .unwrap()
            .is_continue
    );
    runner.stop();
    wait_for_turns(&runner, &sessions, 1).await;
}

#[tokio::test]
async fn rapid_queued_turns_finish_the_correct_run_records() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "true");
    generating_session(&sessions).await;
    for index in 0..12 {
        sessions
            .append_message("talk", queued_prompt(&format!("Prompt {index}")))
            .await
            .unwrap();
    }
    runner.start("talk").await.unwrap();
    wait_for_turns(&runner, &sessions, 12).await;
    assert!(!sessions.has_queued_messages("talk").await.unwrap());
    assert!(
        sessions
            .runs("talk")
            .await
            .unwrap()
            .iter()
            .all(|run| run.result.as_deref() == Some("succeeded"))
    );
}

#[tokio::test]
async fn stopping_between_turns_pauses_the_queue_until_an_explicit_start() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "true");
    generating_session(&sessions).await;
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationSucceeded)
        .await
        .unwrap();
    sessions
        .append_message("talk", queued_prompt("Waiting prompt"))
        .await
        .unwrap();
    runner.stop();
    runner.start_pending().await;
    assert!(sessions.runs("talk").await.unwrap().is_empty());
    assert!(runner.status()["is_queue_paused"].as_bool().unwrap());
    runner.start("talk").await.unwrap();
    wait_for_turns(&runner, &sessions, 1).await;
    assert!(!runner.status()["is_queue_paused"].as_bool().unwrap());
}

#[tokio::test]
async fn stopping_a_turn_keeps_the_pending_prompts_queued() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 5");
    generating_session(&sessions).await;
    runner.start("talk").await.unwrap();
    sessions
        .append_message("talk", queued_prompt("Keep this request"))
        .await
        .unwrap();
    runner.stop();
    wait_for_turns(&runner, &sessions, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(sessions.has_queued_messages("talk").await.unwrap());
    assert_eq!(sessions.runs("talk").await.unwrap().len(), 1);
    assert_eq!(
        sessions.read("talk").await.unwrap().unwrap().state,
        WorkflowState::Stopped
    );
}

#[tokio::test]
async fn a_failed_turn_does_not_start_its_pending_prompts() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 0.1; exit 1");
    generating_session(&sessions).await;
    runner.start("talk").await.unwrap();
    sessions
        .append_message("talk", queued_prompt("Keep this request"))
        .await
        .unwrap();
    wait_for_turns(&runner, &sessions, 1).await;
    assert!(sessions.has_queued_messages("talk").await.unwrap());
    assert_eq!(
        sessions.read("talk").await.unwrap().unwrap().state,
        WorkflowState::Error
    );
}

#[tokio::test]
async fn questions_pause_the_queue_until_the_user_responds() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 0.1");
    generating_session(&sessions).await;
    runner.start("talk").await.unwrap();
    sessions
        .append_message("talk", queued_prompt("Keep this request"))
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::QuestionsAsked)
        .await
        .unwrap();
    wait_for_turns(&runner, &sessions, 1).await;
    assert!(sessions.has_queued_messages("talk").await.unwrap());
    runner.start("talk").await.unwrap();
    wait_for_turns(&runner, &sessions, 2).await;
    assert!(!sessions.has_queued_messages("talk").await.unwrap());
}

#[tokio::test]
async fn a_custom_command_run_captures_output_and_exit_code() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "echo custom-agent");
    sessions
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    runner.start("talk").await.unwrap();
    let status = wait_until_finished(&runner).await;
    assert_eq!(status["exit_code"], 0);
    assert_eq!(status["active_agent"], "custom");
    assert_eq!(status["mode"], "generation");
    assert!(
        status["log_tail"]
            .as_str()
            .unwrap()
            .contains("custom-agent")
    );
}

#[tokio::test]
async fn a_second_start_while_running_is_rejected() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "sleep 5");
    sessions
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    runner.start("talk").await.unwrap();
    assert!(runner.start("talk").await.is_err());
    runner.stop();
    let status = wait_until_finished(&runner).await;
    assert_eq!(status["is_running"], false);
}

#[tokio::test]
async fn a_run_is_refused_when_the_session_is_in_error() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "echo hi");
    sessions
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::RunFailed)
        .await
        .unwrap();
    assert!(runner.start("talk").await.is_err());
}

#[tokio::test]
async fn a_missing_session_is_reported_as_not_found() {
    let directory = TempDir::new().unwrap();
    let (runner, _sessions) = command_runner(&directory, "echo hi");
    let error = runner.start("missing").await.unwrap_err();
    assert_eq!(error.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stopping_a_command_leaves_the_session_stopped_not_failed() {
    let directory = TempDir::new().unwrap();
    // A command long enough to still be running when stop arrives.
    let (runner, sessions) = command_runner(&directory, "sleep 30");
    sessions
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    runner.start("talk").await.unwrap();
    runner.stop();
    wait_until_finished(&runner).await;
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, design_model::WorkflowState::Stopped);
    // A stop is not a failure, so nothing is reported and the run
    // resumes where it left off.
    assert_eq!(session.error, None);
    assert_eq!(
        session.resume_state,
        Some(design_model::WorkflowState::Generating)
    );
}

#[tokio::test]
async fn a_failing_command_moves_the_session_to_error_and_names_the_runtime() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "exit 3");
    sessions
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    runner.start("talk").await.unwrap();
    wait_until_finished(&runner).await;
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, design_model::WorkflowState::Error);
    assert!(
        session
            .error
            .unwrap()
            .contains("SWIFT_DESIGN_AGENT_COMMAND")
    );
}

#[tokio::test]
async fn a_zero_exit_in_generating_moves_the_session_to_reviewing() {
    let directory = TempDir::new().unwrap();
    let (runner, sessions) = command_runner(&directory, "echo done");
    sessions
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    sessions
        .apply("talk", design_model::WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    runner.start("talk").await.unwrap();
    wait_until_finished(&runner).await;
    let session = sessions.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, design_model::WorkflowState::Reviewing);
}
