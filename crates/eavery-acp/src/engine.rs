//! [`AcpEngine`]: an [`Engine`] driven over ACP.
//!
//! It owns the connection, the per-turn routing of `session/update`
//! notifications, and the answers to the three requests an agent makes of its
//! client: `session/request_permission`, `fs/read_text_file` and
//! `fs/write_text_file`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use eavery_core::engine::{
    Engine, EngineError, EventSink, McpServerSpec, OpenedSession, PermissionHandler, StopReason,
};
use eavery_core::event::{Decision, PermissionOption, PermissionView};
use eavery_core::model::{EngineInfo, RiskClass, SessionMode};
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::conn::{ClientHandler, Connection, HandlerError, LaunchSpec};
use crate::wire;

/// The handshake gets 15 seconds (`docs/plan/04-acp-engines.md` §9). A prompt
/// gets none: a turn legitimately takes minutes.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a permission request may sit unanswered before the client answers
/// `cancelled` on the user's behalf (§5). Long enough that a person can read
/// the plan, go and check something, and come back.
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(600);

/// What the engine is allowed to do to the filesystem through the client.
///
/// v1 keeps the two rules the locked decisions fix: reads are served from
/// anywhere the engine could read itself (D15), writes only inside the Project
/// (D6, D15). The plan gate's extra refusal during planning is M4-T03, which
/// flips `writes_allowed`.
#[derive(Debug)]
pub struct FsGuard {
    project_root: RwLock<Option<PathBuf>>,
    writes_allowed: std::sync::atomic::AtomicBool,
}

