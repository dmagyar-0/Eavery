//! A scriptable ACP agent, the primary test double for everything above it
//! (`docs/plan/11-testing-ci.md` §1).
//!
//! It speaks hand-rolled newline-delimited JSON-RPC over stdio and replays a
//! JSON script. It never calls a model and never guesses: an unmatched prompt
//! is a JSON-RPC error, and a client that answers a permission request
//! differently from what the script expects exits the process with code 3.
#![deny(unsafe_code)]

mod rpc;
mod script;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::{Value, json};

use rpc::Message;
use script::{Action, PermissionAction, Script, ScriptRunner, ToolCallAction};

/// Exit code for "the client did not behave as the script expected". Distinct
/// from a crash so a test can tell a policy regression from a bug.
const EXPECTATION_FAILED: u8 = 3;

/// How long an action waits for the client to answer before giving up. Real
/// clients answer in milliseconds; this only exists so a hung test fails
/// rather than hangs.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

const PROTOCOL_VERSION: u16 = 1;

#[derive(Parser, Debug)]
#[command(
    name = "eavery-fake-agent",
    about = "A scriptable ACP agent for Eavery's tests.",
    version
)]
struct Args {
    /// The JSON script to replay. Without one the agent answers `initialize`
    /// and `session/new` and refuses every prompt, which is enough for a
    /// shallow health check.
    #[arg(long)]
    script: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let script = match load_script(args.script.as_deref()) {
        Ok(script) => script,
        Err(err) => {
            eprintln!("eavery-fake-agent: {err:#}");
            return ExitCode::from(2);
        }
    };

