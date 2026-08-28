//! The session workflow: a persisted state machine.
//!
//! A session moves from `intake` through `clarifying` into
//! `generating` and `reviewing`. Every change goes through
//! `transition`, which is the only place that knows which event is
//! allowed in which state. The server persists the result; nothing
//! infers state from chat text or from files on disk.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a session is in the workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    /// The request arrived. No agent turn has run yet.
    Intake,
    /// The agent asked questions and waits for answers. Sessions saved
    /// before the brief was removed may say `brief_ready` or
    /// `awaiting_approval`; they read as this, and the next message
    /// continues them.
    #[serde(alias = "brief_ready", alias = "awaiting_approval")]
    Clarifying,
    /// The agent writes or edits artifacts.
    Generating,
    /// An artifact exists. The user can ask for changes in the chat.
    Reviewing,
    /// A run failed. The session keeps its data and can be retried.
    Error,
}

impl WorkflowState {
    /// Every state, in workflow order.
    pub const ALL: [WorkflowState; 5] = [
        WorkflowState::Intake,
        WorkflowState::Clarifying,
        WorkflowState::Generating,
        WorkflowState::Reviewing,
        WorkflowState::Error,
    ];

    /// The snake_case name used in JSON and in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowState::Intake => "intake",
            WorkflowState::Clarifying => "clarifying",
            WorkflowState::Generating => "generating",
            WorkflowState::Reviewing => "reviewing",
            WorkflowState::Error => "error",
        }
    }

    /// True when the user may send a message or answers and a run may
    /// start: every state except a run in progress and an error.
    pub fn can_take_turn(self) -> bool {
        matches!(
            self,
            WorkflowState::Intake | WorkflowState::Clarifying | WorkflowState::Reviewing
        )
    }
}

impl fmt::Display for WorkflowState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Something that happened to a session. `transition` maps a state and
/// an event to the next state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum WorkflowEvent {
    /// The agent asked a question set.
    QuestionsAsked,
    /// The agent started to write or edit artifacts.
    GenerationStarted,
    /// The run wrote or edited its artifacts.
    GenerationSucceeded,
    /// The user asked the agent to finish a preview artifact.
    ContinueRequested,
    /// A run failed.
    RunFailed,
    /// The user retried after an error.
    Recovered {
        /// The state to return to.
        to: WorkflowState,
    },
}

impl WorkflowEvent {
    /// The snake_case name used in JSON and in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowEvent::QuestionsAsked => "questions_asked",
            WorkflowEvent::GenerationStarted => "generation_started",
            WorkflowEvent::GenerationSucceeded => "generation_succeeded",
            WorkflowEvent::ContinueRequested => "continue_requested",
            WorkflowEvent::RunFailed => "run_failed",
            WorkflowEvent::Recovered { .. } => "recovered",
        }
    }

    /// The states this event is allowed in.
    pub fn allowed_in(self) -> &'static [WorkflowState] {
        use WorkflowState::*;
        match self {
            WorkflowEvent::QuestionsAsked => &[Intake, Clarifying, Generating, Reviewing],
            WorkflowEvent::GenerationStarted => &[Intake, Clarifying, Reviewing],
            WorkflowEvent::GenerationSucceeded => &[Generating],
            WorkflowEvent::ContinueRequested => &[Reviewing],
            WorkflowEvent::RunFailed => &[Intake, Clarifying, Generating, Reviewing],
            WorkflowEvent::Recovered { .. } => &[Error],
        }
    }

    /// The state this event leads to.
    fn target(self) -> WorkflowState {
        match self {
            WorkflowEvent::QuestionsAsked => WorkflowState::Clarifying,
            WorkflowEvent::GenerationStarted | WorkflowEvent::ContinueRequested => {
                WorkflowState::Generating
            }
            WorkflowEvent::GenerationSucceeded => WorkflowState::Reviewing,
            WorkflowEvent::RunFailed => WorkflowState::Error,
            WorkflowEvent::Recovered { to } => to,
        }
    }
}

