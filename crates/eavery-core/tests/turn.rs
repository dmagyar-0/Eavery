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
    Engine, EngineError, EngineFacts, EventSink, OpenedSession, PermissionHandler, RawAgentEvent,
    RawToolCall, RawToolCallUpdate, StopReason,
};
use eavery_core::event::{CoreEvent, DecidedBy, Decision, PermissionOption, PermissionView};
use eavery_core::journal::{Journal, Watch};
use eavery_core::model::{CheckpointKind, Project, RiskClass, SessionMode, TurnPhase};
use eavery_core::policy::{AlwaysOffer, ConnectorRegistry};
use eavery_core::store::{Store, StoredEvent};
use eavery_core::turn::{
    Approval, PLAN_REJECTED, PlanReview, ProjectRunner, TurnCallbacks, TurnError, TurnMode,
};
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
    /// The modes `session/new` offers, and the one it opens in.
    modes: Vec<SessionMode>,
    current_mode: Option<String>,
    /// Every prompt text, every `set_mode`, and every flip of the write
    /// gate, in order: what the plan gate is made of, seen from the engine.
    prompts: Mutex<Vec<String>>,
    modes_set: Mutex<Vec<String>>,
    writes_allowed: Mutex<Vec<bool>>,
}

impl TestEngine {
    fn new(scripts: Vec<Vec<Act>>) -> Arc<Self> {
        Arc::new(Self::build(scripts, None))
    }

    fn gated(scripts: Vec<Vec<Act>>, gate: Arc<Gate>) -> Arc<Self> {
        Arc::new(Self::build(scripts, Some(gate)))
    }

    /// An engine that offers modes, the way Claude Code and Codex do.
    fn with_modes(scripts: Vec<Vec<Act>>, modes: &[&str], current: &str) -> Arc<Self> {
        let mut engine = Self::build(scripts, None);
        engine.modes = modes
            .iter()
            .map(|id| SessionMode {
                id: (*id).to_owned(),
                name: (*id).to_owned(),
                description: None,
            })
            .collect();
        engine.current_mode = Some(current.to_owned());
        Arc::new(engine)
    }