    match Agent::new(script).run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("eavery-fake-agent: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn load_script(path: Option<&std::path::Path>) -> Result<Script> {
    let Some(path) = path else {
        return Ok(Script::default());
    };
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading script {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing script {}", path.display()))
}

/// What the stdin reader hands to the main loop.
enum FromClient {
    Message(Message),
    /// A line that was not a JSON-RPC message. Reported on stderr, then ignored:
    /// a client's framing bug should not take the agent down mid-test.
    Malformed(String),
    Eof,
}

struct Agent {
    runner: ScriptRunner,
    rx: Receiver<FromClient>,
    next_id: u64,
    session_id: String,
    cwd: String,
    /// Set by `session/cancel`; cleared when the turn it cancelled ends.
    cancelled: bool,
    /// Set when the client answered a permission request differently from the
    /// script's `expect`. The process exits with [`EXPECTATION_FAILED`] once
    /// the turn has been answered, so the client sees a reply rather than a
    /// closed pipe.
    expectation_failed: bool,
}

impl Agent {
    fn new(script: Script) -> Self {
        let session_id = script
            .session
            .session_id
            .clone()
            .unwrap_or_else(|| "sess_fake_1".to_owned());
        Self {
            runner: ScriptRunner::new(script),
            rx: spawn_stdin_reader(),
            next_id: 1,
            session_id,
            cwd: String::new(),
            cancelled: false,
            expectation_failed: false,
        }
    }

    fn run(&mut self) -> Result<ExitCode> {
        loop {
            match self.rx.recv() {
                Ok(FromClient::Eof) | Err(_) => return Ok(ExitCode::SUCCESS),
                Ok(FromClient::Malformed(line)) => {
                    eprintln!("eavery-fake-agent: ignoring unparseable line: {line}");
                }
                Ok(FromClient::Message(message)) => {
                    if let Some(code) = self.handle(message)? {
                        return Ok(code);
                    }
                }
            }
        }
    }

    /// Handles one client message. `Some(code)` means stop the process.
    fn handle(&mut self, message: Message) -> Result<Option<ExitCode>> {
        match message {
            Message::Request { id, method, params } => {
                match method.as_str() {
                    "initialize" => {
                        let reply = self.initialize_response();
                        self.send(rpc::ok_response(&id, reply));
                        Ok(None)
                    }
                    "session/new" => {
                        self.cwd = params
                            .get("cwd")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.send(rpc::ok_response(&id, self.session_response()));
                        Ok(None)
                    }
                    "session/load" => {
                        if !self.runner.script().initialize.load_session {
                            self.send(rpc::error_response(
                                &id,
                                rpc::METHOD_NOT_FOUND,
                                "this agent did not advertise loadSession",
                            ));
                        } else {
                            self.send(rpc::ok_response(&id, json!({})));
                        }
                        Ok(None)
                    }
                    "session/set_mode" => {
                        let mode = params
                            .get("modeId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.send(rpc::ok_response(&id, json!({})));
                        self.update(json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }));
                        Ok(None)
                    }
                    "session/prompt" => self.run_turn(&id, &params),
                    other => {
                        self.send(rpc::error_response(
                            &id,
                            rpc::METHOD_NOT_FOUND,
                            &format!("no such method: {other}"),
                        ));
                        Ok(None)
                    }
                }
            }
            Message::Notification { method, .. } => {
                if method == "session/cancel" {
                    self.cancelled = true;
                }
                Ok(None)
            }
            // A response with no request outstanding. Nothing to do with it.
            Message::Response { .. } => Ok(None),
        }
    }

    fn initialize_response(&self) -> Value {
        let init = &self.runner.script().initialize;
        let mut reply = json!({
            "protocolVersion": init.protocol_version.unwrap_or(PROTOCOL_VERSION),
            "agentCapabilities": { "loadSession": init.load_session },
            "authMethods": init.auth_methods,
        });
        if let Some(info) = &init.agent_info {
            reply["agentInfo"] = info.clone();
        }
        reply
    }

    fn session_response(&self) -> Value {
        let mut reply = json!({ "sessionId": self.session_id });
        if let Some(modes) = &self.runner.script().session.modes {
            reply["modes"] = script::substitute_value(modes, &self.cwd);
        }
        reply
    }

    /// Replays the turn matching this prompt, then answers `session/prompt`.
    fn run_turn(&mut self, id: &Value, params: &Value) -> Result<Option<ExitCode>> {
        self.cancelled = false;
        let prompt = prompt_text(params);

        let Some(turn) = self.runner.take_turn(&prompt) else {
            self.send(rpc::error_response(
                id,
                rpc::REFUSED,
                &format!("no scripted turn matches this prompt: {prompt}"),
            ));
            return Ok(None);
        };

        let mut stop_reason = "end_turn".to_owned();
        for action in &turn.actions {
            if self.cancelled {
                stop_reason = "cancelled".to_owned();
                break;
            }
            match self.perform(action)? {
                Flow::Continue => {}
                Flow::Stop(reason) => {
                    stop_reason = reason;
                    break;
                }
                // `exit` simulates a crash: no reply, the pipe just closes.
                Flow::Exit(code) => return Ok(Some(ExitCode::from(code))),
            }
        }
        if self.cancelled {
            stop_reason = "cancelled".to_owned();
        }

        self.send(rpc::ok_response(id, json!({ "stopReason": stop_reason })));

        if self.expectation_failed {
            return Ok(Some(ExitCode::from(EXPECTATION_FAILED)));
        }
        Ok(None)
    }

    fn perform(&mut self, action: &Action) -> Result<Flow> {
        let cwd = self.cwd.clone();
        match action {
            Action::Text(text) => {
                self.update(content_chunk(
                    "agent_message_chunk",
                    &script::substitute(text, &cwd),
                ));
            }
            Action::Thought(text) => {
                self.update(content_chunk(
                    "agent_thought_chunk",
                    &script::substitute(text, &cwd),
                ));
            }
            Action::ToolCall(call) => {
                self.update(tool_call_update(call, &cwd, true));
            }
            Action::ToolCallUpdate(call) => {
                self.update(tool_call_update(call, &cwd, false));
            }
            Action::Plan(plan) => {
                self.update(plan_update(plan, &cwd));
            }
            Action::Mode(mode) => {
                self.update(
                    json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }),
                );
            }
            Action::RequestPermission(permission) => {
                return self.request_permission(permission, &cwd);
            }
            Action::FsWrite(file) => {
                let params = json!({
                    "sessionId": self.session_id,
                    "path": script::substitute(&file.path, &cwd),
                    "content": script::substitute(&file.text, &cwd),
                });
                // A refused write is a legitimate outcome (the plan gate says
                // no), so it is reported and the turn goes on.
                if let Err(err) = self.call("fs/write_text_file", params)? {
                    eprintln!("eavery-fake-agent: write refused: {}", err.message);
                }
            }
            Action::FsRead(read) => {
                let params = json!({
                    "sessionId": self.session_id,
                    "path": script::substitute(&read.path, &cwd),
                });
                let outcome = self.call("fs/read_text_file", params)?;
                // Symmetric with fs_write: a refused read is reported and the
                // turn goes on, so a test can see why the client said no.
                if let Err(err) = &outcome {
                    eprintln!("eavery-fake-agent: read refused: {}", err.message);
                }
                if read.expect_refused && outcome.is_ok() {
                    eprintln!(
                        "eavery-fake-agent: expected the client to refuse reading {}, but it did not",
                        read.path
                    );
                    self.expectation_failed = true;
                }
            }
            Action::WriteDirect(file) => {
                let path = script::substitute(&file.path, &cwd);
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&path, script::substitute(&file.text, &cwd))
                    .with_context(|| format!("write_direct to {path}"))?;
            }
            Action::SleepMs(ms) => self.sleep(Duration::from_millis(*ms)),
            Action::Exit(code) => return Ok(Flow::Exit(*code as u8)),
            Action::Stop(reason) => return Ok(Flow::Stop(reason.clone())),
        }
        Ok(Flow::Continue)
    }

    fn request_permission(&mut self, permission: &PermissionAction, cwd: &str) -> Result<Flow> {
        let options = permission
            .options
            .clone()
            .map(Value::Array)
            .unwrap_or_else(standard_options);

        let params = json!({
            "sessionId": self.session_id,
            "toolCall": {
                "toolCallId": permission.tool_call_id,
                "title": script::substitute(&permission.title, cwd),
                "kind": permission.kind.clone().unwrap_or_else(|| "other".to_owned()),
                "locations": locations(permission.locations.as_deref(), cwd),
            },
            "options": script::substitute_value(&options, cwd),
        });

        let answered = match self.call("session/request_permission", params)? {
            Ok(result) => option_kind(&result, &options),
            Err(err) => {
                eprintln!(
                    "eavery-fake-agent: permission request failed: {}",
                    err.message
                );
                "error".to_owned()
            }
        };

        if let Some(expected) = &permission.expect
            && &answered != expected
        {
            eprintln!(
                "eavery-fake-agent: expected the client to answer {expected} for {:?}, got {answered}",
                permission.title
            );
            self.expectation_failed = true;
        }
        Ok(Flow::Continue)
    }

    /// Sends a request to the client and waits for its response, staying
    /// responsive to `session/cancel` while it waits.
    fn call(&mut self, method: &str, params: Value) -> Result<Result<Value, rpc::RpcError>> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(rpc::request(id, method, params));

        let deadline = std::time::Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("the client did not answer {method} within {RESPONSE_TIMEOUT:?}");
            }
            match self.rx.recv_timeout(remaining) {
                Ok(FromClient::Message(Message::Response { id: got, outcome }))
                    if got == json!(id) =>
                {
                    return Ok(outcome);
                }
                Ok(FromClient::Message(message)) => self.handle_while_busy(message),
                Ok(FromClient::Malformed(line)) => {
                    eprintln!("eavery-fake-agent: ignoring unparseable line: {line}");
                }
                Ok(FromClient::Eof) | Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("the client went away while {method} was outstanding");
                }
                Err(RecvTimeoutError::Timeout) => {
                    anyhow::bail!("the client did not answer {method} within {RESPONSE_TIMEOUT:?}");
                }
            }
        }
    }

    /// A message that arrived while a request was outstanding. Only a cancel is
    /// meaningful here; a second prompt is a client bug and is refused loudly.
    fn handle_while_busy(&mut self, message: Message) {
        match message {
            Message::Notification { method, .. } if method == "session/cancel" => {
                self.cancelled = true;
            }
            Message::Request { id, method, .. } => {
                self.send(rpc::error_response(
                    &id,
                    rpc::REFUSED,
                    &format!("{method} arrived while a turn was still running"),
                ));
            }
            _ => {}
        }
    }

    /// Sleeps, but wakes for a cancel so a cancel test does not wait out the
    /// whole delay.
    fn sleep(&mut self, duration: Duration) {
        let deadline = std::time::Instant::now() + duration;
        while !self.cancelled {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(FromClient::Message(message)) => self.handle_while_busy(message),
                Ok(FromClient::Malformed(_)) => {}
                Ok(FromClient::Eof) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => return,
            }
        }
    }

    fn update(&self, update: Value) {
        self.send(rpc::notification(
            "session/update",
            json!({ "sessionId": self.session_id, "update": update }),
        ));
    }

    fn send(&self, message: Value) {
        let mut stdout = std::io::stdout().lock();
        // A closed pipe means the client is gone; the reader thread will
        // report EOF and the process will end. Nothing useful to say here.
        let _ = writeln!(stdout, "{message}");
        let _ = stdout.flush();
    }
}

