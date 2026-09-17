//! The one event stream. Everything an engine does becomes a [`CoreEvent`]:
//! `eavery-acp` produces them, the store persists them, the CLI prints them,
//! and the UI renders them in one of two vocabularies.
//!
//! See `docs/plan/03-architecture.md` §4.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::EngineError;
use crate::model::{Checkpoint, CheckpointId, EngineStatus, Plan, RiskClass, TurnId, TurnPhase};

/// How many lines of the engine's stderr travel with a crash
/// (`docs/plan/10-task-breakdown.md` M1-T08). The connection keeps more than
/// this so a burst of warnings cannot push the line that matters out of the
/// ring; the event carries the tail of it.
pub const STDERR_TAIL_LINES: usize = 50;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreEvent {
    TurnStarted {
        turn_id: TurnId,
        phase: TurnPhase,
    },
    PhaseChanged {
        turn_id: TurnId,
        phase: TurnPhase,
    },
    /// One streamed chunk of the engine's reply, not a whole message.
    AgentText {
        turn_id: TurnId,
        text: String,
    },
    /// Reasoning. Hidden in Everyday mode.
    AgentThought {
        turn_id: TurnId,
        text: String,
    },
    ToolCallStarted {
        turn_id: TurnId,
        call: ToolCallView,
    },
    ToolCallUpdated {
        turn_id: TurnId,
        call: ToolCallView,
    },
    /// The engine's own checklist, from ACP `plan` session updates.
    PlanUpdated {
        turn_id: TurnId,
        entries: Vec<PlanEntryView>,
    },
    PermissionRequested {
        turn_id: TurnId,
        request: PermissionView,
    },
    PermissionResolved {
        turn_id: TurnId,
        request_id: String,
        decision: Decision,
        by: DecidedBy,
    },
    /// The plan phase finished and produced something to approve.
    PlanReady {
        turn_id: TurnId,
        plan: Plan,
    },
    CheckpointCreated {
        checkpoint: Checkpoint,
    },
    Restored {
        to: CheckpointId,
        new_checkpoint: CheckpointId,
        /// Files that could not be written because something else held them
        /// open. Reported, never silently skipped.
        skipped_locked: Vec<String>,
    },
    TurnFinished {
        turn_id: TurnId,
        stop_reason: String,
        digest: Option<Digest>,
    },
    EngineStatus {
        engine_id: String,
        status: EngineStatus,
    },
    /// The engine process died. `stderr_tail` is the last lines of its stderr.
    EngineCrashed {
        engine_id: String,
        turn_id: Option<TurnId>,
        stderr_tail: Vec<String>,
    },
    Error {
        turn_id: Option<TurnId>,
        code: ErrorCode,
        message: String,
        /// What the user can do about it, in their own vocabulary. Everyday
        /// mode renders errors as next actions, not as failures.
        next_action: Option<String>,
    },
}

impl CoreEvent {
    /// The Turn this event belongs to, where it belongs to one.
    ///
    /// The store keys events on it, so it is read from the event rather than
    /// passed alongside it: a caller that got the two out of step would write
    /// a transcript that cannot be reassembled.
    pub fn turn_id(&self) -> Option<TurnId> {
        match self {
            CoreEvent::TurnStarted { turn_id, .. }
            | CoreEvent::PhaseChanged { turn_id, .. }
            | CoreEvent::AgentText { turn_id, .. }
            | CoreEvent::AgentThought { turn_id, .. }
            | CoreEvent::ToolCallStarted { turn_id, .. }
            | CoreEvent::ToolCallUpdated { turn_id, .. }
            | CoreEvent::PlanUpdated { turn_id, .. }
            | CoreEvent::PermissionRequested { turn_id, .. }
            | CoreEvent::PermissionResolved { turn_id, .. }
            | CoreEvent::PlanReady { turn_id, .. }
            | CoreEvent::TurnFinished { turn_id, .. } => Some(*turn_id),
            CoreEvent::EngineCrashed { turn_id, .. } | CoreEvent::Error { turn_id, .. } => *turn_id,
            CoreEvent::CheckpointCreated { checkpoint } => checkpoint.turn_id,
            // A restore happens between turns, and an engine's status is not
            // about any one of them.
            CoreEvent::Restored { .. } | CoreEvent::EngineStatus { .. } => None,
        }
    }

