//! A JSON-RPC 2.0 connection to an engine child process, over its stdio
//! (`docs/plan/04-acp-engines.md` §4 and §7).
//!
//! Four tasks run per connection: one reads stdout and dispatches, one writes
//! stdin, one drains stderr into a ring buffer, and one waits for the child so
//! that a crash fails every outstanding request instead of hanging it.
//!
//! Agent-to-client requests are handled on their own task. A permission prompt
//! can sit unanswered for minutes while the user decides; blocking the reader
//! for that long would stall the event stream the user is reading.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use eavery_core::engine::EngineError;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

/// How many stderr lines to keep for a crash report. The plan asks for the last
/// 50 in the event; keeping more means the ring survives a burst of warnings
/// before the message that matters.
const STDERR_RING: usize = 200;

/// How long the waiter gives the stderr reader to finish after the child exits.
/// Without it a crash report races the drain and arrives empty, which is the
/// one moment stderr is worth having.
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(1);

pub const METHOD_NOT_FOUND: i64 = -32601;
/// The implementation-defined code Eavery uses for "refused by Eavery", such as
/// a write during the plan phase (`docs/plan/06-plan-gate-permissions.md` §2.2).
pub const REFUSED: i64 = -32000;

/// How to start an engine. `eavery-engines` (M1-T01) builds these from its
/// engine table; `eavery-acp` only runs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub engine_id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Extra environment for the child only. Eavery never writes these into an
    /// engine's own config (working rule 8).
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

impl LaunchSpec {
    pub fn new(engine_id: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self {
            engine_id: engine_id.into(),
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
}

/// Answers the requests an agent makes of its client. Implemented by
/// `AcpEngine`, which routes them to the permission handler of the turn in
/// flight and to the filesystem guard.
#[async_trait::async_trait]
pub trait ClientHandler: Send + Sync + 'static {
    /// The result is the JSON-RPC `result` for the request, or an error object.
    async fn handle(&self, method: &str, params: Value) -> Result<Value, HandlerError>;
}

#[derive(Debug, Clone)]
pub struct HandlerError {
    pub code: i64,
    pub message: String,
}

impl HandlerError {
    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            code: REFUSED,
            message: message.into(),
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: METHOD_NOT_FOUND,
            message: format!("no such method: {method}"),
        }
    }
}

/// The requests Eavery has sent and is still waiting on, keyed by JSON-RPC id.
type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcFailure>>>>>;

/// Where `session/update` notifications go. The engine sets this for the
/// duration of a turn and clears it afterwards; notifications arriving outside
/// a turn are logged and dropped.
pub type NotificationSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct Connection {
    engine_id: String,
    outgoing: mpsc::UnboundedSender<String>,
    pending: PendingRequests,
    next_id: AtomicU64,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// Asks the waiter task to end the child. Taken by the first
    /// [`Connection::shutdown`]; later calls find `None` and are no-ops.
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    /// Resolves once the child has actually gone.
    exited: Mutex<Option<oneshot::Receiver<()>>>,
}

/// Why a request did not produce a result.
#[derive(Debug, Clone)]
pub enum RpcFailure {
    /// The agent answered with a JSON-RPC error object.
    Error { code: i64, message: String },
    /// The connection went away before the answer came.
    Disconnected { reason: String },
}

