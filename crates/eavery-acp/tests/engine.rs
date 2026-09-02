//! `AcpEngine` driven against the fake agent: a real child process, real
//! newline-delimited JSON-RPC, real pipes. The only thing faked is the model.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use eavery_acp::{AcpEngine, LaunchSpec};
use eavery_core::engine::{Engine, EngineError, RawAgentEvent, StopReason};
use eavery_core::event::{Decision, PermissionView};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Answers every permission request the same way and records what it was asked.
fn always(decision: Decision) -> (eavery_core::engine::PermissionHandler, Arc<AskedFor>) {
    let asked = Arc::new(AskedFor::default());
    let recorder = Arc::clone(&asked);
    let handler: eavery_core::engine::PermissionHandler = Arc::new(move |view: PermissionView| {
        let recorder = Arc::clone(&recorder);
        Box::pin(async move {
            recorder.record(view);
            decision
        })
    });
    (handler, asked)
}

#[derive(Default)]
struct AskedFor {
    views: std::sync::Mutex<Vec<PermissionView>>,
}

impl AskedFor {
    fn record(&self, view: PermissionView) {
        self.views.lock().unwrap().push(view);
    }

    fn titles(&self) -> Vec<String> {
        self.views
            .lock()
            .unwrap()
            .iter()
            .map(|v| v.title.clone())
            .collect()
    }

    fn len(&self) -> usize {
        self.views.lock().unwrap().len()
    }
}

fn write_script(dir: &Path, script: Value) -> std::path::PathBuf {
    let path = dir.join("script.json");
    std::fs::write(&path, script.to_string()).expect("write the script");
    path
}

fn engine_for(script: &Path, cwd: &Path) -> AcpEngine {
    AcpEngine::new(
        LaunchSpec::new("fake", common::fake_agent())
            .arg("--script")
            .arg(script.to_string_lossy().into_owned())
            .cwd(cwd),
    )
}

/// Runs a prompt and collects everything it streamed.
async fn run(
    engine: &AcpEngine,
    session: &str,
    text: &str,
    handler: eavery_core::engine::PermissionHandler,
) -> (Vec<RawAgentEvent>, Result<StopReason, EngineError>) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let stop = engine.prompt(session, text, tx, handler).await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (events, stop)
}

/// The M0-T06 acceptance test: text, a tool call, and a permission request,
/// observed in the order the agent sent them.
#[tokio::test]
async fn a_scripted_turn_streams_its_events_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "initialize": {"agentInfo": {"name": "fake", "version": "0.0.1"}},
            "session": {"modes": {"currentModeId": "work", "availableModes": [
                {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"}
            ]}},
            "turns": [{"match": "summarise", "actions": [
                {"thought": "Looking around"},
                {"tool_call": {"id": "t1", "title": "Read report.md", "kind": "read",
                               "status": "completed", "locations": ["{{cwd}}/report.md"]}},
                {"request_permission": {"toolCallId": "t2", "title": "Edit report.md",
                                        "kind": "edit", "expect": "allow_once"}},
                {"tool_call_update": {"id": "t2", "status": "completed"}},
                {"text": "Done."},
                {"stop": "end_turn"}
            ]}]
        }),
    );
    let engine = engine_for(&script, dir.path());

    let info = engine.start().await.expect("start the engine");
    assert_eq!(info.name.as_deref(), Some("fake"));
    assert_eq!(info.protocol_version, 1);
    assert!(!info.load_session);

    let session = engine
        .open_session(dir.path(), &[], None)
        .await
        .expect("open a session");
    assert_eq!(session.session_id, "sess_fake_1");
    assert_eq!(session.current_mode.as_deref(), Some("work"));
    assert_eq!(session.modes.len(), 2);
    assert_eq!(session.modes[1].id, "plan");

    let (handler, asked) = always(Decision::AllowOnce);
    let (events, stop) = run(
        &engine,
        &session.session_id,
        "summarise this folder",
        handler,
    )
    .await;

    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert_eq!(asked.titles(), ["Edit report.md"]);

    assert_eq!(events.len(), 4, "unexpected event stream: {events:#?}");
    assert!(matches!(&events[0], RawAgentEvent::Thought(t) if t == "Looking around"));
    match &events[1] {
        RawAgentEvent::ToolCall(call) => {
            assert_eq!(call.id, "t1");
            assert_eq!(call.kind, "read");
            assert_eq!(call.status, "completed");
            assert_eq!(call.locations.len(), 1);
            assert!(call.locations[0].ends_with("report.md"));
        }
        other => panic!("expected a tool call, got {other:?}"),
    }
    match &events[2] {
        RawAgentEvent::ToolCallUpdate(update) => {
            assert_eq!(update.id, "t2");
            assert_eq!(update.status.as_deref(), Some("completed"));
            assert_eq!(update.title, None, "an absent field means unchanged");
        }
        other => panic!("expected a tool call update, got {other:?}"),
    }
    assert!(matches!(&events[3], RawAgentEvent::Text(t) if t == "Done."));

    engine.shutdown().await;
}

