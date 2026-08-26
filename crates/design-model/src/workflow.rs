//! The session workflow: a persisted state machine.
//!
//! A session moves from `intake` through `clarifying` and the brief
//! states into `generating` and `reviewing`. Every change goes through
//! `transition`, which is the only place that knows which event is
//! allowed in which state. The server persists the result; nothing
//! infers state from chat text or from files on disk.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a session is in the brief-first workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    /// The request arrived. No agent turn has run yet.
    Intake,
    /// The agent asks material questions and waits for answers.
    Clarifying,
    /// The agent wrote the brief. The turn is still finishing.
    BriefReady,
    /// The user can approve the brief, edit it, or generate with
    /// assumptions.
    AwaitingApproval,
    /// The agent writes or edits designs from the approved brief.
    Generating,
    /// A design exists. The user can critique it or ask for changes.
    Reviewing,
    /// A run failed. The session keeps its data and can be retried.
    Error,
}

impl WorkflowState {
    /// Every state, in workflow order.
    pub const ALL: [WorkflowState; 7] = [
        WorkflowState::Intake,
        WorkflowState::Clarifying,
        WorkflowState::BriefReady,
        WorkflowState::AwaitingApproval,
        WorkflowState::Generating,
        WorkflowState::Reviewing,
        WorkflowState::Error,
    ];

