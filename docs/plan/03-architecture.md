# 03 — Architecture: Crates, Types, Events, IPC, Storage

## 1. Repository layout

```
Eavery/
├── Cargo.toml                  # workspace
├── rust-toolchain.toml         # channel = "stable"
├── crates/
│   ├── eavery-core/            # domain model, journal, policy, plan gate, store
│   ├── eavery-acp/             # ACP client: spawn engine, translate to CoreEvent
│   ├── eavery-engines/         # engine table, discovery, health check
│   ├── eavery-docs-mcp/        # binary: MCP server for docx/xlsx/pdf/pptx
│   ├── eavery-fake-agent/      # binary: scriptable ACP agent for tests
│   └── eavery-cli/             # binary: headless driver of eavery-core
├── apps/
│   └── desktop/
│       ├── src-tauri/          # Tauri v2 app (Rust), depends on eavery-core etc.
│       ├── src/                # React + TypeScript + Vite frontend
│       ├── package.json
│       └── tauri.conf.json
├── playbooks/                  # bundled Agent Skills (SKILL.md folders)
├── docs/
└── .github/workflows/ci.yml
```

Dependency direction (arrows mean "depends on"):

```
desktop ──▶ eavery-core ◀── eavery-cli
   │              ▲
   ├──▶ eavery-engines ──▶ eavery-acp ──▶ agent-client-protocol
   └──▶ eavery-acp
eavery-docs-mcp ──▶ rmcp, umya-spreadsheet, docx-rs, lopdf, zip, quick-xml
eavery-fake-agent ──▶ agent-client-protocol (agent side) or hand-rolled JSON-RPC
```

`eavery-core` must not depend on `eavery-acp`. Core defines the `Engine` trait;
`eavery-acp` implements it. This is what makes the fake agent and the CLI cheap.

## 2. Workspace `Cargo.toml` (starting point)

```toml
[workspace]
resolver = "2"
members = [
  "crates/eavery-core",
  "crates/eavery-acp",
  "crates/eavery-engines",
  "crates/eavery-docs-mcp",
  "crates/eavery-fake-agent",
  "crates/eavery-cli",
  "apps/desktop/src-tauri",
]

[workspace.package]
edition = "2024"
license = "MIT"
rust-version = "1.85"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
git2 = { version = "0.20", default-features = false, features = ["vendored-libgit2"] }
rusqlite = { version = "0.32", features = ["bundled"] }
agent-client-protocol = "2"
agent-client-protocol-schema = "2"
rmcp = { version = "0.8", features = ["server", "transport-io", "macros"] }
clap = { version = "4", features = ["derive"] }
directories = "6"
async-trait = "0.1"
futures = "0.3"
tempfile = "3"
ts-rs = { version = "10", features = ["serde-compat", "uuid-impl", "chrono-impl"] }
keyring = "3"
which = "7"
fix-path-env = { git = "https://github.com/tauri-apps/fix-path-env-rs" }
serde_yaml = "0.9"
zip = "2"
quick-xml = "0.37"
calamine = "0.26"
umya-spreadsheet = "2"
rust_xlsxwriter = "0.80"
docx-rs = "0.4"
pdf-extract = "0.8"
lopdf = "0.34"
```

The version numbers above are starting points, not verified pins; several of
these crates release often (`calamine` and `rust_xlsxwriter` were at 0.36 and
0.96 in September 2026). On M0-T01 run `cargo add <crate>` for each to get the
current version, then pin exact versions after the first successful build and
commit `Cargo.lock`. If a version above does not exist, take the latest
compatible one from crates.io and record it in `CHANGELOG-plan.md`. For
`git2`, disabling default features drops the `https` and `ssh` features; the
Journal never talks to a remote.

## 3. Core domain types (`eavery-core`)

```rust
// crates/eavery-core/src/model.rs
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type ProjectId = uuid::Uuid;
pub type SessionId = uuid::Uuid;      // Eavery's own id, not the engine's
pub type TurnId = uuid::Uuid;
pub type CheckpointId = String;       // git commit hex

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,                 // absolute
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub engine_id: Option<String>,     // last used engine
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub engine_id: String,
    pub engine_session_id: Option<String>, // ACP sessionId, for session/load
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase { Planning, AwaitingApproval, Executing, Done, Failed, Cancelled }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub id: TurnId,
    pub session_id: SessionId,
    pub request: String,               // what the user typed
    pub phase: TurnPhase,
    pub plan: Option<Plan>,
    pub pre_checkpoint: Option<CheckpointId>,
    pub post_checkpoint: Option<CheckpointId>,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Plan {
    pub summary: String,
    pub steps: Vec<PlanStep>,
    pub files_touched: Vec<String>,
    pub outbound: Vec<String>,         // "send email to x", "post to Slack #y"
    pub irreversible: Vec<String>,
    pub will_not_do: Vec<String>,
    pub raw_markdown: String,          // fallback rendering
    pub user_edits: Option<String>,    // free text the user added before approving
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanStep { pub text: String, pub done: bool }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass { Read, Reversible, Execute, Outbound, Destructive }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub project_id: ProjectId,
    pub turn_id: Option<TurnId>,
    pub label: String,                 // "Before: rename FY25 → FY26"
    pub kind: CheckpointKind,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub files_changed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointKind { PreTurn, PostTurn, Manual, Restore }
```