    fn build(scripts: Vec<Vec<Act>>, gate: Option<Arc<Gate>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            cwd: Mutex::new(None),
            answers: Mutex::new(Vec::new()),
            cancelled: Mutex::new(false),
            gate: Mutex::new(gate),
            modes: Vec::new(),
            current_mode: None,
            prompts: Mutex::new(Vec::new()),
            modes_set: Mutex::new(Vec::new()),
            writes_allowed: Mutex::new(Vec::new()),
        }
    }

    fn answers(&self) -> Vec<(String, Decision)> {
        self.answers.lock().unwrap().clone()
    }

    fn was_cancelled(&self) -> bool {
        *self.cancelled.lock().unwrap()
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn modes_set(&self) -> Vec<String> {
        self.modes_set.lock().unwrap().clone()
    }

    fn writes_allowed(&self) -> Vec<bool> {
        self.writes_allowed.lock().unwrap().clone()
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
            modes: self.modes.clone(),
            current_mode: self.current_mode.clone(),
        })
    }

    async fn set_mode(&self, _session: &str, mode_id: &str) -> Result<(), EngineError> {
        self.modes_set.lock().unwrap().push(mode_id.to_owned());
        Ok(())
    }

    async fn set_writes_allowed(&self, allowed: bool) {
        self.writes_allowed.lock().unwrap().push(allowed);
    }

    async fn prompt(
        &self,
        _session: &str,
        text: &str,
        tx: EventSink,
        permission: PermissionHandler,
    ) -> Result<StopReason, EngineError> {
        self.prompts.lock().unwrap().push(text.to_owned());
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
    /// Every plan the person was shown.
    reviewed: Arc<Mutex<Vec<PlanReview>>>,
    /// What the person answers a plan with. `None` never answers: the test
    /// has to cancel the turn.
    approval: Arc<Mutex<Option<Approval>>>,
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

    fn reviewed(&self) -> Vec<PlanReview> {
        self.reviewed.lock().unwrap().clone()
    }

    fn will_answer_plan(&self, approval: Option<Approval>) {
        *self.approval.lock().unwrap() = approval;
    }

    fn callbacks(&self, answer: Decision) -> TurnCallbacks {
        let events = Arc::clone(&self.events);
        let asked = Arc::clone(&self.asked);
        let reviewed = Arc::clone(&self.reviewed);
        let approval = Arc::clone(&self.approval);
        TurnCallbacks {
            events: Arc::new(move |stored: &StoredEvent| {
                events.lock().unwrap().push(stored.clone())
            }),
            permission: Arc::new(move |view: PermissionView| {
                asked.lock().unwrap().push(view);
                std::future::ready(answer).boxed()
            }),
            approval: Arc::new(move |review: PlanReview| {
                reviewed.lock().unwrap().push(review);
                let answer = approval.lock().unwrap().clone();
                async move {
                    match answer {
                        Some(approval) => approval,
                        // Nobody answers. The turn waits until it is cancelled.
                        None => std::future::pending().await,
                    }
                }
                .boxed()
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

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative)).expect("read the file")
    }

    async fn runner(&self, engine: Arc<TestEngine>, answer: Decision) -> Arc<ProjectRunner> {
        self.runner_with(
            engine,
            answer,
            &EngineFacts::default(),
            &ConnectorRegistry::default(),
        )
        .await
    }

    async fn runner_with(
        &self,
        engine: Arc<TestEngine>,
        answer: Decision,
        facts: &EngineFacts,
        connectors: &ConnectorRegistry,
    ) -> Arc<ProjectRunner> {
        Arc::new(
            ProjectRunner::open(
                Arc::clone(&self.store),
                Arc::clone(&self.journal),
                engine,
                "test",
                facts,
                connectors,
                self.seen.callbacks(answer),
            )
            .await
            .expect("open the runner"),
        )
    }
}

/// The facts the engine table gives a planning engine: a plan mode, a work
/// mode, and a way of leaving plan mode the gate has to refuse.
fn planning_facts() -> EngineFacts {
    EngineFacts {
        vendor: "Anthropic".into(),
        plan_mode_hint: Some("plan".into()),
        asking_mode_hint: Some("default".into()),
        plan_exit_signatures: vec!["ExitPlanMode".into()],
    }
}

/// The reply the plan prompt asks for (`06-plan-gate-permissions.md` §2.3).
const PLAN_REPLY: &str = "I will update the report.\n\n```eavery-plan\n{\"summary\":\"Update the report\",\"steps\":[\"Open report.txt\",\"Change FY25 to FY26\"],\"files_touched\":[\"report.txt\"],\"outbound\":[\"Send the report to example.com\"],\"irreversible\":[],\"will_not_do\":[\"send any email\"]}\n```";

fn read_of(title: &'static str) -> PermissionView {
    PermissionView {
        request_id: format!("r-{title}"),
        tool_call_id: format!("c-{title}"),
        title: title.into(),
        kind: "read".into(),
        locations: vec![],
        risk: RiskClass::Read,
        ..edit_of(Path::new("unused"))
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
        raw_input: None,
        always: AlwaysOffer::Never,
        in_plan: None,
        connector: None,
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

/// §3.3: "always" for a command, given in Developer mode, is remembered per
/// Project. The same command asked again is answered by the policy without a
/// dialog; a different command is still asked about.
#[tokio::test]
async fn always_for_a_command_is_remembered_for_the_project() {
    let fixture = Fixture::new();
    fixture
        .store
        .set_setting(
            eavery_core::model::Settings::KEY,
            &eavery_core::model::Settings {
                mode: eavery_core::model::UiMode::Developer,
                ..Default::default()
            },
        )
        .unwrap();
    let engine = TestEngine::new(vec![
        vec![Act::Ask(command("Run the backup script"))],
        vec![
            Act::Ask(command("Run  the BACKUP script")),
            Act::Ask(command("Run the other script")),
        ],
    ]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::AllowAlways)
        .await;

    runner.run_turn("Back it up").await.unwrap();
    assert_eq!(fixture.seen.asked().len(), 1);
    assert_eq!(engine.answers()[0].1, Decision::AllowAlways);

    runner.run_turn("Back it up again").await.unwrap();
    let asked = fixture.seen.asked();
    assert_eq!(
        asked.len(),
        2,
        "the remembered command is not asked about again"
    );
    assert_eq!(asked[1].title, "Run the other script");
    assert_eq!(asked[1].always, AlwaysOffer::DeveloperOnly);
    assert_eq!(
        engine.answers()[1].1,
        Decision::AllowOnce,
        "the engine is told once, by the policy"
    );

    let resolved: Vec<(Decision, DecidedBy)> = fixture
        .seen
        .events()
        .iter()
        .filter_map(|stored| match &stored.event {
            CoreEvent::PermissionResolved { decision, by, .. } => Some((*decision, *by)),
            _ => None,
        })
        .collect();
    assert_eq!(
        resolved,
        vec![
            (Decision::AllowAlways, DecidedBy::User),
            (Decision::AllowOnce, DecidedBy::Policy),
            (Decision::AllowAlways, DecidedBy::User),
        ]
    );
}

/// C11: in Everyday mode there is no "always" for a command, and an answer
/// that says so anyway is narrowed to "once" and not remembered. The same
/// for anything outbound, in either mode.
#[tokio::test]
async fn an_always_the_table_forbids_is_narrowed_and_forgotten() {
    let fixture = Fixture::new();
    let mut fetch = command("Send the report to example.com");
    fetch.kind = "fetch".into();
    let engine = TestEngine::new(vec![
        vec![
            Act::Ask(command("Run the backup script")),
            Act::Ask(fetch.clone()),
        ],
        vec![Act::Ask(command("Run the backup script")), Act::Ask(fetch)],
    ]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::AllowAlways)
        .await;

    runner.run_turn("Do it").await.unwrap();
    runner.run_turn("Do it again").await.unwrap();

    assert_eq!(fixture.seen.asked().len(), 4, "nothing was remembered");
    for (title, decision) in engine.answers() {
        assert_eq!(decision, Decision::AllowOnce, "{title}");
    }
    let remembered: Option<Vec<String>> = fixture
        .store
        .setting(&eavery_core::policy::always_key(fixture.project.id))
        .unwrap();
    assert_eq!(remembered, None);
    let asked = fixture.seen.asked();
    assert_eq!(asked[0].always, AlwaysOffer::DeveloperOnly);
    assert_eq!(asked[1].always, AlwaysOffer::Never);
    assert_eq!(
        asked[1].in_plan,
        Some(false),
        "direct mode has no plan to be in"
    );
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

// ---- the plan gate (M4-T05) ------------------------------------------------

/// The whole loop: plan under the gate, wait, execute under the policy. What
/// the engine sees is two prompts, a mode switch before each, and the write
/// gate closed for the first; what the person sees is the plan, and the
/// changes they asked for reach the execute prompt.
#[tokio::test]
async fn a_plan_turn_plans_waits_for_a_yes_and_then_executes() {
    let fixture = Fixture::new();
    let report = fixture.path("report.txt");
    let engine = TestEngine::with_modes(
        vec![
            vec![
                Act::Ask(read_of("Read report.txt")),
                Act::Ask(edit_of(&report)),
                Act::Text(PLAN_REPLY),
            ],
            vec![
                Act::Ask(edit_of(&report)),
                Act::Write("report.txt", "FY26\n"),
            ],
        ],
        &["default", "plan", "acceptEdits"],
        "default",
    );
    fixture.seen.will_answer_plan(Some(Approval::Approved {
        edits: Some("  skip the cover page ".into()),
    }));
    let runner = fixture
        .runner_with(
            Arc::clone(&engine),
            Decision::RejectOnce,
            &planning_facts(),
            &ConnectorRegistry::default(),
        )
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Plan, "Rename FY25 to FY26")
        .await
        .unwrap();

    assert_eq!(outcome.turn.phase, TurnPhase::Done);
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);
    assert_eq!(outcome.digest.files_changed, vec!["report.txt"]);
    let plan = outcome.turn.plan.clone().expect("the plan is on the turn");
    assert_eq!(plan.summary, "Update the report");
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.user_edits.as_deref(), Some("skip the cover page"));

    // Two prompts: the plan prompt with the request, then the execute prompt
    // with the plan and the person's changes.
    let prompts = engine.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[0].contains("They asked: \"Rename FY25 to FY26\""),
        "{}",
        prompts[0]
    );
    assert!(prompts[0].contains("write a plan"), "{}", prompts[0]);
    assert!(
        prompts[1].contains("approved this plan:\nUpdate the report"),
        "{}",
        prompts[1]
    );
    assert!(prompts[1].contains("1. Open report.txt"), "{}", prompts[1]);
    assert!(
        prompts[1].contains("They added these changes to the plan: skip the cover page"),
        "{}",
        prompts[1]
    );

    // §2.1: plan mode before the plan prompt, the asking mode before the
    // execute prompt. §2.2: writes closed for planning, open again after.
    assert_eq!(engine.modes_set(), vec!["plan", "default"]);
    assert_eq!(engine.writes_allowed(), vec![false, true]);

    // The gate answered the plan phase: the read went through, the edit was
    // refused, nobody was asked. The policy answered the execute phase: the
    // edit inside the Project went through.
    assert_eq!(
        engine.answers(),
        vec![
            ("Read report.txt".to_owned(), Decision::AllowOnce),
            (format!("Edit {}", report.display()), Decision::RejectOnce),
            (format!("Edit {}", report.display()), Decision::AllowOnce),
        ]
    );
    assert!(fixture.seen.asked().is_empty(), "the plan gate never asks");
    assert_eq!(
        outcome.digest.refused_actions,
        vec![format!("Edit {}", report.display())],
        "what the gate refused is in the digest"
    );

    // The person was shown the plan, with who saw the documents.
    let reviewed = fixture.seen.reviewed();
    assert_eq!(reviewed.len(), 1);
    assert_eq!(reviewed[0].turn_id, outcome.turn.id);
    assert_eq!(reviewed[0].plan.summary, "Update the report");
    assert_eq!(reviewed[0].vendor, "Anthropic");

    // And the transcript reads in the order it happened.
    let kinds = fixture.seen.kinds();
    assert_eq!(
        kinds,
        vec![
            "engine_status",
            "turn_started",
            "checkpoint_created",
            "permission_requested",
            "permission_resolved",
            "permission_requested",
            "permission_resolved",
            "agent_text",
            "phase_changed",
            "plan_ready",
            "phase_changed",
            "permission_requested",
            "permission_resolved",
            "checkpoint_created",
            "turn_finished",
        ]
    );
    let phases: Vec<TurnPhase> = fixture
        .seen
        .events()
        .iter()
        .filter_map(|stored| match &stored.event {
            CoreEvent::TurnStarted { phase, .. } | CoreEvent::PhaseChanged { phase, .. } => {
                Some(*phase)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            TurnPhase::Planning,
            TurnPhase::AwaitingApproval,
            TurnPhase::Executing
        ]
    );
    let ready = fixture
        .seen
        .events()
        .into_iter()
        .find_map(|stored| match stored.event {
            CoreEvent::PlanReady { plan, vendor, .. } => Some((plan, vendor)),
            _ => None,
        })
        .expect("a plan_ready event");
    assert_eq!(ready.0.summary, "Update the report");
    assert_eq!(ready.1, "Anthropic");

    // Every decision is on the record, with who made it (M4-T06).
    let audit = fixture.store.list_audit(None, None).unwrap();
    let rows: Vec<(DecidedBy, String)> = audit
        .iter()
        .rev()
        .map(|entry| (entry.actor, entry.action.clone()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (DecidedBy::PlanGate, "allow_once".into()),
            (DecidedBy::PlanGate, "reject_once".into()),
            (DecidedBy::User, "plan_approved".into()),
            (DecidedBy::Policy, "allow_once".into()),
        ]
    );
    let approved = audit
        .iter()
        .find(|entry| entry.action == "plan_approved")
        .unwrap();
    assert_eq!(approved.detail["edits"], "skip the cover page");
    assert_eq!(approved.turn_id, Some(outcome.turn.id));
}

/// §2.4: no means nothing runs. One prompt was sent, the files are as they
/// were, and the turn says why it ended.
#[tokio::test]
async fn a_plan_that_is_rejected_runs_nothing() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![
        vec![Act::Text(PLAN_REPLY)],
        vec![Act::Write("report.txt", "FY26\n")],
    ]);
    fixture.seen.will_answer_plan(Some(Approval::Rejected));
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Plan, "Rename FY25 to FY26")
        .await
        .unwrap();

    assert_eq!(outcome.turn.phase, TurnPhase::Cancelled);
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert_eq!(
        engine.prompts().len(),
        1,
        "the execute prompt was never sent"
    );
    assert!(outcome.digest.files_changed.is_empty());
    assert_eq!(fixture.read("report.txt"), "FY25\n");
    assert!(
        outcome.turn.post_checkpoint.is_some(),
        "the turn is still bracketed"
    );
    assert_eq!(
        outcome.turn.plan.map(|plan| plan.summary),
        Some("Update the report".into()),
        "the plan that was refused stays on the turn"
    );

    let finished = fixture
        .seen
        .events()
        .into_iter()
        .find_map(|stored| match stored.event {
            CoreEvent::TurnFinished { stop_reason, .. } => Some(stop_reason),
            _ => None,
        })
        .unwrap();
    assert_eq!(finished, PLAN_REJECTED);
    assert_eq!(
        fixture.store.turn(outcome.turn.id).unwrap().unwrap().phase,
        TurnPhase::Cancelled
    );
    let audit = fixture.store.list_audit(None, None).unwrap();
    assert_eq!(audit[0].actor, DecidedBy::User);
    assert_eq!(audit[0].action, "plan_rejected");
    assert!(
        engine.writes_allowed().ends_with(&[true]),
        "writes are open again"
    );
}

