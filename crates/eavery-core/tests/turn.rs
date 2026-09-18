//! The turn state machine, driven by a scripted engine.
//!
//! The engine here is an in-process [`Engine`] rather than the fake agent
//! binary: `eavery-core` must not depend on `eavery-acp`
//! (`docs/plan/03-architecture.md` §1), and what these tests are about is the
//! state machine, not the wire. The same paths are exercised over real ACP by
//! the CLI's tests.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eavery_core::engine::{
    Engine, EngineError, EventSink, OpenedSession, PermissionHandler, RawAgentEvent, RawToolCall,
    RawToolCallUpdate, StopReason,
};
use eavery_core::event::{CoreEvent, DecidedBy, Decision, PermissionOption, PermissionView};
use eavery_core::journal::{Journal, Watch};
use eavery_core::model::{CheckpointKind, Project, RiskClass, TurnPhase};
use eavery_core::store::{Store, StoredEvent};
use eavery_core::turn::{ProjectRunner, TurnCallbacks, TurnError};
use futures::FutureExt;

// ---- the scripted engine ---------------------------------------------------

/// One thing the engine does during a prompt.
enum Act {
    Text(&'static str),
    Thought(&'static str),
    ToolCall(RawToolCall),
    Update(RawToolCallUpdate),
    /// Ask the client for permission, and record what it answered.
    Ask(PermissionView),
    /// Write a file directly, the way an engine with its own tools does.
    Write(&'static str, &'static str),
    /// End the prompt this way instead of `end_turn`.
    Stop(StopReason),
    /// Fail the prompt.
    Die,
}

#[derive(Default)]
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

struct TestEngine {
    scripts: Mutex<VecDeque<Vec<Act>>>,
    cwd: Mutex<Option<PathBuf>>,
    answers: Mutex<Vec<(String, Decision)>>,
    cancelled: Mutex<bool>,
    /// Held for the first prompt only: it waits at the gate so a test can do
    /// something else while a turn is running. Later prompts run straight
    /// through, or the test would be waiting on a gate nobody opens.
    gate: Mutex<Option<Arc<Gate>>>,
}

impl TestEngine {
    fn new(scripts: Vec<Vec<Act>>) -> Arc<Self> {
        Self::build(scripts, None)
    }

    fn gated(scripts: Vec<Vec<Act>>, gate: Arc<Gate>) -> Arc<Self> {
        Self::build(scripts, Some(gate))
    }

    fn build(scripts: Vec<Vec<Act>>, gate: Option<Arc<Gate>>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            cwd: Mutex::new(None),
            answers: Mutex::new(Vec::new()),
            cancelled: Mutex::new(false),
            gate: Mutex::new(gate),
        })
    }

    fn answers(&self) -> Vec<(String, Decision)> {
        self.answers.lock().unwrap().clone()
    }

    fn was_cancelled(&self) -> bool {
        *self.cancelled.lock().unwrap()
    }
}

#[async_trait::async_trait]
impl Engine for TestEngine {
    async fn start(&self) -> Result<eavery_core::model::EngineInfo, EngineError> {
        Ok(eavery_core::model::EngineInfo {
            engine_id: "test".into(),
            name: Some("test".into()),
            ..Default::default()
        })
    }

    async fn open_session(
        &self,
        cwd: &Path,
        _mcp: &[eavery_core::engine::McpServerSpec],
        _resume: Option<&str>,
    ) -> Result<OpenedSession, EngineError> {
        *self.cwd.lock().unwrap() = Some(cwd.to_path_buf());
        Ok(OpenedSession {
            session_id: "session-1".into(),
            modes: Vec::new(),
            current_mode: None,
        })
    }

    async fn set_mode(&self, _session: &str, _mode_id: &str) -> Result<(), EngineError> {
        Ok(())
    }