impl FsGuard {
    fn new() -> Self {
        Self {
            project_root: RwLock::new(None),
            writes_allowed: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Refuses every write until re-enabled. The plan phase uses this.
    pub fn set_writes_allowed(&self, allowed: bool) {
        self.writes_allowed
            .store(allowed, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn writes_allowed(&self) -> bool {
        self.writes_allowed
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Everything that is true only while one turn is in flight.
#[derive(Clone)]
struct TurnContext {
    sink: EventSink,
    permission: PermissionHandler,
}

pub struct AcpEngine {
    spec: LaunchSpec,
    connection: RwLock<Option<Arc<Connection>>>,
    shared: Arc<Shared>,
}

/// The half of the engine the connection's handler tasks reach into. Separate
/// from `AcpEngine` so the handler can be built before the connection exists.
struct Shared {
    engine_id: String,
    /// A blocking lock on purpose. `session/update` notifications are
    /// dispatched inline on the reader task so they keep their order, and that
    /// task cannot await. Nothing held under this lock blocks: sending on an
    /// unbounded channel returns immediately.
    turn: std::sync::RwLock<Option<TurnContext>>,
    fs: FsGuard,
}

impl AcpEngine {
    pub fn new(spec: LaunchSpec) -> Self {
        let engine_id = spec.engine_id.clone();
        Self {
            spec,
            connection: RwLock::new(None),
            shared: Arc::new(Shared {
                engine_id,
                turn: std::sync::RwLock::new(None),
                fs: FsGuard::new(),
            }),
        }
    }

    /// The filesystem guard, so the turn engine can close writes during the
    /// plan phase.
    pub fn fs_guard(&self) -> &FsGuard {
        &self.shared.fs
    }

    async fn connection(&self) -> Result<Arc<Connection>, EngineError> {
        self.connection
            .read()
            .await
            .clone()
            .ok_or_else(|| EngineError::Protocol {
                engine_id: self.spec.engine_id.clone(),
                detail: "the engine was used before it was started".to_owned(),
            })
    }
}

#[async_trait::async_trait]
impl Engine for AcpEngine {
    async fn start(&self) -> Result<EngineInfo, EngineError> {
        let shared = Arc::clone(&self.shared);
        let notifications = {
            let shared = Arc::clone(&self.shared);
            Arc::new(move |method: &str, params: Value| {
                if method != "session/update" {
                    tracing::debug!(engine = %shared.engine_id, method, "ignoring notification");
                    return;
                }
                let notification: wire::SessionNotification = match serde_json::from_value(
                    params.clone(),
                ) {
                    Ok(notification) => notification,
                    Err(error) => {
                        tracing::warn!(engine = %shared.engine_id, %error, "malformed session/update");
                        return;
                    }
                };
                let Some(event) = wire::map_session_update(&notification.update) else {
                    return;
                };
                // Dispatched here rather than on a spawned task: two updates
                // handed to two tasks can arrive in either order, and the whole
                // point of the stream is that it is in order.
                match shared.turn.read().expect("turn lock").as_ref() {
                    // A send failure means the receiver is gone: the turn ended
                    // while this update was in flight.
                    Some(context) => drop(context.sink.send(event)),
                    None => tracing::debug!(
                        engine = %shared.engine_id,
                        "dropping an update that arrived outside a turn"
                    ),
                }
            })
        };

        let connection = Arc::new(Connection::spawn(&self.spec, shared, notifications)?);
        *self.connection.write().await = Some(Arc::clone(&connection));

        let response = connection
            .request_within("initialize", wire::initialize_params(), HANDSHAKE_TIMEOUT)
            .await?;
        let response: wire::InitializeResponse =
            serde_json::from_value(response).map_err(|error| EngineError::Protocol {
                engine_id: self.spec.engine_id.clone(),
                detail: format!("could not read the initialize response: {error}"),
            })?;

        if response.protocol_version != wire::PROTOCOL_VERSION {
            return Err(EngineError::ProtocolVersion {
                engine_id: self.spec.engine_id.clone(),
                got: response.protocol_version,
                want: wire::PROTOCOL_VERSION,
            });
        }

        Ok(EngineInfo {
            engine_id: self.spec.engine_id.clone(),
            name: response.agent_info.as_ref().map(|info| info.name.clone()),
            version: response
                .agent_info
                .as_ref()
                .map(|info| info.version.clone()),
            protocol_version: response.protocol_version,
            load_session: response.agent_capabilities.load_session,
            auth_methods: response
                .auth_methods
                .into_iter()
                .map(|method| method.id)
                .collect(),
        })
    }

    async fn open_session(
        &self,
        cwd: &Path,
        mcp: &[McpServerSpec],
        resume: Option<&str>,
    ) -> Result<OpenedSession, EngineError> {
        let connection = self.connection().await?;
        *self.shared.fs.project_root.write().await = Some(cwd.to_path_buf());

        let servers: Vec<Value> = mcp.iter().map(mcp_server_json).collect();

        // `session/load` replays the old session's history as notifications, so
        // it is only sent when the caller has somewhere to put them.
        let (method, params) = match resume {
            Some(session_id) => (
                "session/load",
                json!({ "sessionId": session_id, "cwd": cwd, "mcpServers": servers }),
            ),
            None => ("session/new", json!({ "cwd": cwd, "mcpServers": servers })),
        };

        let response = connection
            .request_within(method, params, HANDSHAKE_TIMEOUT)
            .await?;
        let parsed: wire::NewSessionResponse =
            serde_json::from_value(response).map_err(|error| EngineError::Protocol {
                engine_id: self.spec.engine_id.clone(),
                detail: format!("could not read the {method} response: {error}"),
            })?;

        let session_id = match resume {
            // `session/load` answers with no session id: it is the one we asked
            // to load.
            Some(existing) if parsed.session_id.is_empty() => existing.to_owned(),
            _ => parsed.session_id,
        };
        if session_id.is_empty() {
            return Err(EngineError::Protocol {
                engine_id: self.spec.engine_id.clone(),
                detail: format!("{method} did not return a session id"),
            });
        }

        let (modes, current_mode) = match parsed.modes {
            Some(state) => (
                state
                    .available_modes
                    .into_iter()
                    .map(|mode| SessionMode {
                        id: mode.id,
                        name: mode.name,
                        description: mode.description,
                    })
                    .collect(),
                Some(state.current_mode_id),
            ),
            None => (Vec::new(), None),
        };

        Ok(OpenedSession {
            session_id,
            modes,
            current_mode,
        })
    }

    async fn set_mode(&self, session: &str, mode_id: &str) -> Result<(), EngineError> {
        let connection = self.connection().await?;
        connection
            .request_within(
                "session/set_mode",
                json!({ "sessionId": session, "modeId": mode_id }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
        Ok(())
    }

    async fn prompt(
        &self,
        session: &str,
        text: &str,
        tx: EventSink,
        permission: PermissionHandler,
    ) -> Result<StopReason, EngineError> {
        let connection = self.connection().await?;
        *self.shared.turn.write().expect("turn lock") = Some(TurnContext {
            sink: tx,
            permission,
        });

        let params = json!({
            "sessionId": session,
            "prompt": [{ "type": "text", "text": text }],
        });
        let result = connection.request("session/prompt", params).await;

        // The turn is over either way: no more updates belong to this sink.
        *self.shared.turn.write().expect("turn lock") = None;

        let response = result?;
        let response: wire::PromptResponse =
            serde_json::from_value(response).map_err(|error| EngineError::Protocol {
                engine_id: self.spec.engine_id.clone(),
                detail: format!("could not read the prompt response: {error}"),
            })?;

        let stop = StopReason::parse(&response.stop_reason);
        if stop == StopReason::EndTurn && response.stop_reason != "end_turn" {
            tracing::info!(
                engine = %self.spec.engine_id,
                stop_reason = %response.stop_reason,
                "engine ended the turn with a stop reason Eavery does not model"
            );
        }
        Ok(stop)
    }

    async fn cancel(&self, session: &str) -> Result<(), EngineError> {
        let connection = self.connection().await?;
        // A notification, so this returns immediately; the outstanding prompt
        // is what reports `cancelled` (04 §1).
        connection.notify("session/cancel", json!({ "sessionId": session }));
        Ok(())
    }

    async fn stderr_tail(&self) -> Vec<String> {
        match self.connection.read().await.as_ref() {
            Some(connection) => connection.stderr_tail().await,
            None => Vec::new(),
        }
    }

    async fn shutdown(&self) {
        let connection = self.connection.write().await.take();
        if let Some(connection) = connection {
            connection.shutdown().await;
        }
        *self.shared.turn.write().expect("turn lock") = None;
    }
}

#[async_trait::async_trait]
impl ClientHandler for Shared {
    async fn handle(&self, method: &str, params: Value) -> Result<Value, HandlerError> {
        match method {
            "session/request_permission" => self.request_permission(params).await,
            "fs/read_text_file" => self.read_text_file(params).await,
            "fs/write_text_file" => self.write_text_file(params).await,
            other => Err(HandlerError::method_not_found(other)),
        }
    }
}

impl Shared {
    async fn request_permission(&self, params: Value) -> Result<Value, HandlerError> {
        let request: wire::RequestPermissionParams =
            serde_json::from_value(params).map_err(|error| {
                HandlerError::refused(format!("malformed permission request: {error}"))
            })?;

        let options: Vec<PermissionOption> = request
            .options
            .iter()
            .map(|option| PermissionOption {
                option_id: option.option_id.clone(),
                name: option.name.clone(),
                kind: option.kind.clone(),
            })
            .collect();

        let locations: Vec<String> = request
            .tool_call
            .locations
            .unwrap_or_default()
            .into_iter()
            .map(|location| location.path)
            .collect();
        let kind = request.tool_call.kind.unwrap_or_else(|| "other".to_owned());
        let title = request.tool_call.title.unwrap_or_default();

        let view = PermissionView {
            // The plan calls for the JSON-RPC id here, but the handler runs
            // after the id has been consumed by the dispatcher, and the tool
            // call id is what the engine and the audit log both key on.
            request_id: request.tool_call.tool_call_id.clone(),
            tool_call_id: request.tool_call.tool_call_id.clone(),
            title: title.clone(),
            risk: provisional_risk(&kind),
            options,
            explanation: explain(&kind, &locations),
        };

        let handler = {
            // Scoped so the guard is gone before the await below. This is a
            // blocking lock, and holding one across an await is how a runtime
            // deadlocks.
            let turn = self.turn.read().expect("turn lock");
            match turn.as_ref() {
                Some(context) => Arc::clone(&context.permission),
                // Nothing is waiting to answer, and answering "allow" on the
                // user's behalf is exactly what must never happen.
                None => {
                    tracing::warn!(
                        engine = %self.engine_id,
                        %title,
                        "permission requested outside a turn; cancelling it"
                    );
                    return Ok(cancelled_outcome());
                }
            }
        };

        let decision = match tokio::time::timeout(PERMISSION_TIMEOUT, handler(view)).await {
            Ok(decision) => decision,
            Err(_) => {
                tracing::warn!(
                    engine = %self.engine_id,
                    %title,
                    "nobody answered the permission request; cancelling it"
                );
                Decision::Cancelled
            }
        };

        Ok(answer(decision, &request.options))
    }

    async fn read_text_file(&self, params: Value) -> Result<Value, HandlerError> {
        let request: wire::ReadTextFileParams = serde_json::from_value(params)
            .map_err(|error| HandlerError::refused(format!("malformed read request: {error}")))?;

        // D15: reads are served from anywhere the engine's own tools could
        // read. Playbooks live outside the Project and engines are told to read
        // them; refusing would break them for no safety gain.
        let path = PathBuf::from(&request.path);
        if !is_inside_project(&path, self.project_root().await.as_deref()) {
            tracing::debug!(engine = %self.engine_id, path = %request.path, "reading outside the project");
        }

        let text = std::fs::read_to_string(&path).map_err(|error| {
            HandlerError::refused(format!("could not read {}: {error}", request.path))
        })?;

        let text = match (request.line, request.limit) {
            (None, None) => text,
            (line, limit) => {
                // ACP line numbers are 1-based.
                let skip = line.unwrap_or(1).saturating_sub(1) as usize;
                let take = limit.map(|l| l as usize).unwrap_or(usize::MAX);
                text.lines()
                    .skip(skip)
                    .take(take)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        Ok(json!({ "content": text }))
    }

    async fn write_text_file(&self, params: Value) -> Result<Value, HandlerError> {
        let request: wire::WriteTextFileParams = serde_json::from_value(params)
            .map_err(|error| HandlerError::refused(format!("malformed write request: {error}")))?;

        if !self.fs.writes_allowed() {
            return Err(HandlerError::refused(
                "Eavery is in planning mode; no changes are allowed yet",
            ));
        }

        let path = PathBuf::from(&request.path);
        let root = self.project_root().await;
        if !is_inside_project(&path, root.as_deref()) {
            return Err(HandlerError::refused(format!(
                "{} is outside this project, and Eavery only writes inside it",
                request.path
            )));
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                HandlerError::refused(format!("could not create {}: {error}", parent.display()))
            })?;
        }
        std::fs::write(&path, &request.content).map_err(|error| {
            HandlerError::refused(format!("could not write {}: {error}", request.path))
        })?;
        Ok(json!({}))
    }

    async fn project_root(&self) -> Option<PathBuf> {
        self.fs.project_root.read().await.clone()
    }
}

/// A first-pass risk class from the tool kind alone
/// (`docs/plan/06-plan-gate-permissions.md` §3.1).
///
/// It is provisional on purpose: the real classification needs the Project root
/// and the Connector registry, neither of which this layer has, so
/// `eavery-core::policy::classify` (M4-T02) reclassifies before anything is
/// decided. Anything unrecognised lands on `Execute`, never on `Read`.
fn provisional_risk(kind: &str) -> RiskClass {
    match kind {
        "read" | "search" | "think" | "other" => RiskClass::Read,
        "edit" | "delete" | "move" => RiskClass::Reversible,
        "fetch" => RiskClass::Outbound,
        _ => RiskClass::Execute,
    }
}

/// Facts, not phrasing: what and where. The vocabulary layer turns this into
/// something a person reads (working rule 9).
fn explain(kind: &str, locations: &[String]) -> String {
    match locations {
        [] => format!("{kind} (no files named)"),
        [one] => format!("{kind}: {one}"),
        many => format!("{kind}: {} and {} more", many[0], many.len() - 1),
    }
}

/// Turns a [`Decision`] into the ACP outcome, picking the option whose `kind`
/// matches and falling back per `docs/plan/04-acp-engines.md` §1. The fallback
/// can only narrow: if a decision cannot be expressed with the options offered,
/// the request is cancelled rather than allowed.
fn answer(decision: Decision, options: &[wire::PermissionOptionWire]) -> Value {
    let pick = |kind: &str| options.iter().find(|option| option.kind == kind);

    let chosen = pick(decision.wanted_kind()).or_else(|| {
        decision
            .fallback()
            .and_then(|fallback| pick(fallback.wanted_kind()))
    });

    match chosen {
        Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option.option_id}}),
        None => cancelled_outcome(),
    }
}

fn cancelled_outcome() -> Value {
    json!({ "outcome": { "outcome": "cancelled" } })
}

/// Whether `path` sits inside `root`. A path that does not exist yet is judged
/// by its nearest existing ancestor, because a write creates its target.
///
/// M4-T02 replaces this with `eavery-core::policy::is_inside`, which also
/// handles Windows verbatim paths. Until then this is deliberately strict:
/// without a Project root, nothing is inside one.
fn is_inside_project(path: &Path, root: Option<&Path>) -> bool {
    let Some(root) = root else { return false };
    let root = canonical_or_self(root);
    let candidate = nearest_existing(path);
    candidate.starts_with(&root)
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Canonicalises as much of `path` as exists, keeping the rest, so a file about
/// to be created is judged by the directory it will land in.
fn nearest_existing(path: &Path) -> PathBuf {
    if path.exists() {
        return canonical_or_self(path);
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => nearest_existing(parent).join(name),
        _ => path.to_path_buf(),
    }
}

fn mcp_server_json(spec: &McpServerSpec) -> Value {
    match spec {
        McpServerSpec::Stdio {
            name,
            command,
            args,
            env,
        } => json!({
            "name": name,
            "command": command,
            "args": args,
            "env": env.iter().map(|(name, value)| json!({"name": name, "value": value}))
                .collect::<Vec<_>>(),
        }),
        McpServerSpec::Http { name, url, headers } => json!({
            "type": "http",
            "name": name,
            "url": url,
            "headers": headers.iter().map(|(name, value)| json!({"name": name, "value": value}))
                .collect::<Vec<_>>(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(kinds: &[&str]) -> Vec<wire::PermissionOptionWire> {
        kinds
            .iter()
            .map(|kind| wire::PermissionOptionWire {
                option_id: format!("id_{kind}"),
                name: kind.to_string(),
                kind: kind.to_string(),
            })
            .collect()
    }

    #[test]
    fn a_decision_picks_the_option_with_the_matching_kind() {
        let offered = options(&["allow_once", "allow_always", "reject_once"]);
        assert_eq!(
            answer(Decision::RejectOnce, &offered)["outcome"]["optionId"],
            "id_reject_once"
        );
        assert_eq!(
            answer(Decision::AllowAlways, &offered)["outcome"]["optionId"],
            "id_allow_always"
        );
    }

    #[test]
    fn a_missing_always_option_narrows_to_once() {
        let offered = options(&["allow_once", "reject_once"]);
        assert_eq!(
            answer(Decision::AllowAlways, &offered)["outcome"]["optionId"],
            "id_allow_once"
        );
        assert_eq!(
            answer(Decision::RejectAlways, &offered)["outcome"]["optionId"],
            "id_reject_once"
        );
    }

    /// The important direction: when a refusal cannot be expressed, the answer
    /// is a cancellation, never an allow.
    #[test]
    fn a_decision_that_cannot_be_expressed_cancels() {
        let only_allow = options(&["allow_once"]);
        assert_eq!(
            answer(Decision::RejectOnce, &only_allow)["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(
            answer(Decision::Cancelled, &only_allow)["outcome"]["outcome"],
            "cancelled"
        );
        assert_eq!(
            answer(Decision::AllowOnce, &[])["outcome"]["outcome"],
            "cancelled"
        );
    }

    #[test]
    fn unknown_tool_kinds_are_not_treated_as_reads() {
        assert_eq!(provisional_risk("read"), RiskClass::Read);
        assert_eq!(provisional_risk("edit"), RiskClass::Reversible);
        assert_eq!(provisional_risk("fetch"), RiskClass::Outbound);
        assert_eq!(provisional_risk("execute"), RiskClass::Execute);
        assert_eq!(provisional_risk("something_new"), RiskClass::Execute);
    }

    #[test]
    fn explanations_are_facts_not_phrasing() {
        assert_eq!(explain("edit", &["/p/a.md".into()]), "edit: /p/a.md");
        assert_eq!(
            explain("edit", &["/p/a.md".into(), "/p/b.md".into()]),
            "edit: /p/a.md and 1 more"
        );
        assert_eq!(explain("execute", &[]), "execute (no files named)");
    }

    #[test]
    fn nothing_is_inside_a_project_that_has_not_been_opened() {
        assert!(!is_inside_project(Path::new("/tmp/anything"), None));
    }

    #[test]
    fn paths_are_judged_against_the_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/here.txt"), "x").unwrap();

        assert!(is_inside_project(&root.join("sub/here.txt"), Some(root)));
        // A file that does not exist yet is judged by where it would land.
        assert!(is_inside_project(&root.join("sub/new.txt"), Some(root)));
        assert!(!is_inside_project(Path::new("/etc/passwd"), Some(root)));

        let outside = tempfile::tempdir().unwrap();
        assert!(!is_inside_project(
            &outside.path().join("other.txt"),
            Some(root)
        ));
    }

    #[test]
    fn mcp_servers_serialise_in_the_shape_session_new_expects() {
        let stdio = mcp_server_json(&McpServerSpec::Stdio {
            name: "eavery-docs".into(),
            command: PathBuf::from("/abs/eavery-docs-mcp"),
            args: vec!["--root".into()],
            env: vec![("A".into(), "1".into())],
        });
        assert_eq!(stdio["name"], "eavery-docs");
        assert_eq!(stdio["args"][0], "--root");
        assert_eq!(stdio["env"][0]["name"], "A");
        assert_eq!(stdio["env"][0]["value"], "1");
        assert!(stdio.get("type").is_none(), "stdio is the untagged variant");

        let http = mcp_server_json(&McpServerSpec::Http {
            name: "remote".into(),
            url: "https://example/mcp".into(),
            headers: vec![("Authorization".into(), "Bearer x".into())],
        });
        assert_eq!(http["type"], "http");
        assert_eq!(http["headers"][0]["name"], "Authorization");
    }
}