enum Flow {
    Continue,
    Stop(String),
    Exit(u8),
}

fn spawn_stdin_reader() -> Receiver<FromClient> {
    let (tx, rx): (Sender<FromClient>, Receiver<FromClient>) = channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let message = match rpc::parse_line(&line) {
                Ok(Some(message)) => FromClient::Message(message),
                Ok(None) => continue,
                Err(_) => FromClient::Malformed(line),
            };
            if tx.send(message).is_err() {
                return;
            }
        }
        let _ = tx.send(FromClient::Eof);
    });
    rx
}

/// The text of a prompt, joining every text block. Non-text blocks are ignored:
/// scripts match on words.
fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn content_chunk(kind: &str, text: &str) -> Value {
    json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
}

fn locations(paths: Option<&[String]>, cwd: &str) -> Value {
    Value::Array(
        paths
            .unwrap_or_default()
            .iter()
            .map(|p| json!({ "path": script::substitute(p, cwd) }))
            .collect(),
    )
}

/// Builds a `tool_call` or `tool_call_update`. A `tool_call` carries defaults
/// for the fields ACP requires; an update carries only what the script set,
/// because in ACP an absent field means unchanged, not cleared.
fn tool_call_update(call: &ToolCallAction, cwd: &str, is_new: bool) -> Value {
    let mut update = json!({
        "sessionUpdate": if is_new { "tool_call" } else { "tool_call_update" },
        "toolCallId": call.id,
    });

    let mut set = |key: &str, value: Value| {
        update[key] = value;
    };
    match (&call.title, is_new) {
        (Some(title), _) => set("title", json!(script::substitute(title, cwd))),
        (None, true) => set("title", json!("")),
        (None, false) => {}
    }
    match (&call.kind, is_new) {
        (Some(kind), _) => set("kind", json!(kind)),
        (None, true) => set("kind", json!("other")),
        (None, false) => {}
    }
    match (&call.status, is_new) {
        (Some(status), _) => set("status", json!(status)),
        (None, true) => set("status", json!("pending")),
        (None, false) => {}
    }
    if let Some(paths) = &call.locations {
        set("locations", locations(Some(paths), cwd));
    }
    if let Some(content) = &call.content {
        set(
            "content",
            script::substitute_value(&Value::Array(content.clone()), cwd),
        );
    }
    if let Some(raw) = &call.raw_input {
        set("rawInput", script::substitute_value(raw, cwd));
    }
    if let Some(raw) = &call.raw_output {
        set("rawOutput", script::substitute_value(raw, cwd));
    }
    update
}