/// Stop, pressed while the plan waits: there is no prompt in flight to
/// cancel, so the wait itself ends, and the engine is never asked to.
#[tokio::test]
async fn stopping_while_a_plan_waits_ends_the_turn_without_the_engine() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![Act::Text(PLAN_REPLY)]]);
    fixture.seen.will_answer_plan(None);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let running = {
        let runner = Arc::clone(&runner);
        tokio::spawn(async move { runner.run_turn_in(TurnMode::Plan, "Rename the year").await })
    };
    // Until the plan is up there is nothing to stop but the engine; wait for
    // the turn to reach the wait.
    let waited = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while runner.awaiting_approval().is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "the turn never reached AwaitingApproval");
    assert_eq!(runner.awaiting_approval(), runner.running_turn());

    runner.cancel().await.unwrap();

    let outcome = running.await.unwrap().unwrap();
    assert!(!engine.was_cancelled(), "nothing was in flight to cancel");
    assert_eq!(outcome.turn.phase, TurnPhase::Cancelled);
    assert_eq!(engine.prompts().len(), 1);
    assert!(runner.running_turn().is_none(), "the Project is free again");
    assert!(runner.awaiting_approval().is_none());
}

/// Stop during planning reaches the engine, and the turn ends there: no
/// plan, no wait, writes open again for the next turn.
#[tokio::test]
async fn stopping_during_planning_ends_the_turn_before_any_plan() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Text("Looking around..."),
        Act::Stop(StopReason::Cancelled),
    ]]);
    fixture
        .seen
        .will_answer_plan(Some(Approval::Approved { edits: None }));
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Plan, "Rename the year")
        .await
        .unwrap();

    assert_eq!(outcome.turn.phase, TurnPhase::Cancelled);
    assert_eq!(outcome.stop_reason, StopReason::Cancelled);
    assert!(outcome.turn.plan.is_none());
    assert!(fixture.seen.reviewed().is_empty(), "nothing to approve");
    assert!(!fixture.seen.kinds().contains(&"plan_ready".to_owned()));
    assert_eq!(engine.writes_allowed(), vec![false, true]);
}