    async fn prompt(
        &self,
        _session: &str,
        _text: &str,
        tx: EventSink,
        permission: PermissionHandler,
    ) -> Result<StopReason, EngineError> {
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        let cwd = self.cwd.lock().unwrap().clone().expect("a session");

        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
            if self.was_cancelled() {
                return Ok(StopReason::Cancelled);
            }
        }

        for act in script {
            match act {
                Act::Text(text) => {
                    let _ = tx.send(RawAgentEvent::Text(text.into()));
                }
                Act::Thought(text) => {
                    let _ = tx.send(RawAgentEvent::Thought(text.into()));
                }
                Act::ToolCall(call) => {
                    let _ = tx.send(RawAgentEvent::ToolCall(call));
                }
                Act::Update(update) => {
                    let _ = tx.send(RawAgentEvent::ToolCallUpdate(update));
                }
                Act::Ask(view) => {
                    let title = view.title.clone();
                    let decision = permission(view).await;
                    self.answers.lock().unwrap().push((title, decision));
                }
                Act::Write(relative, text) => {
                    let path = cwd.join(relative);
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    std::fs::write(path, text).unwrap();
                }
                Act::Stop(reason) => return Ok(reason),
                Act::Die => {
                    return Err(EngineError::Crashed {
                        engine_id: "test".into(),
                        reason: "the engine stopped responding".into(),
                        stderr_tail: vec!["thread 'main' panicked".into()],
                    });
                }
            }
        }
        Ok(StopReason::EndTurn)
    }

    async fn cancel(&self, _session: &str) -> Result<(), EngineError> {
        *self.cancelled.lock().unwrap() = true;
        Ok(())
    }

    async fn stderr_tail(&self) -> Vec<String> {
        Vec::new()
    }

    async fn shutdown(&self) {}
}

// ---- the fixture -----------------------------------------------------------

/// What the outside world saw: every event, and every request the policy would
/// not answer on its own.
#[derive(Clone, Default)]
struct Seen {
    events: Arc<Mutex<Vec<StoredEvent>>>,
    asked: Arc<Mutex<Vec<PermissionView>>>,
}

impl Seen {
    fn events(&self) -> Vec<StoredEvent> {
        self.events.lock().unwrap().clone()
    }

    fn asked(&self) -> Vec<PermissionView> {
        self.asked.lock().unwrap().clone()
    }