    /// The snake_case name used in JSON and in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowState::Intake => "intake",
            WorkflowState::Clarifying => "clarifying",
            WorkflowState::BriefReady => "brief_ready",
            WorkflowState::AwaitingApproval => "awaiting_approval",
            WorkflowState::Generating => "generating",
            WorkflowState::Reviewing => "reviewing",
            WorkflowState::Error => "error",
        }
    }

    /// True while a briefing run may act: the agent may ask questions
    /// or draft the brief.
    pub fn is_briefing(self) -> bool {
        matches!(self, WorkflowState::Intake | WorkflowState::Clarifying)
    }

    /// True when the user may approve the brief or edit it before
    /// generation.
    pub fn is_awaiting_user(self) -> bool {
        matches!(
            self,
            WorkflowState::BriefReady | WorkflowState::AwaitingApproval
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
    /// The agent wrote a brief revision.
    BriefDrafted,
    /// The briefing turn finished and the brief is in front of the user.
    BriefPresented,
    /// The user edited the brief, which made a new revision.
    BriefEdited,
    /// The user approved the brief.
    Approved,
    /// The user chose to generate with the recorded assumptions.
    GenerateWithAssumptions,
    /// A generation run wrote its designs.
    GenerationSucceeded,
    /// The user sent a critique, which made a new revision.
    CritiqueSubmitted,
    /// The user asked to continue a preview design.
    ContinueRequested,
    /// A run failed or was stopped.
    RunFailed,
    /// The user retried after an error.
    Recovered {
        /// The state to return to.
        to: WorkflowState,
    },
}

impl WorkflowEvent {
    /// The snake_case name used in messages.
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowEvent::QuestionsAsked => "questions_asked",
            WorkflowEvent::BriefDrafted => "brief_drafted",
            WorkflowEvent::BriefPresented => "brief_presented",
            WorkflowEvent::BriefEdited => "brief_edited",
            WorkflowEvent::Approved => "approved",
            WorkflowEvent::GenerateWithAssumptions => "generate_with_assumptions",
            WorkflowEvent::GenerationSucceeded => "generation_succeeded",
            WorkflowEvent::CritiqueSubmitted => "critique_submitted",
            WorkflowEvent::ContinueRequested => "continue_requested",
            WorkflowEvent::RunFailed => "run_failed",
            WorkflowEvent::Recovered { .. } => "recovered",
        }
    }

    /// The states this event is allowed in.
    pub fn allowed_in(self) -> &'static [WorkflowState] {
        use WorkflowState::*;
        match self {
            WorkflowEvent::QuestionsAsked => &[Intake, Clarifying, Generating],
            WorkflowEvent::BriefDrafted => &[Intake, Clarifying],
            WorkflowEvent::BriefPresented => &[BriefReady],
            WorkflowEvent::BriefEdited => &[Clarifying, BriefReady, AwaitingApproval, Reviewing],
            WorkflowEvent::Approved => &[AwaitingApproval],
            WorkflowEvent::GenerateWithAssumptions => {
                &[Intake, Clarifying, BriefReady, AwaitingApproval]
            }
            WorkflowEvent::GenerationSucceeded => &[Generating],
            WorkflowEvent::CritiqueSubmitted | WorkflowEvent::ContinueRequested => &[Reviewing],
            WorkflowEvent::RunFailed => &[
                Intake,
                Clarifying,
                BriefReady,
                AwaitingApproval,
                Generating,
                Reviewing,
            ],
            WorkflowEvent::Recovered { .. } => &[Error],
        }
    }

    /// The state this event leads to.
    fn target(self) -> WorkflowState {
        match self {
            WorkflowEvent::QuestionsAsked => WorkflowState::Clarifying,
            WorkflowEvent::BriefDrafted => WorkflowState::BriefReady,
            WorkflowEvent::BriefPresented | WorkflowEvent::BriefEdited => {
                WorkflowState::AwaitingApproval
            }
            WorkflowEvent::Approved
            | WorkflowEvent::GenerateWithAssumptions
            | WorkflowEvent::CritiqueSubmitted
            | WorkflowEvent::ContinueRequested => WorkflowState::Generating,
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
        let table = [
            (Intake, QuestionsAsked, Clarifying),
            (Clarifying, QuestionsAsked, Clarifying),
            (Generating, QuestionsAsked, Clarifying),
            (Intake, BriefDrafted, BriefReady),
            (Clarifying, BriefDrafted, BriefReady),
            (BriefReady, BriefPresented, AwaitingApproval),
            (Clarifying, BriefEdited, AwaitingApproval),
            (BriefReady, BriefEdited, AwaitingApproval),
            (AwaitingApproval, BriefEdited, AwaitingApproval),
            (Reviewing, BriefEdited, AwaitingApproval),
            (AwaitingApproval, Approved, Generating),
            (Intake, GenerateWithAssumptions, Generating),
            (Clarifying, GenerateWithAssumptions, Generating),
            (BriefReady, GenerateWithAssumptions, Generating),
            (AwaitingApproval, GenerateWithAssumptions, Generating),
            (Generating, GenerationSucceeded, Reviewing),
            (Reviewing, CritiqueSubmitted, Generating),
            (Reviewing, ContinueRequested, Generating),
            (Generating, RunFailed, Error),
            (Clarifying, RunFailed, Error),
            (Error, Recovered { to: Clarifying }, Clarifying),
        ];
        for (from, event, to) in table {
            assert_eq!(transition(from, event), Ok(to), "{from} + {event}");
        }
    }

    #[test]
    fn illegal_transitions_name_state_and_event() {
        let error = transition(WorkflowState::Reviewing, WorkflowEvent::Approved).unwrap_err();
        assert_eq!(
            error.to_string(),
            "event `approved` is not allowed in state `reviewing`: it is allowed in awaiting_approval"
        );
        assert!(transition(WorkflowState::Error, WorkflowEvent::Approved).is_err());
        assert!(transition(WorkflowState::Intake, WorkflowEvent::GenerationSucceeded).is_err());
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
                    to: WorkflowState::Clarifying
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
        let event = serde_json::to_string(&WorkflowEvent::Recovered {
            to: WorkflowState::Intake,
        })
        .unwrap();
        assert_eq!(event, r#"{"event":"recovered","to":"intake"}"#);
    }

    #[test]
    fn briefing_and_awaiting_states_are_named() {
        assert!(WorkflowState::Intake.is_briefing());
        assert!(WorkflowState::Clarifying.is_briefing());
        assert!(!WorkflowState::Generating.is_briefing());
        assert!(WorkflowState::BriefReady.is_awaiting_user());
        assert!(WorkflowState::AwaitingApproval.is_awaiting_user());
        assert!(!WorkflowState::Reviewing.is_awaiting_user());
    }
}