/// §2.2: leaving plan mode is refused whatever kind the engine gave the
/// call, and a plain read is not.
#[tokio::test]
async fn the_plan_gate_refuses_leaving_plan_mode_whatever_its_kind() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Ask(read_of("Read report.txt")),
        Act::Ask(read_of("ExitPlanMode")),
        Act::Text(PLAN_REPLY),
    ]]);
    fixture.seen.will_answer_plan(Some(Approval::Rejected));
    let runner = fixture
        .runner_with(
            Arc::clone(&engine),
            Decision::AllowOnce,
            &planning_facts(),
            &ConnectorRegistry::default(),
        )
        .await;

    runner
        .run_turn_in(TurnMode::Plan, "Rename the year")
        .await
        .unwrap();

    assert_eq!(
        engine.answers(),
        vec![
            ("Read report.txt".to_owned(), Decision::AllowOnce),
            ("ExitPlanMode".to_owned(), Decision::RejectOnce),
        ]
    );
    assert!(
        fixture.seen.asked().is_empty(),
        "the gate decides on its own; the person is never asked during planning"
    );
    let resolved: Vec<(Decision, DecidedBy)> = fixture
        .seen
        .events()
        .iter()
        .filter_map(|stored| match &stored.event {
            CoreEvent::PermissionResolved { decision, by, .. } => Some((*decision, *by)),
            _ => None,
        })
        .collect();
    assert_eq!(
        resolved,
        vec![
            (Decision::AllowOnce, DecidedBy::PlanGate),
            (Decision::RejectOnce, DecidedBy::PlanGate),
        ]
    );
    let audit = fixture.store.list_audit(None, None).unwrap();
    let exit = audit
        .iter()
        .find(|entry| entry.detail["title"] == "ExitPlanMode")
        .unwrap();
    assert_eq!(exit.actor, DecidedBy::PlanGate);
    assert_eq!(exit.detail["plan_exit"], true);
    assert_eq!(exit.detail["phase"], "planning");
}