    fn kinds(&self) -> Vec<String> {
        self.events()
            .iter()
            .map(|stored| {
                serde_json::to_value(&stored.event).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    fn callbacks(&self, answer: Decision) -> TurnCallbacks {
        let events = Arc::clone(&self.events);
        let asked = Arc::clone(&self.asked);
        TurnCallbacks {
            events: Arc::new(move |stored: &StoredEvent| {
                events.lock().unwrap().push(stored.clone())
            }),
            permission: Arc::new(move |view: PermissionView| {
                asked.lock().unwrap().push(view);
                std::future::ready(answer).boxed()
            }),
        }
    }
}

struct Fixture {
    store: Arc<Store>,
    journal: Arc<Journal>,
    project: Project,
    seen: Seen,
    _project_dir: tempfile::TempDir,
    _data_dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let project_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let root = eavery_core::paths::canonicalize(project_dir.path()).unwrap();
        std::fs::write(root.join("report.txt"), "FY25\n").unwrap();

        let store = Arc::new(Store::open_in_data_dir(data_dir.path()).unwrap());
        let project = Project {
            id: uuid::Uuid::new_v4(),
            name: "Month end".into(),
            root: root.clone(),
            created_at: chrono::Utc::now(),
            engine_id: None,
        };
        store.insert_project(&project).unwrap();

        let journal = Arc::new(
            Journal::open_or_create(project.id, &root, data_dir.path(), &Watch::default()).unwrap(),
        );

        Self {
            store,
            journal,
            project,
            seen: Seen::default(),
            _project_dir: project_dir,
            _data_dir: data_dir,
        }
    }

    fn root(&self) -> &Path {
        self.journal.root()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    async fn runner(&self, engine: Arc<TestEngine>, answer: Decision) -> Arc<ProjectRunner> {
        Arc::new(
            ProjectRunner::open(
                Arc::clone(&self.store),
                Arc::clone(&self.journal),
                engine,
                "test",
                &[],
                self.seen.callbacks(answer),
            )
            .await
            .expect("open the runner"),
        )
    }
}

fn edit_of(path: &Path) -> PermissionView {
    PermissionView {
        request_id: "r1".into(),
        tool_call_id: "c1".into(),
        title: format!("Edit {}", path.display()),
        kind: "edit".into(),
        locations: vec![path.display().to_string()],
        risk: RiskClass::Read,
        options: vec![
            PermissionOption {
                option_id: "a".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            },
            PermissionOption {
                option_id: "r".into(),
                name: "Reject".into(),
                kind: "reject_once".into(),
            },
        ],
        explanation: "edit".into(),
    }
}

fn command(title: &'static str) -> PermissionView {
    PermissionView {
        request_id: "r2".into(),
        tool_call_id: "c2".into(),
        title: title.into(),
        kind: "execute".into(),
        locations: vec![],
        risk: RiskClass::Read,
        ..edit_of(Path::new("unused"))
    }
}

// ---- the happy path --------------------------------------------------------

#[tokio::test]
async fn a_turn_is_bracketed_by_checkpoints_and_reports_what_it_changed() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Text("Changing the year."),
        Act::Write("report.txt", "FY26\n"),
        Act::Write("summary.txt", "new\n"),
    ]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let outcome = runner.run_turn("Rename FY25 to FY26").await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(outcome.turn.phase, TurnPhase::Done);
    let pre = outcome
        .turn
        .pre_checkpoint
        .clone()
        .expect("a pre checkpoint");
    let post = outcome
        .turn
        .post_checkpoint
        .clone()
        .expect("a post checkpoint");
    assert_ne!(pre, post, "the turn changed something, so it moved on");

    assert_eq!(outcome.digest.files_changed, vec!["report.txt"]);
    assert_eq!(outcome.digest.files_added, vec!["summary.txt"]);
    assert!(outcome.digest.files_removed.is_empty());
    assert_eq!(
        outcome.digest.undo_to.as_deref(),
        Some(pre.as_str()),
        "Undo goes back to the point before the turn"
    );

    // The checkpoints are in the store as well as in the Journal. The
    // pre-turn one is the point the folder was already at — nothing had
    // changed since it was opened, and the Journal answers with the existing
    // point rather than adding an identical one.
    let checkpoints = fixture
        .store
        .checkpoints_for_project(fixture.project.id, None)
        .unwrap();
    assert!(
        checkpoints.iter().any(|cp| cp.id == pre),
        "the point Undo goes back to is in the store"
    );
    assert!(
        checkpoints
            .iter()
            .any(|cp| cp.id == post && cp.kind == CheckpointKind::PostTurn),
        "and so is the one the turn made"
    );

    // And the turn is on the record as finished.
    let stored = fixture.store.turn(outcome.turn.id).unwrap().unwrap();
    assert_eq!(stored.phase, TurnPhase::Done);
    assert_eq!(stored.request, "Rename FY25 to FY26");
}

#[tokio::test]
async fn the_transcript_reads_in_the_order_it_happened() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Thought("Looking at the file"),
        Act::ToolCall(RawToolCall {
            id: "t1".into(),
            title: "Read report.txt".into(),
            kind: "read".into(),
            status: "in_progress".into(),
            locations: vec![],
            diff_paths: vec![],
            raw_input: None,
        }),
        Act::Update(RawToolCallUpdate {
            id: "t1".into(),
            status: Some("completed".into()),
            ..Default::default()
        }),
        Act::Text("Done."),
    ]]);
    let runner = fixture.runner(engine, Decision::RejectOnce).await;
    let outcome = runner.run_turn("Have a look").await.unwrap();

    let kinds = fixture.seen.kinds();
    assert_eq!(
        kinds,
        vec![
            "engine_status",
            "turn_started",
            "checkpoint_created",
            "agent_thought",
            "tool_call_started",
            "tool_call_updated",
            "agent_text",
            "checkpoint_created",
            "turn_finished",
        ]
    );

    // Nothing in this turn changed a file, so both checkpoint events name the
    // point the folder was already at — and that point belongs to no turn.
    // Everything the engine did belongs to this one.
    let stored = fixture
        .store
        .list_events(runner.session_id(), None, None)
        .unwrap();
    assert_eq!(stored.len(), kinds.len());
    for event in &stored {
        let kind = serde_json::to_value(&event.event).unwrap()["type"]
            .as_str()
            .unwrap()
            .to_owned();
        let expected = match kind.as_str() {
            "engine_status" | "checkpoint_created" => None,
            _ => Some(outcome.turn.id),
        };
        assert_eq!(event.turn_id, expected, "wrong turn on {kind}");
    }
}

