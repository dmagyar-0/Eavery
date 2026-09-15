//! The domain model: Projects, Sessions, Turns, Plans, Checkpoints.
//!
//! Types here are the vocabulary of the whole product. They are shared with the
//! frontend through `ts-rs`, so every public type derives `TS` alongside serde.
//! See `docs/plan/03-architecture.md` §3.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub type ProjectId = uuid::Uuid;
/// Eavery's own session id, not the engine's. The engine's is
/// [`Session::engine_session_id`].
pub type SessionId = uuid::Uuid;
pub type TurnId = uuid::Uuid;
/// A git commit hex in the Journal.
pub type CheckpointId = String;

/// A folder on disk the user has opened. "Project" is the everyday word for a
/// workspace.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Absolute.
    pub root: PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// The engine last used for this Project, if any.
    pub engine_id: Option<String>,
}

/// One conversation with one engine about one Project.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Session {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub engine_id: String,
    /// The ACP `sessionId`, kept for `session/load`.
    pub engine_session_id: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Where a [`Turn`] is in the plan → approve → execute loop
/// (`docs/plan/06-plan-gate-permissions.md` §1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    Planning,
    AwaitingApproval,
    Executing,
    Done,
    Failed,
    Cancelled,
}

/// One user request and everything the engine does about it.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Turn {
    pub id: TurnId,
    pub session_id: SessionId,
    /// What the user typed.
    pub request: String,
    pub phase: TurnPhase,
    pub plan: Option<Plan>,
    pub pre_checkpoint: Option<CheckpointId>,
    pub post_checkpoint: Option<CheckpointId>,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

/// What the engine says it will do, shown to the user before anything runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Plan {
    pub summary: String,
    pub steps: Vec<PlanStep>,
    pub files_touched: Vec<String>,
    /// Actions that leave the machine: "send email to x", "post to Slack #y".
    pub outbound: Vec<String>,
    pub irreversible: Vec<String>,
    pub will_not_do: Vec<String>,
    /// The engine's full reply, used when the structured block is missing.
    pub raw_markdown: String,
    /// Free text the user added before approving.
    pub user_edits: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PlanStep {
    pub text: String,
    pub done: bool,
}

impl PlanStep {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            done: false,
        }
    }
}

/// Wire shape of the ```` ```eavery-plan ```` JSON block
/// (`docs/plan/06-plan-gate-permissions.md` §2.3).
///
/// The plan prompt asks for `steps` as plain strings, so this must not be
/// [`Plan`], whose `steps` is `Vec<PlanStep>`: the type mismatch would be a
/// serde error and every valid plan would silently fall back to raw markdown.
/// Parse into `PlanJson`, then convert with `Plan::from`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PlanJson {
    pub summary: String,
    pub steps: Vec<String>,
    pub files_touched: Vec<String>,
    pub outbound: Vec<String>,
    pub irreversible: Vec<String>,
    pub will_not_do: Vec<String>,
}

impl From<PlanJson> for Plan {
    fn from(json: PlanJson) -> Self {
        Plan {
            summary: json.summary,
            steps: json.steps.into_iter().map(PlanStep::new).collect(),
            files_touched: json.files_touched,
            outbound: json.outbound,
            irreversible: json.irreversible,
            will_not_do: json.will_not_do,
            raw_markdown: String::new(),
            user_edits: None,
        }
    }
}

/// How hard an action is to take back. Permission prompts are decided on this
/// axis, not on tool type (decision D7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    Read,
    Reversible,
    Execute,
    Outbound,
    Destructive,
}

/// A commit in the Journal. "Checkpoint" is the everyday word for commit.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub project_id: ProjectId,
    pub turn_id: Option<TurnId>,
    /// "Before: rename FY25 → FY26".
    pub label: String,
    pub kind: CheckpointKind,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub files_changed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointKind {
    PreTurn,
    PostTurn,
    Manual,
    Restore,
}

/// What an engine reported about itself at `initialize`
/// (`docs/plan/04-acp-engines.md` §1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct EngineInfo {
    pub engine_id: String,
    /// `agentInfo.name` from the initialize response, when the engine sends one.
    pub name: Option<String>,
    pub version: Option<String>,
    pub protocol_version: u16,
    /// `agentCapabilities.loadSession`: whether `session/load` may be used.
    pub load_session: bool,
    /// Auth methods the engine advertises. Empty means no auth step is needed.
    pub auth_methods: Vec<String>,
}

/// A mode the engine offers, from `session/new`'s optional `modes`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SessionMode {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// The result of a health check, rendered by
/// `docs/plan/07-ui-vocabulary.md` §5.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineStatus {
    /// The executable could not be found. `searched` is the list of places
    /// looked in, so the user is never told "not found" without saying where.
    NotInstalled {
        instructions: String,
        searched: Vec<String>,
    },
    /// Present, but its adapter needs Node and Node is missing.
    NeedsNode,
    /// Present and responding, but the user has not signed in yet.
    NeedsSignIn { command: String },
    /// A download is in progress.
    Installing { percent: u8 },
    /// The engine's own sign-in is open in the browser.
    SigningIn,
    Ready {
        info: EngineInfo,
        modes: Vec<SessionMode>,
        /// The mode the engine started in, when it reports modes at all.
        current_mode: Option<String>,
    },
    /// Installed but not usable right now. `reason` is developer-facing.
    Unavailable { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample block from `docs/plan/06-plan-gate-permissions.md` §2.3.
    /// This is the exact payload the plan prompt asks engines to emit; the test
    /// exists because `Plan` and the wire shape disagree about `steps` and the
    /// disagreement is silent (see `PlanJson`).
    const SAMPLE: &str = r#"{"summary":"...","steps":["..."],"files_touched":["relative/path.docx"],
 "outbound":[],"irreversible":[],"will_not_do":["send any email"]}"#;

    #[test]
    fn sample_plan_block_parses_with_populated_steps() {
        let json: PlanJson = serde_json::from_str(SAMPLE).expect("sample block is valid JSON");
        let plan = Plan::from(json);

        assert_eq!(plan.summary, "...");
        assert_eq!(plan.steps, vec![PlanStep::new("...")]);
        assert_eq!(plan.files_touched, vec!["relative/path.docx"]);
        assert!(plan.outbound.is_empty());
        assert!(plan.irreversible.is_empty());
        assert_eq!(plan.will_not_do, vec!["send any email"]);
    }

    #[test]
    fn plan_json_tolerates_missing_and_unknown_fields() {
        let json: PlanJson =
            serde_json::from_str(r#"{"summary":"Just look","surprise":true}"#).unwrap();
        let plan = Plan::from(json);
        assert_eq!(plan.summary, "Just look");
        assert!(plan.steps.is_empty());
    }

    /// A realistic plan does not deserialise as a `Plan` directly. If this ever
    /// starts succeeding, `PlanJson` has become redundant; if it fails
    /// differently, the wire shape changed.
    #[test]
    fn plan_is_not_the_wire_shape() {
        assert!(serde_json::from_str::<Plan>(SAMPLE).is_err());
    }
}