/// Accepts either a bare array of entries or `{"entries": [...]}`, and fills in
/// the `priority` and `status` that ACP requires but a test author forgets.
fn plan_update(plan: &Value, cwd: &str) -> Value {
    let entries = match plan {
        Value::Array(entries) => entries.clone(),
        other => other
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    let entries: Vec<Value> = entries
        .into_iter()
        .map(|entry| {
            let mut entry = script::substitute_value(&entry, cwd);
            if entry.get("priority").is_none() {
                entry["priority"] = json!("medium");
            }
            if entry.get("status").is_none() {
                entry["status"] = json!("pending");
            }
            entry
        })
        .collect();
    json!({ "sessionUpdate": "plan", "entries": entries })
}

/// The three options every ACP agent offers (`docs/plan/04-acp-engines.md` §1).
fn standard_options() -> Value {
    json!([
        { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
        { "optionId": "allow_always", "name": "Always allow", "kind": "allow_always" },
        { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
    ])
}

/// Maps a permission response back to the `kind` of the option the client
/// picked, which is what a script's `expect` names.
fn option_kind(result: &Value, options: &Value) -> String {
    let outcome = &result["outcome"];
    match outcome["outcome"].as_str() {
        Some("selected") => {
            let picked = outcome["optionId"].as_str().unwrap_or_default();
            options
                .as_array()
                .and_then(|options| {
                    options
                        .iter()
                        .find(|o| o["optionId"] == picked)
                        .and_then(|o| o["kind"].as_str())
                })
                .unwrap_or("unknown")
                .to_owned()
        }
        Some("cancelled") => "cancelled".to_owned(),
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text_joins_text_blocks_and_ignores_the_rest() {
        let params = json!({"prompt": [
            {"type": "text", "text": "first"},
            {"type": "image", "data": "..."},
            {"type": "text", "text": "second"},
        ]});
        assert_eq!(prompt_text(&params), "first\nsecond");
        assert_eq!(prompt_text(&json!({})), "");
    }

    #[test]
    fn a_new_tool_call_carries_the_fields_acp_requires() {
        let call = ToolCallAction {
            id: "t1".into(),
            ..ToolCallAction::default()
        };
        let update = tool_call_update(&call, "/p", true);
        assert_eq!(update["sessionUpdate"], "tool_call");
        assert_eq!(update["toolCallId"], "t1");
        assert_eq!(update["kind"], "other");
        assert_eq!(update["status"], "pending");
        assert_eq!(update["title"], "");
    }

    /// In ACP an absent field on an update means unchanged. Defaulting them
    /// here would silently reset a completed call to pending.
    #[test]
    fn an_update_carries_only_what_the_script_set() {
        let call = ToolCallAction {
            id: "t1".into(),
            status: Some("completed".into()),
            ..ToolCallAction::default()
        };
        let update = tool_call_update(&call, "/p", false);
        assert_eq!(update["sessionUpdate"], "tool_call_update");
        assert_eq!(update["status"], "completed");
        assert!(update.get("kind").is_none());
        assert!(update.get("title").is_none());
    }

    #[test]
    fn tool_call_locations_become_acp_location_objects() {
        let call = ToolCallAction {
            id: "t1".into(),
            locations: Some(vec!["{{cwd}}/report.docx".into()]),
            ..ToolCallAction::default()
        };
        let update = tool_call_update(&call, "/tmp/p", true);
        assert_eq!(update["locations"][0]["path"], "/tmp/p/report.docx");
    }

    #[test]
    fn plan_entries_are_filled_in_both_shapes() {
        let bare = plan_update(&json!([{"content": "Step one"}]), "/p");
        assert_eq!(bare["entries"][0]["content"], "Step one");
        assert_eq!(bare["entries"][0]["priority"], "medium");
        assert_eq!(bare["entries"][0]["status"], "pending");

        let wrapped = plan_update(
            &json!({"entries": [{"content": "Step one", "status": "completed"}]}),
            "/p",
        );
        assert_eq!(wrapped["entries"][0]["status"], "completed");
    }

    #[test]
    fn permission_answers_map_back_to_the_option_kind() {
        let options = standard_options();
        let selected = json!({"outcome": {"outcome": "selected", "optionId": "reject"}});
        assert_eq!(option_kind(&selected, &options), "reject_once");

        let always = json!({"outcome": {"outcome": "selected", "optionId": "allow_always"}});
        assert_eq!(option_kind(&always, &options), "allow_always");

        let cancelled = json!({"outcome": {"outcome": "cancelled"}});
        assert_eq!(option_kind(&cancelled, &options), "cancelled");

        // An option the agent never offered is not quietly treated as an allow.
        let bogus = json!({"outcome": {"outcome": "selected", "optionId": "nope"}});
        assert_eq!(option_kind(&bogus, &options), "unknown");
    }

    #[test]
    fn content_chunks_have_the_acp_shape() {
        let chunk = content_chunk("agent_message_chunk", "hello");
        assert_eq!(chunk["sessionUpdate"], "agent_message_chunk");
        assert_eq!(chunk["content"]["type"], "text");
        assert_eq!(chunk["content"]["text"], "hello");
    }
}