## 4. The event model

Everything the engine does becomes a `CoreEvent`. The ACP crate produces them,
the store persists them, the CLI prints them, the UI renders them.

```rust
// crates/eavery-core/src/event.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreEvent {
    TurnStarted { turn_id: TurnId, phase: TurnPhase },
    PhaseChanged { turn_id: TurnId, phase: TurnPhase },
    AgentText { turn_id: TurnId, text: String },              // streamed chunk
    AgentThought { turn_id: TurnId, text: String },           // hidden in Everyday
    ToolCallStarted { turn_id: TurnId, call: ToolCallView },
    ToolCallUpdated { turn_id: TurnId, call: ToolCallView },
    PlanUpdated { turn_id: TurnId, entries: Vec<PlanEntryView> }, // ACP `plan`
    PermissionRequested { turn_id: TurnId, request: PermissionView },
    PermissionResolved { turn_id: TurnId, request_id: String, decision: Decision, by: DecidedBy },
    PlanReady { turn_id: TurnId, plan: Plan },
    CheckpointCreated { checkpoint: Checkpoint },
    Restored { to: CheckpointId, new_checkpoint: CheckpointId, skipped_locked: Vec<String> },
    TurnFinished { turn_id: TurnId, stop_reason: String, digest: Option<Digest> },
    EngineStatus { engine_id: String, status: EngineStatus },
    Error { turn_id: Option<TurnId>, code: ErrorCode, message: String, next_action: Option<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallView {
    pub id: String,
    pub title: String,
    pub kind: String,        // ACP kind string, "other" if missing
    pub status: String,      // pending | in_progress | completed | failed
    pub locations: Vec<String>,
    pub risk: RiskClass,
    pub diff_summary: Option<String>,   // "3 lines changed in report.md"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionView {
    pub request_id: String,  // JSON-RPC id as string
    pub tool_call_id: String,
    pub title: String,
    pub risk: RiskClass,
    pub options: Vec<PermissionOption>, // from ACP, passed through
    pub explanation: String,            // vocabulary-neutral facts: paths, hosts
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PermissionOption { pub option_id: String, pub name: String, pub kind: String }

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision { AllowOnce, AllowAlways, RejectOnce, RejectAlways, Cancelled }

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy { Policy, User, PlanGate }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Digest {
    pub files_added: Vec<String>,
    pub files_changed: Vec<String>,
    pub files_removed: Vec<String>,
    pub outbound_actions: Vec<String>,  // what left the machine (from allowed outbound calls)
    pub refused_actions: Vec<String>,
    pub undo_to: Option<CheckpointId>,  // the pre-turn checkpoint
}
```

## 5. The `Engine` trait (core side)

```rust
// crates/eavery-core/src/engine.rs
#[async_trait::async_trait]
pub trait Engine: Send + Sync {
    /// Spawn the process, run initialize, return capabilities.
    async fn start(&mut self) -> Result<EngineInfo, EngineError>;
    /// session/new (or session/load if engine_session_id is Some and supported).
    async fn open_session(&mut self, cwd: &Path, mcp: &[McpServerSpec], resume: Option<&str>) -> Result<String, EngineError>;
    async fn set_mode(&mut self, session: &str, mode_id: &str) -> Result<(), EngineError>;
    /// Sends session/prompt. Streams CoreEvents to `tx` until the prompt returns.
    /// `permission` is called for every session/request_permission and must answer.
    async fn prompt(&mut self, session: &str, text: &str, tx: EventSink, permission: PermissionHandler) -> Result<StopReason, EngineError>;
    async fn cancel(&mut self, session: &str) -> Result<(), EngineError>;
    async fn shutdown(&mut self);
}

pub type EventSink = tokio::sync::mpsc::UnboundedSender<RawAgentEvent>;
pub type PermissionHandler = std::sync::Arc<dyn Fn(PermissionView) -> futures::future::BoxFuture<'static, Decision> + Send + Sync>;
```

`RawAgentEvent` is the ACP `SessionUpdate` mapped 1:1 (message chunk, thought
chunk, tool call, tool call update, plan, mode change, available commands).
`eavery-core::turn` turns raw events into `CoreEvent`s, adds turn ids, applies
policy, and writes the store. Keep the ACP crate dumb.

## 6. The turn state machine (`eavery-core::turn`)

```
User request
   │
   ▼
[pre-turn checkpoint]  ── fails ──▶ Error{next_action:"Close open files and try again"}; STOP
   │
   ▼
Planning ── prompt(plan_prompt), permission handler = PlanGateHandler (rejects mutations)
   │
   ├─ parse ```eavery-plan``` JSON → Plan  (fallback: raw markdown)
   ▼
AwaitingApproval ── user approves (optionally with edits) / rejects
   │
   ▼
Executing ── prompt(execute_prompt), permission handler = PolicyHandler (irreversibility axis)
   │
   ▼
[post-turn checkpoint] → Digest (diff pre..post + outbound log)
   │
   ▼
Done
```