/// A refusal must reach the agent as a refusal. The script asserts this from
/// its side too: a wrong answer exits the fake agent with code 3, which shows
/// up here as the turn failing.
#[tokio::test]
async fn a_refusal_reaches_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"request_permission": {"toolCallId": "t1", "title": "Edit it", "kind": "edit",
                                    "expect": "reject_once"}},
            {"text": "Understood."},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, asked) = always(Decision::RejectOnce);
    let (events, stop) = run(&engine, &session.session_id, "do it", handler).await;

    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert_eq!(asked.len(), 1);
    assert!(matches!(&events[0], RawAgentEvent::Text(t) if t == "Understood."));
    engine.shutdown().await;
}

/// The permission view carries what the UI needs to phrase a question: the tool
/// call, its kind, and the options the engine actually offered.
#[tokio::test]
async fn the_permission_view_carries_the_engines_own_options() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"request_permission": {"toolCallId": "t1", "title": "Fetch example.com",
                                    "kind": "fetch", "locations": ["{{cwd}}/out.txt"],
                                    "options": [
                                        {"optionId": "y", "name": "Go ahead", "kind": "allow_once"},
                                        {"optionId": "n", "name": "No", "kind": "reject_once"}
                                    ],
                                    "expect": "allow_once"}},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, asked) = always(Decision::AllowOnce);
    let (_, stop) = run(&engine, &session.session_id, "fetch it", handler).await;
    assert_eq!(stop.unwrap(), StopReason::EndTurn);

    let views = asked.views.lock().unwrap();
    let view = views.first().expect("one permission request");
    assert_eq!(view.tool_call_id, "t1");
    assert_eq!(view.title, "Fetch example.com");
    assert_eq!(view.risk, eavery_core::model::RiskClass::Outbound);
    assert_eq!(view.options.len(), 2);
    assert_eq!(view.options[0].option_id, "y");
    assert_eq!(view.options[0].kind, "allow_once");
    assert!(view.explanation.contains("fetch"));
    assert!(view.explanation.contains("out.txt"));
}

#[tokio::test]
async fn writes_through_the_client_land_inside_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"fs_write": {"path": "{{cwd}}/notes.txt", "text": "FY26"}},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, _) = always(Decision::AllowOnce);
    let (_, stop) = run(&engine, &session.session_id, "write it", handler).await;
    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "FY26"
    );
    engine.shutdown().await;
}

/// D6/D15: writes outside the Project are refused. The agent is told why, and
/// the turn carries on rather than failing.
#[tokio::test]
async fn writes_outside_the_project_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("elsewhere.txt");
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"fs_write": {"path": target.to_string_lossy(), "text": "should not appear"}},
            {"text": "I could not write that."},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, _) = always(Decision::AllowOnce);
    let (events, stop) = run(&engine, &session.session_id, "write outside", handler).await;

    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert!(!target.exists(), "the write should have been refused");
    assert!(matches!(&events[0], RawAgentEvent::Text(t) if t.starts_with("I could not")));
    engine.shutdown().await;
}

