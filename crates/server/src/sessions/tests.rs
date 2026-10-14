#![allow(clippy::unwrap_used)]

use super::*;

#[test]
fn a_user_turn_keeps_the_candidates_it_pinned() {
    let message = ChatMessage::user("Bigger title.", None).with_pinned(vec![
        "talk-candidate-1".to_owned(),
        "talk-candidate-3".to_owned(),
    ]);
    let json = serde_json::to_value(&message).unwrap();
    assert_eq!(json["pinned"][1], "talk-candidate-3");
    let back: ChatMessage = serde_json::from_value(json).unwrap();
    assert_eq!(back, message);
    let plain = serde_json::to_value(ChatMessage::user("Hi.", None)).unwrap();
    assert!(plain.get("pinned").is_none());
}

#[test]
fn a_regenerate_turn_names_its_artifact_and_round_trips() {
    let message = ChatMessage::regenerate_request("[screen 2] Write it anew.", "talk-candidate-1");
    assert!(message.is_regenerate);
    assert!(!message.is_continue);
    assert_eq!(message.design.as_deref(), Some("talk-candidate-1"));
    let json = serde_json::to_value(&message).unwrap();
    assert_eq!(json["is_regenerate"], true);
    let back: ChatMessage = serde_json::from_value(json).unwrap();
    assert_eq!(back, message);
    let plain = serde_json::to_value(ChatMessage::user("Hi.", None)).unwrap();
    assert!(plain.get("is_regenerate").is_none());
}

#[test]
fn known_app_products_require_one_window_contract_except_for_landing_pages() {
    for kind in [
        "consumer_app",
        "business_app",
        "developer_tool",
        "dashboard",
    ] {
        let mut options = RunOptions {
            product_kind: Some(kind.to_owned()),
            ..RunOptions::default()
        };
        assert!(options.should_have_app_window());
        options.scope = Some("landing_page".to_owned());
        assert!(!options.should_have_app_window());
    }
}

#[test]
fn a_suggestion_fills_a_blank_axis_and_marks_it() {
    let mut options = RunOptions::default();
    let filled = options.suggest(
        ArtifactKind::Demo,
        &[
            ("product_kind".to_owned(), "developer_tool".to_owned()),
            ("color_mode".to_owned(), "dark".to_owned()),
        ],
    );
    assert_eq!(filled, ["product_kind", "color_mode"]);
    assert_eq!(options.product_kind.as_deref(), Some("developer_tool"));
    assert_eq!(options.color_mode.as_deref(), Some("dark"));
    assert_eq!(options.suggested, ["product_kind", "color_mode"]);
}

#[test]
fn a_suggestion_never_replaces_an_answer_or_names_an_unknown_value() {
    let mut options = RunOptions {
        product_kind: Some("dashboard".to_owned()),
        ..RunOptions::default()
    };
    let filled = options.suggest(
        ArtifactKind::Demo,
        &[
            ("product_kind".to_owned(), "developer_tool".to_owned()),
            ("scope".to_owned(), "everything".to_owned()),
            ("vibe".to_owned(), "loud".to_owned()),
            // A deck axis is not a demo axis.
            ("audience".to_owned(), "newcomers".to_owned()),
        ],
    );
    assert!(filled.is_empty());
    assert_eq!(options.product_kind.as_deref(), Some("dashboard"));
    assert_eq!(options.scope, None);
    assert_eq!(options.audience, None);
    assert!(options.suggested.is_empty());
}

#[test]
fn the_axes_of_a_demo_leave_out_the_audience_and_the_tone() {
    let options = RunOptions {
        audience: Some("newcomers".to_owned()),
        tone: Some("warm".to_owned()),
        color_mode: Some("dark".to_owned()),
        product_kind: Some("developer_tool".to_owned()),
        ..RunOptions::default()
    };
    assert_eq!(
        options.axes(ArtifactKind::Demo),
        [("Color mode", "dark"), ("Product kind", "developer_tool")]
    );
    assert_eq!(
        options.axes(ArtifactKind::Deck),
        [
            ("Color mode", "dark"),
            ("Audience", "newcomers"),
            ("Tone", "warm")
        ]
    );
}
use design_model::{QuestionKind, QuestionOption};