    /// The event for a failure talking to an engine.
    ///
    /// A dead process is its own event, not an error message: the transcript
    /// shows what the engine said before it went, and Diagnostics shows the
    /// stderr that explains it. Everything else is an [`CoreEvent::Error`]
    /// carrying the next action the user can take.
    pub fn from_engine_error(error: &EngineError, turn_id: Option<TurnId>) -> CoreEvent {
        if let EngineError::Crashed {
            engine_id,
            stderr_tail,
            ..
        } = error
        {
            return CoreEvent::EngineCrashed {
                engine_id: engine_id.clone(),
                turn_id,
                stderr_tail: tail(stderr_tail),
            };
        }
        CoreEvent::Error {
            turn_id,
            code: error.code(),
            message: error.to_string(),
            next_action: error.next_action(),
        }
    }
}

/// The last [`STDERR_TAIL_LINES`] lines. The end is the interesting part: a
/// panic message is the last thing a process writes.
fn tail(lines: &[String]) -> Vec<String> {
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    lines[start..].to_vec()
}

/// A tool call as the UI sees it: already classified, already summarised.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct ToolCallView {
    pub id: String,
    pub title: String,
    /// The ACP kind string, `"other"` when the engine omits it.
    pub kind: String,
    /// `pending` | `in_progress` | `completed` | `failed`.
    pub status: String,
    pub locations: Vec<String>,
    pub risk: RiskClass,
    /// "3 lines changed in report.md".
    pub diff_summary: Option<String>,
}

/// One entry of the engine's own plan checklist (ACP `plan` update).
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct PlanEntryView {
    pub content: String,
    pub priority: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct PermissionView {
    /// The JSON-RPC id of the `session/request_permission` request, as a string.
    pub request_id: String,
    pub tool_call_id: String,
    pub title: String,
    pub risk: RiskClass,
    /// Passed through from ACP so the answer can name an option the engine
    /// actually offered.
    pub options: Vec<PermissionOption>,
    /// Vocabulary-neutral facts — paths, hosts — for the UI to phrase.
    pub explanation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    /// `allow_once` | `allow_always` | `reject_once` | `reject_always`.
    pub kind: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Cancelled,
}

impl Decision {
    /// The ACP option `kind` this decision wants.
    pub fn wanted_kind(self) -> &'static str {
        match self {
            Decision::AllowOnce => "allow_once",
            Decision::AllowAlways => "allow_always",
            Decision::RejectOnce => "reject_once",
            Decision::RejectAlways => "reject_always",
            Decision::Cancelled => "",
        }
    }

    /// What to fall back to when the engine did not offer [`Self::wanted_kind`]
    /// (`docs/plan/04-acp-engines.md` §1). Never widens a decision: an
    /// unavailable `allow_always` becomes `allow_once`, and anything else
    /// becomes a cancellation rather than an accidental allow.
    pub fn fallback(self) -> Option<Decision> {
        match self {
            Decision::AllowAlways => Some(Decision::AllowOnce),
            Decision::RejectAlways => Some(Decision::RejectOnce),
            _ => None,
        }
    }
}

/// Who made a permission decision. Everything is on the record, including the
/// decisions the user never saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    Policy,
    User,
    PlanGate,
}

