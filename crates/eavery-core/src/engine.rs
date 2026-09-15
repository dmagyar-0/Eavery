//! The engine contract. `eavery-core` defines it; `eavery-acp` implements it.
//!
//! Every method takes `&self`. [`Engine::prompt`] blocks for a whole turn and
//! [`Engine::cancel`] must be callable from another task while it is blocked;
//! with `&mut self` the borrow checker forbids exactly that. Implementations
//! keep their mutable state behind channels or a mutex.
//!
//! See `docs/plan/03-architecture.md` §5 and `docs/plan/04-acp-engines.md` §6.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::event::{Decision, PermissionView};
use crate::model::{EngineInfo, SessionMode};

/// An MCP server to hand the engine in `session/new`. "Connector" is the
/// everyday word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpServerSpec {
    /// A child process speaking MCP over stdio.
    Stdio {
        name: String,
        command: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        /// Passed to the server process only. Eavery never writes these to the
        /// engine's own config.
        #[serde(default)]
        env: Vec<(String, String)>,
    },
    Http {
        name: String,
        url: String,
        #[serde(default)]
        headers: Vec<(String, String)>,
    },
}

impl McpServerSpec {
    pub fn name(&self) -> &str {
        match self {
            McpServerSpec::Stdio { name, .. } | McpServerSpec::Http { name, .. } => name,
        }
    }
}

/// Why a turn ended, from the `session/prompt` response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

impl StopReason {
    /// ACP is allowed to grow new stop reasons; an unknown one must not fail a
    /// turn that already did its work. Anything unrecognised is treated as a
    /// normal end of turn and logged by the caller.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "max_tokens" => StopReason::MaxTokens,
            "max_turn_requests" => StopReason::MaxTurnRequests,
            "refusal" => StopReason::Refusal,
            "cancelled" => StopReason::Cancelled,
            _ => StopReason::EndTurn,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::MaxTurnRequests => "max_turn_requests",
            StopReason::Refusal => "refusal",
            StopReason::Cancelled => "cancelled",
        }
    }
}

/// What the engine did, mapped 1:1 from ACP `session/update`
/// (`docs/plan/04-acp-engines.md` §6). Deliberately dumb: no turn ids, no
/// policy, no vocabulary. `eavery-core::turn` turns these into `CoreEvent`s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RawAgentEvent {
    /// `agent_message_chunk`.
    Text(String),
    /// `agent_thought_chunk`.
    Thought(String),
    ToolCall(RawToolCall),
    ToolCallUpdate(RawToolCallUpdate),
    /// `plan`: the engine's own checklist.
    PlanEntries(Vec<RawPlanEntry>),
    /// `current_mode_update`.
    ModeChanged(String),
    /// Anything Eavery does not model: `available_commands_update`,
    /// `usage_update`, non-text content blocks, and any `sessionUpdate` value
    /// this version has never heard of. Logged, never shown, never fatal.
    Other(serde_json::Value),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RawToolCall {
    pub id: String,
    pub title: String,
    /// `read` | `edit` | `delete` | `move` | `search` | `execute` | `think` |
    /// `fetch` | `other`. `other` when the engine omits it.
    pub kind: String,
    /// `pending` | `in_progress` | `completed` | `failed`.
    pub status: String,
    pub locations: Vec<String>,
    /// Paths from `content[].type == "diff"` entries.
    pub diff_paths: Vec<String>,
    pub raw_input: Option<serde_json::Value>,
}

/// A `tool_call_update`: everything except the id is optional, and absent means
/// "unchanged", not "cleared".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RawToolCallUpdate {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub locations: Option<Vec<String>>,
    pub diff_paths: Option<Vec<String>>,
    pub raw_output: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawPlanEntry {
    pub content: String,
    pub priority: Option<String>,
    pub status: Option<String>,
}

/// Where the raw events go while a prompt is in flight.
pub type EventSink = tokio::sync::mpsc::UnboundedSender<RawAgentEvent>;

/// Answers `session/request_permission`. It must always answer: the engine is
/// blocked until it does. Implementations hand the request to the UI (or the
/// terminal) and await the reply under a timeout, after which they cancel.
pub type PermissionHandler =
    Arc<dyn Fn(PermissionView) -> futures::future::BoxFuture<'static, Decision> + Send + Sync>;