impl Connection {
    /// Spawns the engine and starts the four tasks.
    pub fn spawn(
        spec: &LaunchSpec,
        handler: Arc<dyn ClientHandler>,
        notifications: NotificationSink,
    ) -> Result<Self, EngineError> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: without it every engine spawn flashes a console.
            command.creation_flags(0x0800_0000);
        }

        tracing::debug!(
            engine = %spec.engine_id,
            program = %spec.program.display(),
            args = ?spec.args,
            cwd = ?spec.cwd,
            "spawning engine"
        );

        let mut child = command.spawn().map_err(|source| EngineError::Spawn {
            engine_id: spec.engine_id.clone(),
            source,
        })?;

        let stdin = child.stdin.take().expect("stdin was piped");
        let stdout = child.stdout.take().expect("stdout was piped");
        let child_stderr = child.stderr.take().expect("stderr was piped");

        let (outgoing, outgoing_rx) = mpsc::unbounded_channel::<String>();
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_RING)));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (exited_tx, exited_rx) = oneshot::channel();

        let connection = Connection {
            engine_id: spec.engine_id.clone(),
            outgoing: outgoing.clone(),
            pending: Arc::clone(&pending),
            next_id: AtomicU64::new(1),
            stderr: Arc::clone(&stderr),
            shutdown: Mutex::new(Some(shutdown_tx)),
            exited: Mutex::new(Some(exited_rx)),
        };

        let (drained_tx, drained_rx) = oneshot::channel();

        tokio::spawn(write_loop(stdin, outgoing_rx, spec.engine_id.clone()));
        tokio::spawn(stderr_loop(
            child_stderr,
            Arc::clone(&stderr),
            spec.engine_id.clone(),
            drained_tx,
        ));
        tokio::spawn(read_loop(
            stdout,
            spec.engine_id.clone(),
            Arc::clone(&pending),
            outgoing,
            handler,
            notifications,
        ));
        tokio::spawn(wait_loop(WaitLoop {
            engine_id: spec.engine_id.clone(),
            child,
            shutdown: shutdown_rx,
            exited: exited_tx,
            drained: drained_rx,
            pending,
            stderr,
        }));

        Ok(connection)
    }

    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Sends a request and waits for its response.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.outgoing.send(message.to_string()).is_err() {
            self.pending.lock().await.remove(&id);
            return Err(self.crashed("the engine's input stream closed").await);
        }

        match rx.await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(RpcFailure::Error { code, message })) => Err(EngineError::Rpc {
                engine_id: self.engine_id.clone(),
                method: method.to_owned(),
                code,
                message,
            }),
            Ok(Err(RpcFailure::Disconnected { reason })) => Err(self.crashed(&reason).await),
            // The pending entry was dropped without an answer: the reader task
            // is gone, which means the process is too.
            Err(_) => Err(self.crashed("the engine stopped responding").await),
        }
    }

    /// The same, but gives up after `timeout`. Used for the handshake, never
    /// for a prompt: a turn legitimately takes minutes.
    pub async fn request_within(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, EngineError> {
        match tokio::time::timeout(timeout, self.request(method, params)).await {
            Ok(result) => result,
            Err(_) => Err(EngineError::Timeout {
                engine_id: self.engine_id.clone(),
                during: method.to_owned(),
                timeout_secs: timeout.as_secs(),
            }),
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        // A closed channel means the engine is already gone. Whatever this
        // notification was about no longer applies.
        let _ = self.outgoing.send(message.to_string());
    }

    pub async fn stderr_tail(&self) -> Vec<String> {
        self.stderr.lock().await.iter().cloned().collect()
    }

    async fn crashed(&self, reason: &str) -> EngineError {
        EngineError::Crashed {
            engine_id: self.engine_id.clone(),
            reason: reason.to_owned(),
            stderr_tail: self.stderr_tail().await,
        }
    }

    /// Ends the process and waits for it to go. Safe to call more than once:
    /// only the first call does anything, and every call returns once the child
    /// has actually exited.
    pub async fn shutdown(&self) {
        // The child is owned by the waiter task, which is already blocked on
        // `wait()`. Asking it to kill is what keeps `shutdown` from having to
        // take a lock that task is holding across an await.
        if let Some(shutdown) = self.shutdown.lock().await.take() {
            let _ = shutdown.send(());
        }
        if let Some(exited) = self.exited.lock().await.take() {
            let _ = exited.await;
        }
    }
}

async fn write_loop(
    mut stdin: tokio::process::ChildStdin,
    mut rx: mpsc::UnboundedReceiver<String>,
    engine_id: String,
) {
    while let Some(line) = rx.recv().await {
        tracing::trace!(engine = %engine_id, direction = "out", %line);
        if stdin.write_all(line.as_bytes()).await.is_err()
            || stdin.write_all(b"\n").await.is_err()
            || stdin.flush().await.is_err()
        {
            tracing::debug!(engine = %engine_id, "engine closed its stdin");
            return;
        }
    }
}

async fn stderr_loop(
    stderr: tokio::process::ChildStderr,
    ring: Arc<Mutex<VecDeque<String>>>,
    engine_id: String,
    drained: oneshot::Sender<()>,
) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::trace!(engine = %engine_id, direction = "stderr", %line);
        let mut ring = ring.lock().await;
        if ring.len() == STDERR_RING {
            ring.pop_front();
        }
        ring.push_back(line);
    }
    // Tells the waiter the pipe is empty, so a crash report is not assembled
    // before the lines that explain it have arrived.
    let _ = drained.send(());
}