Cancel is allowed in Planning and Executing. A cancel in Executing still takes
the post-turn checkpoint so the partial result is undoable.

Everyday mode can offer "Just do it" for requests the policy classifies as
read-only (the plan phase found no `files_touched`, no `outbound`). That is a
UI shortcut, not a bypass: the plan phase still runs.

## 7. Tauri IPC contract

The frontend is a renderer. It has no business logic. It calls these commands
and listens to one event.

Commands (`#[tauri::command]`, all return `Result<T, AppError>` serialised as
`{ code, message, next_action }`):

| Command | Args | Returns |
|---|---|---|
| `list_projects` | | `Project[]` |
| `open_project` | `{ path }` | `Project` (creates Journal if missing) |
| `remove_project` | `{ project_id }` | `()` (does not delete files or Journal) |
| `list_engines` | | `EngineInfo[]` with status |
| `set_project_engine` | `{ project_id, engine_id }` | `()` |
| `start_turn` | `{ project_id, request, mode: "plan" \| "direct" }` | `TurnId` |
| `approve_plan` | `{ turn_id, edits?: string }` | `()` |
| `reject_plan` | `{ turn_id }` | `()` |
| `answer_permission` | `{ turn_id, request_id, decision }` | `()` |
| `cancel_turn` | `{ turn_id }` | `()` |
| `list_checkpoints` | `{ project_id }` | `Checkpoint[]` |
| `checkpoint_now` | `{ project_id, label }` | `Checkpoint` |
| `restore_checkpoint` | `{ project_id, checkpoint_id }` | `Checkpoint` (the new restore commit) |
| `diff_summary` | `{ project_id, from, to }` | `Digest`-shaped file lists plus text diffs for text files |
| `list_events` | `{ session_id, after?: number, limit? }` | `CoreEvent[]` (history replay) |
| `list_connectors` / `upsert_connector` / `remove_connector` | | MCP server specs |
| `list_playbooks` | `{ project_id }` | `Playbook[]` |
| `get_settings` / `set_settings` | | `Settings` (mode, engine defaults, keys stored in OS keychain via `keyring` crate) |
| `run_health_check` | `{ engine_id }` | `EngineStatus` |

Event: `app.emit("core://event", CoreEvent)` for every event. The payload
includes `seq: u64` so the UI can detect gaps and re-fetch via `list_events`.

## 8. On-disk layout

Use the `directories` crate: `ProjectDirs::from("dev", "eavery", "Eavery")`.

```
<data_dir>/                      # e.g. ~/Library/Application Support/dev.eavery.Eavery
├── eavery.sqlite                # projects, sessions, turns, events, audit, settings
├── journals/<project-id>/       # bare-ish git dir with core.worktree = project root
├── engines/goose/<version>/     # downloaded goose binary (BYO-key / local path)
├── connectors.json              # MCP server specs (user-added)
└── logs/eavery.log              # tracing output, rotated
<config_dir>/                    # user-visible settings.json (mode, default engine)
~/.eavery/playbooks/             # user's Playbook library (Agent Skills folders)
<project root>/.agents/skills/   # project Playbooks (spec-compatible location)
<project root>/AGENTS.md         # project instructions, read by engines natively
```

SQLite schema (migrations in `eavery-core/src/store/migrations/*.sql`):

```sql
CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT, root TEXT UNIQUE, created_at TEXT, engine_id TEXT);
CREATE TABLE sessions (id TEXT PRIMARY KEY, project_id TEXT, engine_id TEXT, engine_session_id TEXT, created_at TEXT);
CREATE TABLE turns (id TEXT PRIMARY KEY, session_id TEXT, request TEXT, phase TEXT, plan_json TEXT, pre_checkpoint TEXT, post_checkpoint TEXT, started_at TEXT);
CREATE TABLE events (seq INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, turn_id TEXT, at TEXT, json TEXT);
CREATE TABLE checkpoints (id TEXT PRIMARY KEY, project_id TEXT, turn_id TEXT, label TEXT, kind TEXT, created_at TEXT, files_changed INTEGER);
CREATE TABLE audit (seq INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT, project_id TEXT, turn_id TEXT, actor TEXT, action TEXT, risk TEXT, detail_json TEXT);
CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT);
```

The `audit` table is append-only: no UPDATE or DELETE statements exist in the
code for it.

## 9. Logging and diagnostics

- `tracing` everywhere. Level from `EAVERY_LOG` env var, default `info`.
- Every engine spawn logs the resolved executable path, args, cwd, and PATH.
- Every raw ACP message is logged at `trace` level in `eavery-acp`
  (`EAVERY_LOG=eavery_acp=trace`).
- Developer mode has a "Diagnostics" panel that shows the log tail and a
  "Copy diagnostics" button. This will save hours during M1.