fn store() -> (tempfile::TempDir, SessionStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = SessionStore::new(directory.path().join("sessions"));
    (directory, store)
}

fn question(id: &str) -> BriefQuestion {
    BriefQuestion {
        id: id.to_owned(),
        label: format!("Which {id}?"),
        rationale: None,
        kind: QuestionKind::SingleSelect,
        required: true,
        options: vec![
            QuestionOption {
                value: "a".to_owned(),
                label: "A".to_owned(),
            },
            QuestionOption {
                value: "b".to_owned(),
                label: "B".to_owned(),
            },
        ],
        allow_other: false,
    }
}

fn set(id: &str) -> BriefQuestionSet {
    BriefQuestionSet {
        title: "Q".to_owned(),
        message: "m".to_owned(),
        questions: vec![question(id)],
        can_proceed_with_assumptions: true,
    }
}

#[tokio::test]
async fn creating_a_session_starts_in_intake_and_survives_a_new_store() {
    let (directory, store) = store();
    let session = store
        .create(NewSession::demo(
            "finance-app",
            "Finance app",
            "Design a landing page.",
        ))
        .await
        .unwrap();
    assert_eq!(session.state, WorkflowState::Intake);
    assert_eq!(session.artifact_kind, ArtifactKind::Demo);
    assert!(
        store
            .create(NewSession::demo("finance-app", "x", "y"))
            .await
            .is_err()
    );
    let reopened = SessionStore::new(directory.path().join("sessions"));
    let loaded = reopened.read("finance-app").await.unwrap().unwrap();
    assert_eq!(loaded.request, "Design a landing page.");
    assert_eq!(loaded.state, WorkflowState::Intake);
    assert_eq!(store.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_session_rewrite_replaces_the_record_and_leaves_no_temporary() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    store
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    let names: Vec<String> = std::fs::read_dir(store.session_directory("talk"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|name| !name.contains(".writing")),
        "leftover temporary in {names:?}"
    );
    let loaded = store.read("talk").await.unwrap().unwrap();
    assert_eq!(loaded.state, WorkflowState::Generating);
}

#[tokio::test]
async fn question_sets_number_in_order_and_answers_attach_to_the_latest() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    assert_eq!(
        store
            .write_question_set("talk", &set("platform"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .write_question_set("talk", &set("audience"))
            .await
            .unwrap(),
        2
    );
    store
        .record_answers(
            "talk",
            2,
            vec![QuestionAnswer {
                question_id: "audience".to_owned(),
                values: vec!["a".to_owned()],
                ..QuestionAnswer::default()
            }],
        )
        .await
        .unwrap();
    let answered = store.answered_questions("talk").await.unwrap();
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0].0.id, "audience");
    assert_eq!(answered[0].1.values, vec!["a"]);
    assert_eq!(store.question_sets("talk").await.unwrap().len(), 2);
}

#[tokio::test]
async fn apply_rejects_illegal_events_and_keeps_the_file() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    assert!(
        store
            .apply("talk", WorkflowEvent::GenerationSucceeded)
            .await
            .is_err()
    );
    let session = store.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, WorkflowState::Intake);
    let after = store
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    assert_eq!(after.state, WorkflowState::Generating);
}