/// A tool call that says nothing new must not lose what it said before: an
/// absent field in an update means unchanged.
#[tokio::test]
async fn a_tool_call_update_keeps_what_it_does_not_mention() {
    let fixture = Fixture::new();
    let report = fixture.path("report.txt");
    let engine = TestEngine::new(vec![vec![
        Act::ToolCall(RawToolCall {
            id: "t1".into(),
            title: "Edit report.txt".into(),
            kind: "edit".into(),
            status: "pending".into(),
            locations: vec![report.display().to_string()],
            diff_paths: vec![],
            raw_input: None,
        }),
        Act::Update(RawToolCallUpdate {
            id: "t1".into(),
            status: Some("completed".into()),
            diff_paths: Some(vec!["report.txt".into()]),
            ..Default::default()
        }),
    ]]);
    let runner = fixture.runner(engine, Decision::RejectOnce).await;
    runner.run_turn("Edit it").await.unwrap();

    let updated = fixture
        .seen
        .events()
        .into_iter()
        .find_map(|stored| match stored.event {
            CoreEvent::ToolCallUpdated { call, .. } => Some(call),
            _ => None,
        })
        .expect("a tool call update");

    assert_eq!(updated.title, "Edit report.txt");
    assert_eq!(updated.kind, "edit");
    assert_eq!(updated.status, "completed");
    assert_eq!(updated.locations, vec![report.display().to_string()]);
    assert_eq!(updated.risk, RiskClass::Reversible);
    assert_eq!(
        updated.diff_summary.as_deref(),
        Some("changes in report.txt")
    );
}

// ---- permissions -----------------------------------------------------------

/// What the pre-turn checkpoint makes reversible goes through without a
/// question. Asking about it would be theatre, and a person asked about
/// everything stops reading.
#[tokio::test]
async fn an_edit_inside_the_project_is_allowed_without_asking() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Ask(edit_of(&fixture.path("report.txt"))),
        Act::Write("report.txt", "FY26\n"),
    ]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;
    runner.run_turn("Rename the year").await.unwrap();

    assert_eq!(engine.answers()[0].1, Decision::AllowOnce);
    assert!(
        fixture.seen.asked().is_empty(),
        "nobody was asked about a reversible edit"
    );

    // Silent still means written down.
    let audit = fixture.store.list_audit(None, None).unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor, DecidedBy::Policy);
    assert_eq!(audit[0].action, "allow_once");
    assert_eq!(audit[0].risk, Some(RiskClass::Reversible));

    let resolved = fixture
        .seen
        .events()
        .into_iter()
        .find_map(|stored| match stored.event {
            CoreEvent::PermissionResolved { by, decision, .. } => Some((by, decision)),
            _ => None,
        })
        .expect("a resolved permission");
    assert_eq!(resolved, (DecidedBy::Policy, Decision::AllowOnce));
}