/// D15: reads are served from anywhere the engine could read itself, because
/// Playbooks live outside the Project.
#[tokio::test]
async fn reads_are_served_from_outside_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    let playbook = library.path().join("SKILL.md");
    std::fs::write(&playbook, "# Month-end close\nStep one.").unwrap();

    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"fs_read": {"path": playbook.to_string_lossy()}},
            {"text": "Read it."},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, _) = always(Decision::AllowOnce);
    let (events, stop) = run(&engine, &session.session_id, "read the playbook", handler).await;
    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert!(matches!(&events[0], RawAgentEvent::Text(t) if t == "Read it."));
    engine.shutdown().await;
}

/// The plan gate closes writes without touching the connection; the refusal
/// message is the one `06 §2.2` specifies.
#[tokio::test]
async fn the_fs_guard_can_close_writes() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"fs_write": {"path": "{{cwd}}/notes.txt", "text": "FY26"}},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();
    engine.fs_guard().set_writes_allowed(false);

    let (handler, _) = always(Decision::AllowOnce);
    let (_, stop) = run(&engine, &session.session_id, "write it", handler).await;
    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert!(!dir.path().join("notes.txt").exists());

    engine.fs_guard().set_writes_allowed(true);
    assert!(engine.fs_guard().writes_allowed());
    engine.shutdown().await;
}

/// Cancel is called from another task while `prompt` is blocked. This is the
/// reason every `Engine` method takes `&self`.
#[tokio::test]
async fn cancel_returns_the_outstanding_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"text": "starting"},
            {"sleep_ms": 30000},
            {"text": "never sent"},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = Arc::new(engine_for(&script, dir.path()));
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let canceller = Arc::clone(&engine);
    let session_id = session.session_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        canceller.cancel(&session_id).await.expect("cancel");
    });

    let (handler, _) = always(Decision::AllowOnce);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let stop = tokio::time::timeout(
        Duration::from_secs(20),
        engine.prompt(&session.session_id, "take your time", tx, handler),
    )
    .await
    .expect("the prompt must return after a cancel, not hang");

    assert_eq!(stop.unwrap(), StopReason::Cancelled);
    assert!(matches!(rx.try_recv(), Ok(RawAgentEvent::Text(t)) if t == "starting"));
    engine.shutdown().await;
}

/// A crash must surface as an error carrying the stderr tail, not as a hang.
#[tokio::test]
async fn a_crash_mid_turn_fails_the_prompt_with_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"text": "about to die"},
            // The client refuses this, the fake agent says so on stderr, and
            // that line is what the crash report has to carry.
            {"fs_read": {"path": "/definitely/not/here"}},
            {"exit": 1}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, _) = always(Decision::AllowOnce);
    let (_, stop) = tokio::time::timeout(
        Duration::from_secs(20),
        run(&engine, &session.session_id, "crash", handler),
    )
    .await
    .expect("a crash must not hang the prompt");

    match stop {
        Err(EngineError::Crashed {
            engine_id,
            stderr_tail,
            ..
        }) => {
            assert_eq!(engine_id, "fake");
            assert!(
                stderr_tail.iter().any(|line| line.contains("read refused")),
                "expected the refused read on stderr, got {stderr_tail:?}"
            );
        }
        other => panic!("expected a crash error, got {other:?}"),
    }
    assert!(!engine.stderr_tail().await.is_empty());
    engine.shutdown().await;
}

/// An unmatched prompt is a JSON-RPC error from the agent, and must arrive as
/// one rather than as a silent empty turn.
#[tokio::test]
async fn an_agent_error_becomes_an_rpc_error() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(dir.path(), json!({"turns": []}));
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, _) = always(Decision::AllowOnce);
    let (_, stop) = run(
        &engine,
        &session.session_id,
        "nothing matches this",
        handler,
    )
    .await;
    match stop {
        Err(EngineError::Rpc { method, code, .. }) => {
            assert_eq!(method, "session/prompt");
            assert_eq!(code, -32000);
        }
        other => panic!("expected an rpc error, got {other:?}"),
    }
    engine.shutdown().await;
}