impl fmt::Display for WorkflowEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a transition was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowError {
    /// The event is not allowed in the current state.
    #[error("event `{event}` is not allowed in state `{state}`: it is allowed in {allowed}")]
    IllegalTransition {
        /// The state the session is in.
        state: WorkflowState,
        /// The event that was refused.
        event: WorkflowEvent,
        /// The states the event is allowed in, comma separated.
        allowed: String,
    },
    /// A recovery named `error` as its target.
    #[error("cannot recover into `error`: name the state to return to")]
    InvalidRecovery,
}

/// The next state after `event` happens in `state`, or why it cannot.
pub fn transition(
    state: WorkflowState,
    event: WorkflowEvent,
) -> Result<WorkflowState, WorkflowError> {
    if let WorkflowEvent::Recovered { to } = event
        && to == WorkflowState::Error
    {
        return Err(WorkflowError::InvalidRecovery);
    }
    let allowed = event.allowed_in();
    if !allowed.contains(&state) {
        return Err(WorkflowError::IllegalTransition {
            state,
            event,
            allowed: allowed
                .iter()
                .map(|state| state.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        });
    }
    Ok(event.target())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{WorkflowError, WorkflowEvent, WorkflowState, transition};

    #[test]
    fn every_documented_transition_is_allowed() {
        use WorkflowEvent::*;
        use WorkflowState::*;
        let cases = [
            (Intake, QuestionsAsked, Clarifying),
            (Intake, GenerationStarted, Generating),
            (Clarifying, QuestionsAsked, Clarifying),
            (Clarifying, GenerationStarted, Generating),
            (Generating, QuestionsAsked, Clarifying),
            (Generating, GenerationSucceeded, Reviewing),
            (Reviewing, GenerationStarted, Generating),
            (Reviewing, QuestionsAsked, Clarifying),
            (Reviewing, ContinueRequested, Generating),
            (Generating, RunFailed, Error),
            (Error, Recovered { to: Reviewing }, Reviewing),
        ];
        for (from, event, to) in cases {
            assert_eq!(transition(from, event), Ok(to), "{from} + {event}");
        }
    }

    #[test]
    fn illegal_transitions_name_state_and_event() {
        let error =
            transition(WorkflowState::Intake, WorkflowEvent::GenerationSucceeded).unwrap_err();
        assert_eq!(
            error.to_string(),
            "event `generation_succeeded` is not allowed in state `intake`: it is allowed in generating"
        );
        assert!(transition(WorkflowState::Error, WorkflowEvent::GenerationStarted).is_err());
        assert!(transition(WorkflowState::Intake, WorkflowEvent::ContinueRequested).is_err());
        assert!(transition(WorkflowState::Error, WorkflowEvent::RunFailed).is_err());
    }

    #[test]
    fn error_recovers_to_any_state_except_error() {
        for state in WorkflowState::ALL {
            let result = transition(WorkflowState::Error, WorkflowEvent::Recovered { to: state });
            if state == WorkflowState::Error {
                assert_eq!(result, Err(WorkflowError::InvalidRecovery));
            } else {
                assert_eq!(result, Ok(state));
            }
        }
        assert!(
            transition(
                WorkflowState::Intake,
                WorkflowEvent::Recovered {
                    to: WorkflowState::Reviewing
                }
            )
            .is_err()
        );
    }

    #[test]
    fn workflow_state_round_trips_as_snake_case() {
        for state in WorkflowState::ALL {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, format!("\"{}\"", state.as_str()));
            assert_eq!(serde_json::from_str::<WorkflowState>(&json).unwrap(), state);
        }
        assert!(serde_json::from_str::<WorkflowState>("\"done\"").is_err());
        for old in ["\"brief_ready\"", "\"awaiting_approval\""] {
            assert_eq!(
                serde_json::from_str::<WorkflowState>(old).unwrap(),
                WorkflowState::Clarifying
            );
        }
        let event = serde_json::to_string(&WorkflowEvent::Recovered {
            to: WorkflowState::Intake,
        })
        .unwrap();
        assert_eq!(event, r#"{"event":"recovered","to":"intake"}"#);
    }

    #[test]
    fn the_turn_states_are_named() {
        assert!(WorkflowState::Intake.can_take_turn());
        assert!(WorkflowState::Clarifying.can_take_turn());
        assert!(WorkflowState::Reviewing.can_take_turn());
        assert!(!WorkflowState::Generating.can_take_turn());
        assert!(!WorkflowState::Error.can_take_turn());
    }
}