/// §2.2, last paragraph: an edit that completes during planning without a
/// permission request on the way means the engine went round its own
/// asking mode. It is reported, and the plan phase carries on.
#[tokio::test]
async fn a_change_made_during_planning_without_asking_is_reported() {
    let fixture = Fixture::new();
    let report = fixture.path("report.txt");
    let engine = TestEngine::new(vec![vec![
        Act::ToolCall(RawToolCall {
            id: "t1".into(),
            title: "Edit report.txt".into(),
            kind: "edit".into(),
            status: "in_progress".into(),
            locations: vec![report.display().to_string()],
            diff_paths: vec![],
            raw_input: None,
        }),
        Act::Write("report.txt", "FY26\n"),
        Act::Update(RawToolCallUpdate {
            id: "t1".into(),
            status: Some("completed".into()),
            ..Default::default()
        }),
        Act::Text(PLAN_REPLY),
    ]]);
    fixture.seen.will_answer_plan(Some(Approval::Rejected));
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Plan, "Rename the year")
        .await
        .unwrap();

    let errors: Vec<eavery_core::event::ErrorCode> = fixture
        .seen
        .events()
        .iter()
        .filter_map(|stored| match &stored.event {
            CoreEvent::Error { code, .. } => Some(*code),
            _ => None,
        })
        .collect();
    assert_eq!(
        errors,
        vec![eavery_core::event::ErrorCode::PlanGateBypassed]
    );
    assert_eq!(
        fixture.seen.reviewed().len(),
        1,
        "the plan phase finished normally"
    );
    // The Journal has what it changed: the digest names it, and Undo covers it.
    assert_eq!(outcome.digest.files_changed, vec!["report.txt"]);
    assert!(outcome.digest.undo_to.is_some());
}

