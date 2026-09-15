//! Drives the fake agent the way a client does: spawn it, write
//! newline-delimited JSON-RPC to its stdin, read it back from its stdout.
//!
//! This is the "`printf ... | fake-agent`" check from M0-T05, written down so
//! it runs in CI on all three platforms instead of living in someone's shell
//! history.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct FakeAgent {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl FakeAgent {
    fn spawn(script: Option<&std::path::Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eavery-fake-agent"));
        if let Some(script) = script {
            command.arg("--script").arg(script);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn the fake agent");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").expect("write to the agent");
        self.stdin.flush().expect("flush");
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .expect("read from the agent");
        assert!(read > 0, "the agent closed its stdout unexpectedly");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("agent wrote non-JSON {line:?}: {e}"))
    }

    /// Reads until a response to `id` arrives, collecting everything else.
    fn recv_until_response(&mut self, id: u64) -> (Vec<Value>, Value) {
        let mut others = Vec::new();
        loop {
            let message = self.recv();
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return (others, message);
            }
            others.push(message);
        }
    }

    fn finish(mut self) -> Option<i32> {
        drop(self.stdin);
        self.child.wait().expect("wait for the agent").code()
    }
}

fn initialize(agent: &mut FakeAgent) -> Value {
    agent.send(json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": true, "writeTextFile": true},
                "terminal": false
            }
        }
    }));
    agent.recv()
}

fn new_session(agent: &mut FakeAgent, cwd: &str) -> Value {
    agent.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "session/new",
        "params": { "cwd": cwd, "mcpServers": [] }
    }));
    agent.recv()
}

fn prompt(agent: &mut FakeAgent, id: u64, session: &str, text: &str) {
    agent.send(json!({
        "jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": { "sessionId": session, "prompt": [{"type": "text", "text": text}] }
    }));
}

fn write_script(dir: &std::path::Path, script: Value) -> std::path::PathBuf {
    let path = dir.join("script.json");
    std::fs::write(&path, script.to_string()).expect("write the script");
    path
}

#[test]
fn it_answers_initialize_and_session_new_without_a_script() {
    let mut agent = FakeAgent::spawn(None);

    let init = initialize(&mut agent);
    assert_eq!(init["result"]["protocolVersion"], 1);
    assert_eq!(init["result"]["agentCapabilities"]["loadSession"], false);

    let session = new_session(&mut agent, "/tmp");
    assert_eq!(session["result"]["sessionId"], "sess_fake_1");
    // No script, so there is nothing to say and it says so rather than
    // pretending the turn succeeded.
    prompt(&mut agent, 2, "sess_fake_1", "anything");
    let reply = agent.recv();
    assert_eq!(reply["error"]["code"], -32000);

    assert_eq!(agent.finish(), Some(0));
}