#[tokio::test]
async fn run_failed_remembers_the_previous_state_for_retry() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    store
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    store
        .update("talk", |session| {
            session.error = Some("boom".to_owned());
        })
        .await
        .unwrap();
    let failed = store.apply("talk", WorkflowEvent::RunFailed).await.unwrap();
    assert_eq!(failed.state, WorkflowState::Error);
    assert_eq!(failed.resume_state, Some(WorkflowState::Generating));
    // The run halted in generating; the user resumes on the canvas.
    assert_eq!(
        store.recovery_target("talk").await.unwrap(),
        WorkflowState::Reviewing
    );
    let recovered = store
        .apply(
            "talk",
            WorkflowEvent::Recovered {
                to: WorkflowState::Generating,
            },
        )
        .await
        .unwrap();
    assert_eq!(recovered.state, WorkflowState::Generating);
    assert_eq!(recovered.error, None);
    assert_eq!(recovered.resume_state, None);
}

#[tokio::test]
async fn a_stop_halts_without_recording_a_failure() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    store
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    let stopped = store
        .apply("talk", WorkflowEvent::RunStopped)
        .await
        .unwrap();
    assert_eq!(stopped.state, WorkflowState::Stopped);
    // A stop has nothing to report, so no failure message is kept.
    assert_eq!(stopped.error, None);
    assert_eq!(stopped.resume_state, Some(WorkflowState::Generating));
    assert_eq!(
        store.recovery_target("talk").await.unwrap(),
        WorkflowState::Reviewing
    );
    let resumed = store
        .apply(
            "talk",
            WorkflowEvent::Recovered {
                to: WorkflowState::Generating,
            },
        )
        .await
        .unwrap();
    assert_eq!(resumed.state, WorkflowState::Generating);
    assert_eq!(resumed.resume_state, None);
}

#[tokio::test]
async fn a_record_written_before_the_stop_state_still_resumes() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    store
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    store.apply("talk", WorkflowEvent::RunFailed).await.unwrap();
    // Records written before the rename name the field
    // `state_before_error`; the alias keeps them resumable.
    let raw = std::fs::read_to_string(store.session_path("talk")).unwrap();
    let old = raw.replace("\"resume_state\"", "\"state_before_error\"");
    assert!(old.contains("state_before_error"));
    let loaded: Session = serde_json::from_str(&old).unwrap();
    assert_eq!(loaded.resume_state, Some(WorkflowState::Generating));
}

#[test]
fn a_halted_session_starts_no_run_until_it_resumes() {
    assert_eq!(run_mode_for(WorkflowState::Stopped), None);
    assert_eq!(run_mode_for(WorkflowState::Error), None);
    assert_eq!(
        run_mode_for(WorkflowState::Generating),
        Some(RunMode::Generation)
    );
}

#[tokio::test]
async fn runs_record_revision_runtime_and_result() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    let run_id = store
        .start_run(
            "talk",
            RunRecord {
                run_id: String::new(),
                mode: RunMode::Generation,
                runtime: "built-in".to_owned(),
                provider: Some("openai".to_owned()),
                model: Some("gpt-5".to_owned()),
                started_at: rfc3339_now(),
                finished_at: None,
                result: None,
                error: None,
                artifacts: Vec::new(),
            },
        )
        .await
        .unwrap();
    store
        .finish_run(
            "talk",
            &run_id,
            "succeeded",
            None,
            vec!["talk-candidate-1".to_owned()],
        )
        .await
        .unwrap();
    let runs = store.runs("talk").await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].mode, RunMode::Generation);
    assert_eq!(runs[0].result.as_deref(), Some("succeeded"));
    assert_eq!(runs[0].artifacts, vec!["talk-candidate-1"]);
}

#[test]
fn old_run_modes_read_as_generation() {
    let mode: RunMode = serde_json::from_str("\"briefing\"").unwrap();
    assert_eq!(mode, RunMode::Generation);
}

#[tokio::test]
async fn rename_moves_the_directory() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("old-name", "Old", "A talk."))
        .await
        .unwrap();
    assert!(store.rename("old-name", "new-name").await.unwrap());
    assert!(store.read("old-name").await.unwrap().is_none());
    let moved = store.read("new-name").await.unwrap().unwrap();
    assert_eq!(moved.id, "new-name");
    assert!(!store.rename("missing", "x").await.unwrap());
}