/// A mutation that did ask during planning is not a bypass, even when the
/// engine then reports it completed (some do, with a failed status inside).
#[tokio::test]
async fn a_change_that_asked_first_is_not_a_bypass() {
    let fixture = Fixture::new();
    let report = fixture.path("report.txt");
    let mut asked = edit_of(&report);
    asked.tool_call_id = "t1".into();
    let engine = TestEngine::new(vec![vec![
        Act::Ask(asked),
        Act::ToolCall(RawToolCall {
            id: "t1".into(),
            title: "Edit report.txt".into(),
            kind: "edit".into(),
            status: "completed".into(),
            locations: vec![report.display().to_string()],
            diff_paths: vec![],
            raw_input: None,
        }),
        Act::Text(PLAN_REPLY),
    ]]);
    fixture.seen.will_answer_plan(Some(Approval::Rejected));
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    runner
        .run_turn_in(TurnMode::Plan, "Rename the year")
        .await
        .unwrap();

    assert!(!fixture.seen.kinds().contains(&"error".to_owned()));
}

/// §2.3, rule 2 (and §7, test 4): no block, or a broken one, and the reply
/// itself is the plan, with its list items as steps. The turn never fails on
/// the plan.
#[tokio::test]
async fn a_reply_without_a_plan_block_is_the_plan_in_the_engines_own_words() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![
        Act::Text("I would do two things:\n"),
        Act::Text("- open the report\n- change the year\n\n"),
        Act::Text("```eavery-plan\n{not json\n```"),
    ]]);
    fixture.seen.will_answer_plan(Some(Approval::Rejected));
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Plan, "Rename the year")
        .await
        .unwrap();

    let plan = outcome.turn.plan.unwrap();
    assert_eq!(plan.summary, "I would do two things:");
    assert_eq!(
        plan.steps
            .iter()
            .map(|step| step.text.as_str())
            .collect::<Vec<_>>(),
        vec!["open the report", "change the year"]
    );
    assert!(plan.raw_markdown.contains("{not json"));
    assert_eq!(
        fixture.seen.reviewed()[0].plan.summary,
        "I would do two things:"
    );
}