/// What went wrong talking to an engine. These map onto
/// [`crate::event::ErrorCode`] at the turn layer, where a next action is added.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{engine_id} is not installed (looked in: {})", searched.join(", "))]
    NotInstalled {
        engine_id: String,
        searched: Vec<String>,
    },
    #[error("could not start {engine_id}: {source}")]
    Spawn {
        engine_id: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{engine_id} needs you to sign in")]
    NeedsSignIn { engine_id: String, command: String },
    #[error("{engine_id} did not respond within {timeout_secs}s during {during}")]
    Timeout {
        engine_id: String,
        during: String,
        timeout_secs: u64,
    },
    #[error("{engine_id} stopped: {reason}")]
    Crashed {
        engine_id: String,
        reason: String,
        stderr_tail: Vec<String>,
    },
    /// A JSON-RPC error object came back for a request Eavery sent.
    #[error("{engine_id} refused {method}: {message} (code {code})")]
    Rpc {
        engine_id: String,
        method: String,
        code: i64,
        message: String,
    },
    #[error("{engine_id} speaks protocol version {got}, and Eavery speaks {want}")]
    ProtocolVersion {
        engine_id: String,
        got: u16,
        want: u16,
    },
    #[error("could not understand {engine_id}: {0}", .detail)]
    Protocol { engine_id: String, detail: String },
    #[error("no session {session_id} on {engine_id}")]
    NoSuchSession {
        engine_id: String,
        session_id: String,
    },
}

/// The result of opening a session: the engine's session id plus whatever
/// modes it offers, which the plan gate needs to pick a plan mode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenedSession {
    pub session_id: String,
    pub modes: Vec<SessionMode>,
    pub current_mode: Option<String>,
}

/// One engine process, driven over ACP.
#[async_trait::async_trait]
pub trait Engine: Send + Sync {
    /// Spawn the process and run `initialize`.
    async fn start(&self) -> Result<EngineInfo, EngineError>;

    /// `session/new`, or `session/load` when `resume` is set and the engine
    /// advertised `loadSession`.
    async fn open_session(
        &self,
        cwd: &Path,
        mcp: &[McpServerSpec],
        resume: Option<&str>,
    ) -> Result<OpenedSession, EngineError>;

    /// `session/set_mode`. A failure here is not fatal: the client-side plan
    /// gate holds regardless (`docs/plan/06-plan-gate-permissions.md` §2.1).
    async fn set_mode(&self, session: &str, mode_id: &str) -> Result<(), EngineError>;

    /// `session/prompt`. Streams [`RawAgentEvent`]s to `tx` and returns when
    /// the turn ends. `permission` is called for every
    /// `session/request_permission` and must answer.
    async fn prompt(
        &self,
        session: &str,
        text: &str,
        tx: EventSink,
        permission: PermissionHandler,
    ) -> Result<StopReason, EngineError>;

    /// `session/cancel`. Safe to call while [`Self::prompt`] is in flight; the
    /// outstanding prompt then returns [`StopReason::Cancelled`].
    async fn cancel(&self, session: &str) -> Result<(), EngineError>;

    /// The last lines of the engine's stderr, for crash reports.
    async fn stderr_tail(&self) -> Vec<String>;

    async fn shutdown(&self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_stop_reasons_end_the_turn_normally() {
        assert_eq!(StopReason::parse("end_turn"), StopReason::EndTurn);
        assert_eq!(StopReason::parse("cancelled"), StopReason::Cancelled);
        assert_eq!(StopReason::parse("something_new"), StopReason::EndTurn);
    }

    #[test]
    fn stop_reason_round_trips_through_its_wire_string() {
        for reason in [
            StopReason::EndTurn,
            StopReason::MaxTokens,
            StopReason::MaxTurnRequests,
            StopReason::Refusal,
            StopReason::Cancelled,
        ] {
            assert_eq!(StopReason::parse(reason.as_str()), reason);
        }
    }

    #[test]
    fn mcp_server_specs_serialise_by_transport() {
        let stdio = McpServerSpec::Stdio {
            name: "eavery-docs".into(),
            command: PathBuf::from("/abs/eavery-docs-mcp"),
            args: vec![],
            env: vec![],
        };
        let json = serde_json::to_value(&stdio).unwrap();
        assert_eq!(json["transport"], "stdio");
        assert_eq!(json["name"], "eavery-docs");
        assert_eq!(stdio.name(), "eavery-docs");
    }

    /// The trait has to stay object-safe: the turn engine holds engines as
    /// `Arc<dyn Engine>` so the CLI, the desktop app and the tests can swap
    /// implementations.
    #[test]
    fn engine_trait_is_object_safe() {
        fn assert_object_safe(_: Option<&dyn Engine>) {}
        assert_object_safe(None);
    }
}