async fn read_loop(
    stdout: tokio::process::ChildStdout,
    engine_id: String,
    pending: PendingRequests,
    outgoing: mpsc::UnboundedSender<String>,
    handler: Arc<dyn ClientHandler>,
    notifications: NotificationSink,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        tracing::trace!(engine = %engine_id, direction = "in", %line);

        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            // An engine that writes something other than JSON-RPC to stdout is
            // usually printing a warning it meant for stderr. Not fatal.
            tracing::warn!(engine = %engine_id, %line, "ignoring unparseable line from engine");
            continue;
        };

        let id = message.get("id").filter(|id| !id.is_null()).cloned();
        let method = message.get("method").and_then(Value::as_str);

        match (method, id) {
            // A request from the agent: handled off the reader so a slow answer
            // does not stall the stream.
            (Some(method), Some(id)) => {
                let method = method.to_owned();
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                let handler = Arc::clone(&handler);
                let outgoing = outgoing.clone();
                tokio::spawn(async move {
                    let reply = match handler.handle(&method, params).await {
                        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                        Err(error) => json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": {"code": error.code, "message": error.message}
                        }),
                    };
                    let _ = outgoing.send(reply.to_string());
                });
            }
            (Some(method), None) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                notifications(method, params);
            }
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else {
                    tracing::warn!(engine = %engine_id, ?id, "response with an id we never sent");
                    continue;
                };
                let Some(tx) = pending.lock().await.remove(&id) else {
                    tracing::warn!(engine = %engine_id, id, "response to an unknown request");
                    continue;
                };
                let outcome = match message.get("error") {
                    Some(error) => Err(RpcFailure::Error {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("(no message)")
                            .to_owned(),
                    }),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(outcome);
            }
            (None, None) => {
                tracing::warn!(engine = %engine_id, %line, "message with neither method nor id");
            }
        }
    }
    tracing::debug!(engine = %engine_id, "engine closed its stdout");
}

struct WaitLoop {
    engine_id: String,
    child: Child,
    shutdown: oneshot::Receiver<()>,
    exited: oneshot::Sender<()>,
    /// Fires when the stderr reader reaches EOF.
    drained: oneshot::Receiver<()>,
    pending: PendingRequests,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

/// Owns the child. It waits for the process to end, either on its own or
/// because [`Connection::shutdown`] asked, and then fails everything still
/// outstanding so a crash surfaces as an error rather than as a hang.
///
/// The child lives here rather than behind a mutex on `Connection` because
/// `wait()` is held across an await for the whole life of the process: a
/// shutdown that had to take that lock would wait for the exit it is trying to
/// cause.
async fn wait_loop(loop_state: WaitLoop) {
    let WaitLoop {
        engine_id,
        mut child,
        shutdown,
        exited,
        drained,
        pending,
        stderr,
    } = loop_state;

    let status = tokio::select! {
        status = child.wait() => status.ok(),
        _ = shutdown => {
            // Closing stdin is how an ACP agent is asked to leave; killing is
            // the fallback for one that will not.
            drop(child.stdin.take());
            let _ = child.kill().await;
            child.wait().await.ok()
        }
    };

    let reason = match status {
        Some(status) => format!("the engine exited with {status}"),
        None => "the engine exited".to_owned(),
    };
    // The child is gone but its stderr may still be sitting in a pipe buffer.
    // Whoever is about to be told the engine crashed wants those lines.
    let _ = tokio::time::timeout(STDERR_DRAIN_GRACE, drained).await;

    let tail = stderr.lock().await.iter().cloned().collect::<Vec<_>>();
    if !tail.is_empty() {
        tracing::warn!(engine = %engine_id, %reason, stderr = ?tail.last(), "engine exited");
    }

    let waiting = std::mem::take(&mut *pending.lock().await);
    for (_, tx) in waiting {
        let _ = tx.send(Err(RpcFailure::Disconnected {
            reason: reason.clone(),
        }));
    }

    // Releases whoever is waiting in `Connection::shutdown`.
    let _ = exited.send(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_specs_build_up_fluently() {
        let spec = LaunchSpec::new("goose", "/usr/local/bin/goose")
            .arg("acp")
            .env("GOOSE_PROVIDER", "ollama")
            .cwd("/tmp/project");
        assert_eq!(spec.engine_id, "goose");
        assert_eq!(spec.args, ["acp"]);
        assert_eq!(
            spec.env,
            [("GOOSE_PROVIDER".to_owned(), "ollama".to_owned())]
        );
        assert_eq!(spec.cwd, Some(PathBuf::from("/tmp/project")));
    }

    #[tokio::test]
    async fn spawning_something_that_does_not_exist_is_a_spawn_error() {
        struct Nothing;
        #[async_trait::async_trait]
        impl ClientHandler for Nothing {
            async fn handle(&self, method: &str, _: Value) -> Result<Value, HandlerError> {
                Err(HandlerError::method_not_found(method))
            }
        }

        let spec = LaunchSpec::new("nope", "eavery-no-such-program-anywhere");
        let result = Connection::spawn(&spec, Arc::new(Nothing), Arc::new(|_, _| {}));
        assert!(matches!(result, Err(EngineError::Spawn { .. })));
    }
}