/// §3.2, the two Outbound rows: whether the plan listed it changes the
/// wording of the question and nothing else. Both are asked; neither is
/// answered by the plan.
#[tokio::test]
async fn an_outbound_call_says_whether_the_plan_listed_it_and_is_asked_either_way() {
    let fixture = Fixture::new();
    let mut listed = command("Send the report to example.com");
    listed.kind = "fetch".into();
    listed.tool_call_id = "f1".into();
    listed.request_id = "f1".into();
    let mut unlisted = command("Post to the intranet");
    unlisted.kind = "fetch".into();
    unlisted.tool_call_id = "f2".into();
    unlisted.request_id = "f2".into();
    let engine = TestEngine::new(vec![
        vec![Act::Text(PLAN_REPLY)],
        vec![Act::Ask(listed), Act::Ask(unlisted)],
    ]);
    fixture
        .seen
        .will_answer_plan(Some(Approval::Approved { edits: None }));
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::AllowOnce)
        .await;

    let outcome = runner.run_turn_in(TurnMode::Plan, "Send it").await.unwrap();

    let asked = fixture.seen.asked();
    assert_eq!(asked.len(), 2, "outbound is never silent, listed or not");
    assert_eq!(asked[0].in_plan, Some(true));
    assert_eq!(asked[1].in_plan, Some(false));
    assert_eq!(asked[0].always, AlwaysOffer::Never);
    assert_eq!(
        outcome.digest.outbound_actions,
        vec!["Send the report to example.com", "Post to the intranet"]
    );
    let audit = fixture.store.list_audit(None, None).unwrap();
    assert_eq!(audit[0].detail["in_plan"], false);
    assert_eq!(audit[1].detail["in_plan"], true);
}

/// An engine with no modes, or none matching the hints, still gets the
/// plan gate: nothing is switched, nothing fails.
#[tokio::test]
async fn an_engine_without_matching_modes_is_gated_all_the_same() {
    let fixture = Fixture::new();
    let report = fixture.path("report.txt");
    let engine = TestEngine::with_modes(
        vec![
            vec![Act::Ask(edit_of(&report)), Act::Text(PLAN_REPLY)],
            vec![Act::Write("report.txt", "FY26\n")],
        ],
        &["yolo"],
        "yolo",
    );
    fixture
        .seen
        .will_answer_plan(Some(Approval::Approved { edits: None }));
    let runner = fixture
        .runner_with(
            Arc::clone(&engine),
            Decision::RejectOnce,
            &planning_facts(),
            &ConnectorRegistry::default(),
        )
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Plan, "Rename the year")
        .await
        .unwrap();

    assert_eq!(outcome.turn.phase, TurnPhase::Done);
    assert!(
        engine.modes_set().is_empty(),
        "the session opened in the only mode there is"
    );
    assert_eq!(engine.answers()[0].1, Decision::RejectOnce, "gated anyway");
    assert_eq!(outcome.digest.files_changed, vec!["report.txt"]);
}

/// Direct mode is the execute phase alone: one prompt, with the request
/// where the plan goes (§5), and no plan on the turn.
#[tokio::test]
async fn direct_mode_sends_only_the_execute_prompt() {
    let fixture = Fixture::new();
    let engine = TestEngine::new(vec![vec![Act::Text("Done.")]]);
    let runner = fixture
        .runner(Arc::clone(&engine), Decision::RejectOnce)
        .await;

    let outcome = runner.run_turn("Tidy the folder").await.unwrap();

    let prompts = engine.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("approved this plan:\nTidy the folder\n"),
        "{}",
        prompts[0]
    );
    assert!(outcome.turn.plan.is_none());
    assert!(fixture.seen.reviewed().is_empty());
    assert!(
        engine.writes_allowed().is_empty(),
        "direct mode never closes the gate"
    );
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