/// What a turn actually did, in file terms.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
pub struct Digest {
    pub files_added: Vec<String>,
    pub files_changed: Vec<String>,
    pub files_removed: Vec<String>,
    /// What left the machine, from the outbound calls that were allowed. Always
    /// rendered, "Nothing" when empty.
    pub outbound_actions: Vec<String>,
    pub refused_actions: Vec<String>,
    /// The pre-turn checkpoint: the one Undo goes back to.
    pub undo_to: Option<CheckpointId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The engine executable could not be found or would not start.
    EngineUnavailable,
    /// The engine stopped answering.
    EngineTimeout,
    /// The engine process exited during a turn.
    EngineCrashed,
    /// A checkpoint could not be taken, so the turn did not run (working rule 7).
    CheckpointFailed,
    RestoreFailed,
    /// A file could not be written because something else held it open.
    FileLocked,
    /// The engine mutated something during the plan phase without asking
    /// (`docs/plan/06-plan-gate-permissions.md` §2.2).
    PlanGateBypassed,
    /// A second turn was started on a Project that already has one running.
    TurnAlreadyRunning,
    ProjectTooLarge,
    PermissionTimeout,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Checkpoint, CheckpointKind, EngineInfo, PlanStep, SessionMode, TurnPhase};

    fn turn_id() -> TurnId {
        uuid::Uuid::nil()
    }

    fn tool_call() -> ToolCallView {
        ToolCallView {
            id: "c1".into(),
            title: "Edit report.md".into(),
            kind: "edit".into(),
            status: "completed".into(),
            locations: vec!["/abs/report.md".into()],
            risk: RiskClass::Reversible,
            diff_summary: Some("3 lines changed in report.md".into()),
        }
    }

    fn checkpoint() -> Checkpoint {
        Checkpoint {
            id: "abc123".into(),
            project_id: uuid::Uuid::nil(),
            turn_id: Some(turn_id()),
            label: "Before: rename FY25 → FY26".into(),
            kind: CheckpointKind::PreTurn,
            created_at: chrono::DateTime::UNIX_EPOCH,
            files_changed: 3,
        }
    }

    /// One of every variant. The list is exhaustive by construction: adding a
    /// variant to `CoreEvent` without adding it here fails to compile, because
    /// of the match below.
    fn every_variant() -> Vec<CoreEvent> {
        let events = vec![
            CoreEvent::TurnStarted {
                turn_id: turn_id(),
                phase: TurnPhase::Planning,
            },
            CoreEvent::PhaseChanged {
                turn_id: turn_id(),
                phase: TurnPhase::Executing,
            },
            CoreEvent::AgentText {
                turn_id: turn_id(),
                text: "hello".into(),
            },
            CoreEvent::AgentThought {
                turn_id: turn_id(),
                text: "looking around".into(),
            },
            CoreEvent::ToolCallStarted {
                turn_id: turn_id(),
                call: tool_call(),
            },
            CoreEvent::ToolCallUpdated {
                turn_id: turn_id(),
                call: tool_call(),
            },
            CoreEvent::PlanUpdated {
                turn_id: turn_id(),
                entries: vec![PlanEntryView {
                    content: "Open the report".into(),
                    priority: Some("high".into()),
                    status: Some("pending".into()),
                }],
            },
            CoreEvent::PermissionRequested {
                turn_id: turn_id(),
                request: PermissionView {
                    request_id: "7".into(),
                    tool_call_id: "c1".into(),
                    title: "Edit report.md".into(),
                    risk: RiskClass::Reversible,
                    options: vec![PermissionOption {
                        option_id: "allow".into(),
                        name: "Allow".into(),
                        kind: "allow_once".into(),
                    }],
                    explanation: "report.md, inside this project".into(),
                },
            },
            CoreEvent::PermissionResolved {
                turn_id: turn_id(),
                request_id: "7".into(),
                decision: Decision::AllowOnce,
                by: DecidedBy::Policy,
            },
            CoreEvent::PlanReady {
                turn_id: turn_id(),
                plan: Plan {
                    steps: vec![PlanStep::new("Open the report")],
                    ..Plan::default()
                },
            },
            CoreEvent::CheckpointCreated {
                checkpoint: checkpoint(),
            },
            CoreEvent::Restored {
                to: "abc123".into(),
                new_checkpoint: "def456".into(),
                skipped_locked: vec!["budget.xlsx".into()],
            },
            CoreEvent::TurnFinished {
                turn_id: turn_id(),
                stop_reason: "end_turn".into(),
                digest: Some(Digest {
                    files_changed: vec!["report.md".into()],
                    undo_to: Some("abc123".into()),
                    ..Digest::default()
                }),
            },
            CoreEvent::EngineStatus {
                engine_id: "fake".into(),
                status: EngineStatus::Ready {
                    info: EngineInfo {
                        engine_id: "fake".into(),
                        ..EngineInfo::default()
                    },
                    modes: vec![SessionMode {
                        id: "plan".into(),
                        name: "Plan".into(),
                        description: None,
                    }],
                    current_mode: Some("plan".into()),
                },
            },
            CoreEvent::EngineCrashed {
                engine_id: "fake".into(),
                turn_id: Some(turn_id()),
                stderr_tail: vec!["panicked".into()],
            },
            CoreEvent::Error {
                turn_id: None,
                code: ErrorCode::CheckpointFailed,
                message: "budget.xlsx is open in Excel".into(),
                next_action: Some("Close open files and try again".into()),
            },
        ];

        // Exhaustiveness guard: a new variant must be added above.
        for event in &events {
            match event {
                CoreEvent::TurnStarted { .. }
                | CoreEvent::PhaseChanged { .. }
                | CoreEvent::AgentText { .. }
                | CoreEvent::AgentThought { .. }
                | CoreEvent::ToolCallStarted { .. }
                | CoreEvent::ToolCallUpdated { .. }
                | CoreEvent::PlanUpdated { .. }
                | CoreEvent::PermissionRequested { .. }
                | CoreEvent::PermissionResolved { .. }
                | CoreEvent::PlanReady { .. }
                | CoreEvent::CheckpointCreated { .. }
                | CoreEvent::Restored { .. }
                | CoreEvent::TurnFinished { .. }
                | CoreEvent::EngineStatus { .. }
                | CoreEvent::EngineCrashed { .. }
                | CoreEvent::Error { .. } => {}
            }
        }
        events
    }

    #[test]
    fn every_core_event_variant_round_trips_through_json() {
        for event in every_variant() {
            let json = serde_json::to_string(&event).expect("serialise");
            let back: CoreEvent = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(
                json,
                serde_json::to_string(&back).expect("re-serialise"),
                "round trip changed the event: {json}"
            );
        }
    }

    /// The tag is what the frontend switches on, and the plan writes it in
    /// `snake_case`. A rename here silently breaks every UI branch.
    #[test]
    fn events_are_tagged_in_snake_case() {
        let json = serde_json::to_value(&CoreEvent::TurnStarted {
            turn_id: turn_id(),
            phase: TurnPhase::Planning,
        })
        .unwrap();
        assert_eq!(json["type"], "turn_started");
        assert_eq!(json["phase"], "planning");
    }

    /// The store reads the Turn off the event rather than being told it
    /// separately, so every variant has to answer. The two that belong to no
    /// Turn say so: a restore happens between turns, and an engine's status is
    /// not about one.
    #[test]
    fn every_event_knows_which_turn_it_belongs_to() {
        for event in every_variant() {
            let expected = match &event {
                // The sample `Error` is one raised outside a turn, which is
                // the case the `Option` exists for.
                CoreEvent::Restored { .. }
                | CoreEvent::EngineStatus { .. }
                | CoreEvent::Error { .. } => None,
                _ => Some(turn_id()),
            };
            assert_eq!(
                event.turn_id(),
                expected,
                "wrong turn for {}",
                serde_json::to_value(&event).unwrap()["type"]
            );
        }
    }

    /// A checkpoint carries its own Turn, and the event around it must not
    /// disagree with it.
    #[test]
    fn a_checkpoint_event_takes_the_turn_from_the_checkpoint() {
        let mut without = checkpoint();
        without.turn_id = None;
        assert_eq!(
            CoreEvent::CheckpointCreated {
                checkpoint: without
            }
            .turn_id(),
            None
        );
    }

    #[test]
    fn a_dead_engine_becomes_a_crash_event_with_the_last_fifty_lines() {
        let stderr: Vec<String> = (0..200).map(|line| format!("line {line}")).collect();
        let error = crate::engine::EngineError::Crashed {
            engine_id: "goose".into(),
            reason: "the engine stopped responding".into(),
            stderr_tail: stderr,
        };

        match CoreEvent::from_engine_error(&error, Some(turn_id())) {
            CoreEvent::EngineCrashed {
                engine_id,
                turn_id: on_turn,
                stderr_tail,
            } => {
                assert_eq!(engine_id, "goose");
                assert_eq!(on_turn, Some(turn_id()));
                assert_eq!(stderr_tail.len(), STDERR_TAIL_LINES);
                assert_eq!(
                    stderr_tail.last().unwrap(),
                    "line 199",
                    "the end of stderr is the part that explains the crash"
                );
            }
            other => panic!("expected EngineCrashed, got {other:?}"),
        }
    }

    #[test]
    fn a_short_stderr_is_carried_whole() {
        let error = crate::engine::EngineError::Crashed {
            engine_id: "fake".into(),
            reason: "exited".into(),
            stderr_tail: vec!["read refused".into()],
        };
        match CoreEvent::from_engine_error(&error, None) {
            CoreEvent::EngineCrashed { stderr_tail, .. } => {
                assert_eq!(stderr_tail, vec!["read refused".to_owned()]);
            }
            other => panic!("expected EngineCrashed, got {other:?}"),
        }
    }

    /// Every other engine failure is an error the user can act on, so none of
    /// them may arrive without a next action.
    #[test]
    fn every_other_engine_failure_carries_a_next_action() {
        use crate::engine::EngineError;
        let failures = [
            EngineError::NotInstalled {
                engine_id: "goose".into(),
                searched: vec!["/usr/local/bin".into()],
            },
            EngineError::Spawn {
                engine_id: "goose".into(),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
            EngineError::NeedsSignIn {
                engine_id: "codex".into(),
                command: "codex login".into(),
            },
            EngineError::Timeout {
                engine_id: "goose".into(),
                during: "initialize".into(),
                timeout_secs: 15,
            },
            EngineError::Rpc {
                engine_id: "goose".into(),
                method: "session/prompt".into(),
                code: -32601,
                message: "no such method".into(),
            },
            EngineError::ProtocolVersion {
                engine_id: "goose".into(),
                got: 99,
                want: 1,
            },
            EngineError::Protocol {
                engine_id: "goose".into(),
                detail: "no session id".into(),
            },
            EngineError::NoSuchSession {
                engine_id: "goose".into(),
                session_id: "sess_x".into(),
            },
        ];

        for failure in failures {
            match CoreEvent::from_engine_error(&failure, None) {
                CoreEvent::Error {
                    message,
                    next_action,
                    ..
                } => {
                    assert!(!message.is_empty());
                    assert!(
                        next_action.is_some_and(|action| !action.is_empty()),
                        "{failure} has no next action"
                    );
                }
                other => panic!("expected an Error event for {failure}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_sign_in_error_names_the_command_to_run() {
        let error = crate::engine::EngineError::NeedsSignIn {
            engine_id: "codex".into(),
            command: "codex login".into(),
        };
        let CoreEvent::Error { next_action, .. } = CoreEvent::from_engine_error(&error, None)
        else {
            panic!("expected an Error event");
        };
        assert!(next_action.unwrap().contains("codex login"));
    }

    #[test]
    fn decision_falls_back_without_widening() {
        assert_eq!(Decision::AllowAlways.fallback(), Some(Decision::AllowOnce));
        assert_eq!(
            Decision::RejectAlways.fallback(),
            Some(Decision::RejectOnce)
        );
        assert_eq!(Decision::AllowOnce.fallback(), None);
        assert_eq!(Decision::RejectOnce.fallback(), None);
    }
}