#[tokio::test]
async fn a_session_carries_its_artifact_kind() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk.").with_kind(ArtifactKind::Deck))
        .await
        .unwrap();
    let session = store.read("talk").await.unwrap().unwrap();
    assert_eq!(session.artifact_kind, ArtifactKind::Deck);
    assert_eq!(
        store.list().await.unwrap()[0].artifact_kind,
        ArtifactKind::Deck
    );
    // A record written before the field existed loads as a demo.
    let raw = std::fs::read_to_string(store.session_path("talk")).unwrap();
    let without_kind = raw.replace("\"artifact_kind\": \"deck\",", "");
    let old: Session = serde_json::from_str(&without_kind).unwrap();
    assert_eq!(old.artifact_kind, ArtifactKind::Demo);
}

#[test]
fn session_id_of_artifact_strips_the_candidate_suffix() {
    assert_eq!(session_id_of_artifact("talk-candidate-2"), "talk");
    assert_eq!(session_id_of_artifact("talk"), "talk");
    assert_eq!(
        session_id_of_artifact("finance-app-candidate-10"),
        "finance-app"
    );
}

#[test]
fn session_ids_reject_candidate_suffixes() {
    assert!(is_valid_session_id("finance-app"));
    assert!(!is_valid_session_id("talk-candidate-1"));
    assert!(!is_valid_session_id("render"));
    assert!(!is_valid_session_id("Bad Id"));
}

#[tokio::test]
async fn an_appended_message_is_stamped_with_its_time() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A talk."))
        .await
        .unwrap();
    store
        .append_message("talk", ChatMessage::user("Hello.", None))
        .await
        .unwrap();
    let message = store.messages("talk").await.unwrap().pop().unwrap();
    let at = message.at.expect("a time");
    assert!(at.ends_with('Z') && at.len() == 20, "{at}");
}

#[tokio::test]
async fn a_run_the_last_process_left_behind_is_stopped_at_boot() {
    let (_directory, store) = store();
    store
        .create(NewSession::demo("talk", "Talk", "A landing page."))
        .await
        .unwrap();
    store
        .apply("talk", WorkflowEvent::GenerationStarted)
        .await
        .unwrap();
    let record = RunRecord {
        run_id: String::new(),
        mode: RunMode::Generation,
        runtime: "built-in".to_owned(),
        provider: None,
        model: None,
        started_at: rfc3339_now(),
        finished_at: None,
        result: None,
        error: None,
        artifacts: Vec::new(),
    };
    let run_id = store.start_run("talk", record).await.unwrap();
    assert_eq!(store.stop_orphaned_runs().await.unwrap(), vec!["talk"]);
    let session = store.read("talk").await.unwrap().unwrap();
    assert_eq!(session.state, WorkflowState::Stopped);
    let runs = store.runs("talk").await.unwrap();
    let record = runs.iter().find(|record| record.run_id == run_id).unwrap();
    assert_eq!(record.result.as_deref(), Some("stopped"));
    assert!(record.finished_at.is_some());
    // A second boot finds nothing to do.
    assert!(store.stop_orphaned_runs().await.unwrap().is_empty());
}

#[test]
fn a_new_session_id_is_a_random_version_4_uuid() {
    let id = new_session_id().unwrap();
    assert_eq!(id.len(), 36);
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        [8, 4, 4, 4, 12]
    );
    assert!(groups[2].starts_with('4'), "version nibble: {id}");
    assert!(
        matches!(&groups[3][..1], "8" | "9" | "a" | "b"),
        "variant nibble: {id}"
    );
    // The id is a file stem and a URL segment everywhere it goes.
    assert!(is_valid_session_id(&id));
    assert_ne!(id, new_session_id().unwrap());
}