// ---- questions (§5) --------------------------------------------------------

#[tokio::test]
async fn a_question_reads_answers_and_may_not_change_anything() {
    let fixture = Fixture::new();
    let report = fixture.path("report.txt");
    let engine = TestEngine::with_modes(
        vec![vec![
            Act::Ask(read_of("Read report.txt")),
            // The engine tries to edit anyway. Nothing about a question
            // invites this, which is the point: the guarantee cannot rest on
            // the engine behaving.
            Act::Ask(edit_of(&report)),
            Act::Text("The cover says FY25."),
        ]],
        &["default", "plan", "acceptEdits"],
        "default",
    );
    // The standing answer is "allow": if the edit is still refused, it was
    // refused by the gate and not by a user who happened to say no.
    let runner = fixture
        .runner_with(
            Arc::clone(&engine),
            Decision::AllowOnce,
            &planning_facts(),
            &ConnectorRegistry::default(),
        )
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Ask, "Which year is on the cover?")
        .await
        .unwrap();

    assert_eq!(outcome.turn.phase, TurnPhase::Done);
    assert_eq!(outcome.stop_reason, StopReason::EndTurn);

    // One prompt — there is no execute phase to follow — and it is the
    // question prompt, carrying the question.
    let prompts = engine.prompts();
    assert_eq!(prompts.len(), 1, "a question is one prompt");
    assert!(
        prompts[0].contains("Which year is on the cover?"),
        "{}",
        prompts[0]
    );
    assert!(prompts[0].contains("change nothing"), "{}", prompts[0]);
    assert!(
        !prompts[0].contains("approved this plan"),
        "a question is not an execute prompt: {}",
        prompts[0]
    );

    // The engine's read-only mode was selected, and writes were shut for the
    // whole of it and opened again on the way out.
    assert_eq!(engine.modes_set(), vec!["plan"]);
    assert_eq!(engine.writes_allowed(), vec![false, true]);

    // The gate answered both requests: the read through, the edit refused,
    // and the person was never asked.
    assert_eq!(
        engine.answers(),
        vec![
            ("Read report.txt".to_owned(), Decision::AllowOnce),
            (format!("Edit {}", report.display()), Decision::RejectOnce),
        ]
    );
    assert!(
        fixture.seen.asked().is_empty(),
        "a question never interrupts the person"
    );
    assert_eq!(
        outcome.digest.refused_actions,
        vec![format!("Edit {}", report.display())],
        "and what it refused is on the record"
    );

    // Nothing changed, which is the whole promise.
    assert!(outcome.digest.files_added.is_empty());
    assert!(outcome.digest.files_changed.is_empty());
    assert!(outcome.digest.files_removed.is_empty());

    // The decision was written down as the question's, not the plan's.
    let audit = fixture
        .store
        .list_audit(Some(fixture.project.id), Some(50))
        .unwrap();
    let phases: Vec<String> = audit
        .iter()
        .filter_map(|row| row.detail.get("phase")?.as_str().map(str::to_owned))
        .collect();
    assert!(
        phases.iter().all(|phase| phase == "asking"),
        "the audit says which phase refused: {phases:?}"
    );
}

#[tokio::test]
async fn a_question_that_writes_behind_the_gate_is_reported_and_still_protected() {
    let fixture = Fixture::new();
    let engine = TestEngine::with_modes(
        vec![vec![
            // No permission request at all: the engine simply writes, the way
            // one that ignores its own read-only mode would.
            Act::Write("report.txt", "FY26\n"),
            Act::Text("Done."),
        ]],
        &["default", "plan"],
        "default",
    );
    let runner = fixture
        .runner_with(
            Arc::clone(&engine),
            Decision::RejectOnce,
            &planning_facts(),
            &ConnectorRegistry::default(),
        )
        .await;

    let outcome = runner
        .run_turn_in(TurnMode::Ask, "Which year is on the cover?")
        .await
        .unwrap();

    // The Journal caught it, so the person can still take it back — a
    // question that changed something is a bug in the engine, not a reason
    // to lose the change silently.
    assert_eq!(outcome.digest.files_changed, vec!["report.txt"]);
    assert!(outcome.digest.undo_to.is_some(), "and it is still undoable");
}