/// Undo cannot reach outside the Project, so nothing out there is reversible,
/// whatever the engine's own layer guessed.
#[tokio::test]
async fn an_edit_outside_the_project_is_asked_about() {
    let fixture = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    let engine = TestEngine::new(vec![vec![Act::Ask(edit_of(
        &outside.path().join("other.txt"),
    ))]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;
    runner.run_turn("Edit something else").await.unwrap();

    assert_eq!(engine.answers()[0].1, Decision::RejectOnce);
    let asked = fixture.seen.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].risk,
        RiskClass::Destructive,
        "the request the user sees is classified by Eavery, not by the engine's layer"
    );

    let audit = fixture.store.list_audit(None, None).unwrap();
    assert_eq!(audit[0].actor, DecidedBy::User);
    assert_eq!(audit[0].action, "reject_once");
}

#[tokio::test]
async fn running_a_command_is_the_users_call_and_a_refusal_is_reported() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![Act::Ask(command("Run the backup script"))]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;
    let outcome = runner.run_turn("Back it up").await.unwrap();

    assert_eq!(fixture.seen.asked().len(), 1);
    assert_eq!(
        outcome.digest.refused_actions,
        vec!["Run the backup script"]
    );
    assert!(outcome.digest.outbound_actions.is_empty());
}

/// What left the machine is always part of the digest, so the user can see it
/// even when they said yes in the moment.
#[tokio::test]
async fn what_leaves_the_machine_is_listed_in_the_digest() {
    let fixture = Fixture::new();
    let mut fetch = command("Send the report to example.com");
    fetch.kind = "fetch".into();
    let engine = TestEngine::new(vec![vec![Act::Ask(fetch)]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::AllowOnce)
        .await;
    let outcome = runner.run_turn("Send it").await.unwrap();

    assert_eq!(fixture.seen.asked().len(), 1, "outbound is never silent");
    assert_eq!(
        outcome.digest.outbound_actions,
        vec!["Send the report to example.com"]
    );
    assert!(outcome.digest.refused_actions.is_empty());
}

// ---- cancel, crash, and the guards ----------------------------------------

/// A cancelled turn still gets its post-turn checkpoint: it changed files
/// before it stopped, and those changes have to be undoable.
#[tokio::test]
async fn a_cancelled_turn_is_still_checkpointed_and_still_undoable() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Write("report.txt", "half done\n"),
        Act::Stop(StopReason::Cancelled),
    ]]);
    let runner = fixture.runner(engine, Decision::RejectOnce).await;
    let outcome = runner.run_turn("Rename the year").await.unwrap();

    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert_eq!(outcome.turn.phase, TurnPhase::Cancelled);
    assert!(outcome.turn.post_checkpoint.is_some());
    assert_eq!(outcome.digest.files_changed, vec!["report.txt"]);
    assert_eq!(
        fixture.store.turn(outcome.turn.id).unwrap().unwrap().phase,
        TurnPhase::Cancelled
    );
}

/// Stop, pressed while the engine is working: the request reaches the engine,
/// and the turn comes back as cancelled rather than as a failure.
#[tokio::test]
async fn stopping_a_running_turn_reaches_the_engine() {
    let fixture = Fixture::new();
    let gate = Arc::new(Gate::default());
    let engine = TestEngine::gated(
        vec![vec![Act::Write("report.txt", "half done\n")]],
        Arc::clone(&gate),
    );
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let running = {
        let runner = Arc::clone(&runner);
        tokio::spawn(async move { runner.run_turn("Rename the year").await })
    };
    gate.entered.notified().await;

    runner.cancel().await.unwrap();
    gate.release.notify_one();

    let outcome = running.await.unwrap().unwrap();
    assert!(engine.was_cancelled(), "the engine was told to stop");
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert_eq!(outcome.turn.phase, TurnPhase::Cancelled);
    assert!(outcome.turn.post_checkpoint.is_some());
}