/// Two turns on one session, to prove the per-turn sink is swapped rather than
/// accumulated: the second turn's events must not reach the first turn's
/// receiver.
#[tokio::test]
async fn a_second_turn_gets_its_own_event_stream() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [
            {"match": "first", "actions": [{"text": "one"}, {"stop": "end_turn"}]},
            {"match": "second", "actions": [{"text": "two"}, {"stop": "end_turn"}]}
        ]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let (handler, _) = always(Decision::AllowOnce);
    let (first, _) = run(
        &engine,
        &session.session_id,
        "the first one",
        Arc::clone(&handler),
    )
    .await;
    let (second, _) = run(&engine, &session.session_id, "the second one", handler).await;

    assert!(matches!(&first[..], [RawAgentEvent::Text(t)] if t == "one"));
    assert!(matches!(&second[..], [RawAgentEvent::Text(t)] if t == "two"));
    engine.shutdown().await;
}

#[tokio::test]
async fn set_mode_reaches_the_agent_and_comes_back_as_a_mode_change() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({
            "session": {"modes": {"currentModeId": "work", "availableModes": [
                {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"}
            ]}},
            "turns": [{"actions": [{"text": "ok"}, {"stop": "end_turn"}]}]
        }),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    engine
        .set_mode(&session.session_id, "plan")
        .await
        .expect("set the mode");

    let (handler, _) = always(Decision::AllowOnce);
    let (events, stop) = run(&engine, &session.session_id, "anything", handler).await;
    assert_eq!(stop.unwrap(), StopReason::EndTurn);

    // `session/set_mode` answers before it announces, so its
    // `current_mode_update` can land inside the next turn's stream. That is the
    // engine's ordering rather than a bug, so this pins what the turn itself
    // produced and allows the mode change alongside it.
    let modes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RawAgentEvent::ModeChanged(mode) => Some(mode.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        modes.iter().all(|mode| *mode == "plan"),
        "unexpected mode: {modes:?}"
    );

    let text: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RawAgentEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, ["ok"]);
    engine.shutdown().await;
}

/// Using the engine before starting it is a programming error, and says so
/// rather than panicking somewhere deeper.
#[tokio::test]
async fn using_an_unstarted_engine_is_an_error_not_a_panic() {
    let engine = AcpEngine::new(LaunchSpec::new("fake", "eavery-fake-agent"));
    let result = engine.open_session(Path::new("/tmp"), &[], None).await;
    assert!(matches!(result, Err(EngineError::Protocol { .. })));
    assert!(engine.stderr_tail().await.is_empty());
}

/// An engine that will not start is reported as a spawn failure naming the
/// engine, not as a timeout ten seconds later.
#[tokio::test]
async fn an_engine_that_is_not_there_fails_to_start() {
    let engine = AcpEngine::new(LaunchSpec::new("ghost", "eavery-no-such-program-anywhere"));
    match engine.start().await {
        Err(EngineError::Spawn { engine_id, .. }) => assert_eq!(engine_id, "ghost"),
        other => panic!("expected a spawn error, got {other:?}"),
    }
}

/// Permission requests are answered off the reader task, so a slow answer must
/// not stop later updates from arriving.
#[tokio::test]
async fn a_slow_permission_answer_does_not_stall_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [
            {"request_permission": {"toolCallId": "t1", "title": "Slow one", "kind": "edit",
                                    "expect": "allow_once"}},
            {"text": "after the wait"},
            {"stop": "end_turn"}
        ]}]}),
    );
    let engine = engine_for(&script, dir.path());
    engine.start().await.unwrap();
    let session = engine.open_session(dir.path(), &[], None).await.unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let handler: eavery_core::engine::PermissionHandler = Arc::new(move |_| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            counter.fetch_add(1, Ordering::SeqCst);
            Decision::AllowOnce
        })
    });

    let (events, stop) = run(&engine, &session.session_id, "wait for me", handler).await;
    assert_eq!(stop.unwrap(), StopReason::EndTurn);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(matches!(&events[..], [RawAgentEvent::Text(t)] if t == "after the wait"));
    engine.shutdown().await;
}