#[test]
fn it_echoes_a_scripted_text_reply() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"} },
            "turns": [{ "match": "hello", "actions": [
                {"text": "Hello back."},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));

    let init = initialize(&mut agent);
    assert_eq!(init["result"]["agentInfo"]["name"], "fake");
    new_session(&mut agent, "/tmp");

    prompt(&mut agent, 2, "sess_fake_1", "hello there");
    let (updates, response) = agent.recv_until_response(2);

    assert_eq!(
        updates.len(),
        1,
        "expected exactly one session/update: {updates:?}"
    );
    assert_eq!(updates[0]["method"], "session/update");
    assert_eq!(
        updates[0]["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(
        updates[0]["params"]["update"]["content"]["text"],
        "Hello back."
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");

    assert_eq!(agent.finish(), Some(0));
}

#[test]
fn it_reports_modes_and_accepts_a_mode_change() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "session": { "modes": { "currentModeId": "work", "availableModes": [
                {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"}
            ]}},
            "turns": []
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);

    let session = new_session(&mut agent, "/tmp");
    assert_eq!(session["result"]["modes"]["currentModeId"], "work");
    assert_eq!(
        session["result"]["modes"]["availableModes"][1]["id"],
        "plan"
    );

    agent.send(json!({
        "jsonrpc": "2.0", "id": 2, "method": "session/set_mode",
        "params": { "sessionId": "sess_fake_1", "modeId": "plan" }
    }));
    let (_, ok) = agent.recv_until_response(2);
    assert!(ok["result"].is_object());
    let update = agent.recv();
    assert_eq!(
        update["params"]["update"]["sessionUpdate"],
        "current_mode_update"
    );
    assert_eq!(update["params"]["update"]["currentModeId"], "plan");

    assert_eq!(agent.finish(), Some(0));
}

/// The full plan-phase shape: a thought, a tool call, a permission request the
/// client refuses, and a plan block. This is the script from
/// `docs/plan/11-testing-ci.md` §2, and the client here plays the plan gate.
#[test]
fn it_runs_a_scripted_turn_with_a_permission_request() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().replace('\\', "/");
    let script = write_script(
        dir.path(),
        json!({
            "turns": [{ "match": "plan", "actions": [
                {"thought": "Looking around"},
                {"tool_call": {"id": "t1", "title": "Read report.md", "kind": "read",
                               "status": "completed", "locations": ["{{cwd}}/report.md"]}},
                {"request_permission": {"toolCallId": "t2", "title": "Edit report.md",
                                        "kind": "edit", "expect": "reject_once"}},
                {"text": "I would change one number."},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, &cwd);
    prompt(&mut agent, 2, "sess_fake_1", "make a plan");

    let mut seen = Vec::new();
    let response = loop {
        let message = agent.recv();
        if message["method"] == "session/request_permission" {
            let options = message["params"]["options"].as_array().unwrap();
            let reject = options.iter().find(|o| o["kind"] == "reject_once").unwrap();
            let id = message["id"].clone();
            agent.send(json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"outcome": {"outcome": "selected", "optionId": reject["optionId"]}}
            }));
            seen.push("permission".to_owned());
            continue;
        }
        if message.get("method").is_none() {
            break message;
        }
        seen.push(
            message["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap_or("?")
                .to_owned(),
        );
    };

    assert_eq!(
        seen,
        [
            "agent_thought_chunk",
            "tool_call",
            "permission",
            "agent_message_chunk"
        ]
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    // The script expected reject_once and got it, so the agent exits cleanly.
    assert_eq!(agent.finish(), Some(0));
}

/// A client that answers differently from the script must fail the test loudly
/// rather than quietly proving nothing.
#[test]
fn a_wrong_permission_answer_exits_with_code_three() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "turns": [{ "actions": [
                {"request_permission": {"toolCallId": "t1", "title": "Edit it",
                                        "kind": "edit", "expect": "reject_once"}},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, "/tmp");
    prompt(&mut agent, 2, "sess_fake_1", "do it");

    let request = agent.recv();
    assert_eq!(request["method"], "session/request_permission");
    agent.send(json!({
        "jsonrpc": "2.0", "id": request["id"],
        "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}
    }));

    // The turn is still answered, so the client is never left hanging.
    let (_, response) = agent.recv_until_response(2);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(agent.finish(), Some(3));
}

#[test]
fn it_writes_through_the_clients_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().replace('\\', "/");
    let script = write_script(
        dir.path(),
        json!({
            "turns": [{ "actions": [
                {"fs_write": {"path": "{{cwd}}/notes.txt", "text": "FY26"}},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, &cwd);
    prompt(&mut agent, 2, "sess_fake_1", "write it");

    let request = agent.recv();
    assert_eq!(request["method"], "fs/write_text_file");
    assert_eq!(request["params"]["path"], format!("{cwd}/notes.txt"));
    assert_eq!(request["params"]["content"], "FY26");
    agent.send(json!({"jsonrpc": "2.0", "id": request["id"], "result": {}}));

    let (_, response) = agent.recv_until_response(2);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(agent.finish(), Some(0));
}

/// `write_direct` is how a test simulates an engine that edits files with its
/// own tools instead of asking the client. The Journal has to cope with that.
#[test]
fn write_direct_bypasses_the_client() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().replace('\\', "/");
    let script = write_script(
        dir.path(),
        json!({
            "turns": [{ "actions": [
                {"write_direct": {"path": "{{cwd}}/sneaky.txt", "text": "written by the agent"}},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, &cwd);
    prompt(&mut agent, 2, "sess_fake_1", "write it");

    let (_, response) = agent.recv_until_response(2);
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("sneaky.txt")).unwrap(),
        "written by the agent"
    );
    assert_eq!(agent.finish(), Some(0));
}

#[test]
fn a_cancel_stops_the_turn_and_the_prompt_still_returns() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "turns": [{ "actions": [
                {"text": "starting"},
                {"sleep_ms": 30000},
                {"text": "this should never be sent"},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, "/tmp");
    prompt(&mut agent, 2, "sess_fake_1", "take your time");

    let first = agent.recv();
    assert_eq!(first["params"]["update"]["content"]["text"], "starting");

    agent.send(json!({
        "jsonrpc": "2.0", "method": "session/cancel",
        "params": {"sessionId": "sess_fake_1"}
    }));

    let (updates, response) = agent.recv_until_response(2);
    assert!(
        updates.is_empty(),
        "the turn kept going after cancel: {updates:?}"
    );
    assert_eq!(response["result"]["stopReason"], "cancelled");
    assert_eq!(agent.finish(), Some(0));
}

#[test]
fn exit_simulates_a_crash_mid_turn() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "turns": [{ "actions": [{"text": "about to die"}, {"exit": 1}] }]
        }),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, "/tmp");
    prompt(&mut agent, 2, "sess_fake_1", "crash");

    let update = agent.recv();
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "about to die"
    );
    assert_eq!(agent.finish(), Some(1));
}

/// Unparseable input is a client bug, not a reason to take the agent down in
/// the middle of a test that has not made its point yet.
#[test]
fn junk_on_stdin_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [{"text": "still here"}, {"stop": "end_turn"}]}]}),
    );
    let mut agent = FakeAgent::spawn(Some(&script));
    initialize(&mut agent);
    new_session(&mut agent, "/tmp");

    agent.send(json!("not a message"));
    writeln!(agent.stdin, "{{ not even json").unwrap();
    agent.send(json!({}));

    prompt(&mut agent, 2, "sess_fake_1", "anything");
    let (updates, response) = agent.recv_until_response(2);
    assert_eq!(
        updates[0]["params"]["update"]["content"]["text"],
        "still here"
    );
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert_eq!(agent.finish(), Some(0));
}