/// With no turn running there is nothing to stop, which is not a failure.
#[tokio::test]
async fn stopping_when_nothing_is_running_does_nothing() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    runner.cancel().await.unwrap();
    assert!(!engine.was_cancelled());
}

#[tokio::test]
async fn an_engine_that_dies_leaves_the_work_it_did_protected() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Write("report.txt", "half done\n"),
        Act::Die,
    ]]);
    let runner = fixture.runner(engine, Decision::RejectOnce).await;

    let error = runner
        .run_turn("Rename the year")
        .await
        .expect_err("it died");
    assert!(matches!(error, TurnError::Engine(_)), "{error}");

    let kinds = fixture.seen.kinds();
    assert!(kinds.contains(&"engine_crashed".to_owned()));
    assert_eq!(
        kinds.last().unwrap(),
        "turn_finished",
        "a turn that failed still finishes, with what it managed to do"
    );

    let finished = fixture
        .seen
        .events()
        .into_iter()
        .find_map(|stored| match stored.event {
            CoreEvent::TurnFinished { digest, .. } => digest,
            _ => None,
        })
        .expect("a digest");
    assert_eq!(finished.files_changed, vec!["report.txt"]);
    assert!(finished.undo_to.is_some(), "the half-done work is undoable");
}

/// C13: one turn per Project. The second request is refused rather than
/// queued — two engines writing the same folder is not something the Journal
/// can make sense of afterwards.
#[tokio::test]
async fn a_second_turn_while_one_is_running_is_refused() {
    let fixture = Fixture::new();
    let gate = Arc::new(Gate::default());
    let engine = TestEngine::gated(
        vec![vec![Act::Write("report.txt", "FY26\n")]],
        Arc::clone(&gate),
    );
    let runner = fixture.runner(engine, Decision::RejectOnce).await;

    let running = {
        let runner = Arc::clone(&runner);
        tokio::spawn(async move { runner.run_turn("Rename the year").await })
    };
    gate.entered.notified().await;

    let refused = runner.run_turn("And again").await.expect_err("refused");
    assert!(
        matches!(
            refused,
            TurnError::Busy {
                turn_id: Some(_),
                ..
            }
        ),
        "{refused}"
    );
    assert_eq!(
        refused.code(),
        eavery_core::event::ErrorCode::TurnAlreadyRunning
    );
    assert!(refused.next_action().is_some());
    assert!(runner.running_turn().is_some());

    gate.release.notify_one();
    running.await.unwrap().unwrap();

    assert!(runner.running_turn().is_none(), "the Project is free again");
    runner.run_turn("Now it is my turn").await.unwrap();
}

/// Going back while the engine is working would have it write over the
/// restored files on its way out.
#[tokio::test]
async fn going_back_while_a_turn_is_running_is_refused() {
    let fixture = Fixture::new();
    let gate = Arc::new(Gate::default());
    let engine = TestEngine::gated(vec![vec![Act::Text("working")]], Arc::clone(&gate));
    let runner = fixture.runner(engine, Decision::RejectOnce).await;
    let first = runner
        .checkpoint_now("A point to go back to")
        .await
        .unwrap();

    let running = {
        let runner = Arc::clone(&runner);
        tokio::spawn(async move { runner.run_turn("Rename the year").await })
    };
    gate.entered.notified().await;

    let refused = runner.restore(&first.id).await.expect_err("refused");
    assert!(matches!(refused, TurnError::Busy { .. }), "{refused}");

    gate.release.notify_one();
    running.await.unwrap().unwrap();
    runner.restore(&first.id).await.expect("free now");
}

// ---- going back ------------------------------------------------------------

#[tokio::test]
async fn going_back_puts_the_files_back_and_says_so() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Write("report.txt", "FY26\n"),
        Act::Write("summary.txt", "new\n"),
    ]]);
    let runner = fixture.runner(engine, Decision::RejectOnce).await;
    let outcome = runner.run_turn("Rename the year").await.unwrap();
    let undo_to = outcome.digest.undo_to.clone().unwrap();

    assert_eq!(
        std::fs::read_to_string(fixture.path("report.txt")).unwrap(),
        "FY26\n"
    );

    let restored_to = runner.restore(&undo_to).await.unwrap();
    assert!(restored_to.skipped_locked.is_empty());
    assert_eq!(restored_to.checkpoint.kind, CheckpointKind::Restore);
    assert_eq!(
        std::fs::read_to_string(fixture.path("report.txt")).unwrap(),
        "FY25\n",
        "the file is back as it was before the turn"
    );
    assert!(
        !fixture.path("summary.txt").exists(),
        "and the file the turn added is gone"
    );

    let restored = fixture
        .seen
        .events()
        .into_iter()
        .find_map(|stored| match stored.event {
            CoreEvent::Restored { to, .. } => Some(to),
            _ => None,
        })
        .expect("a restored event");
    assert_eq!(restored, undo_to);

    // The restore's own checkpoints reach the store, so the Checkpoints panel
    // shows the way back from here too.
    let cached = fixture
        .store
        .checkpoints_for_project(fixture.project.id, None)
        .unwrap();
    assert!(cached.iter().any(|cp| cp.id == restored_to.checkpoint.id));
    assert!(
        cached
            .iter()
            .filter(|cp| cp.kind == CheckpointKind::Manual)
            .count()
            >= 1,
        "including the one taken before going back"
    );
}

/// History only moves forward: an edit Eavery never saw is committed before
/// the restore writes over it, so it can be reached again.
#[tokio::test]
async fn an_edit_eavery_never_saw_survives_going_back() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![Act::Write("report.txt", "FY26\n")]]);
    let runner = fixture.runner(engine, Decision::RejectOnce).await;
    let outcome = runner.run_turn("Rename the year").await.unwrap();

    std::fs::write(fixture.path("mine.txt"), "my own note\n").unwrap();
    runner
        .restore(&outcome.digest.undo_to.clone().unwrap())
        .await
        .unwrap();

    let checkpoints = runner.sync_checkpoints(20).await.unwrap();
    let before_going_back = checkpoints
        .iter()
        .find(|cp| cp.label == "Before going back")
        .expect("the checkpoint that preserved the work tree");

    let changes = fixture
        .journal
        .diff(
            &outcome.turn.post_checkpoint.clone().unwrap(),
            &before_going_back.id,
        )
        .unwrap();
    assert_eq!(
        changes.added,
        vec![PathBuf::from("mine.txt")],
        "the note the user made is in the history, not lost to the restore"
    );
}

// ---- when the folder cannot be protected -----------------------------------

/// Working rule 7: no checkpoint, no turn. A folder that could not be
/// protected is a folder nothing is allowed to touch.
#[tokio::test]
async fn a_turn_does_not_run_when_the_folder_cannot_be_protected() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![Act::Write("report.txt", "FY26\n")]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    // The folder goes away between opening the Project and the request.
    std::fs::remove_dir_all(fixture.root()).unwrap();

    let error = runner
        .run_turn("Rename the year")
        .await
        .expect_err("nothing to protect");
    assert!(matches!(error, TurnError::Checkpoint { .. }), "{error}");
    assert_eq!(
        error.code(),
        eavery_core::event::ErrorCode::CheckpointFailed
    );
    assert!(error.next_action().is_some(), "the user is told what to do");

    assert!(
        engine.answers().is_empty(),
        "the engine was never asked to do anything"
    );
    let kinds = fixture.seen.kinds();
    assert!(kinds.contains(&"error".to_owned()));
    assert!(!kinds.contains(&"agent_text".to_owned()));
    assert!(runner.running_turn().is_none(), "and the Project is free");
}
