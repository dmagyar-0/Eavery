//! The turn state machine: what happens between a person asking for something
//! and their folder being different.
//!
//! Three loops share it (`docs/plan/06-plan-gate-permissions.md` §1 and §5).
//! The plan loop is the default: the engine runs twice, once to look and say
//! what it would do, once — after the person has said yes — to do it. Direct
//! mode sends only the second prompt, with the request where the plan would
//! go. A question sends one prompt with writes held shut and the plan gate
//! answering, and never reaches an execute phase at all. All three are
//! bracketed by checkpoints, and all three answer the engine's permission
//! requests from the same policy.
//!
//! ```text
//! request
//!    │
//!    ▼
//! [pre-turn checkpoint] ── fails ──▶ Error; STOP, the turn never runs
//!    │
//!    ▼
//! Planning ── plan prompt; writes closed; the plan gate refuses every
//!    │        mutation and every attempt to leave plan mode
//!    ▼
//! AwaitingApproval ── the plan, parsed or in the engine's own words, waits
//!    │                for an explicit yes. No timeout says yes for anyone.
//!    ├─ rejected / cancelled ──▶ post-turn checkpoint ──▶ Cancelled
//!    ▼
//! Executing ── execute prompt; permission handler: reversible goes
//!    │         through, the rest is asked
//!    ▼
//! (a question skips all of the above but the checkpoints: one prompt,
//!  writes shut, the gate refusing every mutation, then the digest — which
//!  is expected to be empty, and says so when it is not)
//!    ▼
//! [post-turn checkpoint] ── taken whatever happened, so a cancelled or
//!    │                      crashed turn is still undoable
//!    ▼
//! digest (pre..post) ──▶ Done / Cancelled / Failed
//! ```
//!
//! Three rules hold everywhere in this module:
//!
//! - **No checkpoint, no turn.** A folder that could not be protected is a
//!   folder nothing is allowed to touch (working rule 7).
//! - **One turn per Project** (`docs/plan/02-challenges.md` C13). A second
//!   `run_turn` is refused, and so is a restore while a turn is running.
//! - **The transcript never stops the work.** Before the engine starts, a
//!   store failure ends the turn. Once it is running, a failure to write an
//!   event down is logged and the turn goes on: a lost line of transcript
//!   must not abandon a turn halfway through changing someone's files.
//!
//! See `docs/plan/03-architecture.md` §6 and
//! `docs/plan/06-plan-gate-permissions.md`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use ts_rs::TS;

use crate::engine::{
    Engine, EngineError, EngineFacts, PermissionHandler, RawAgentEvent, RawToolCall,
    RawToolCallUpdate, StopReason, pick_mode,
};
use crate::event::{
    CoreEvent, DecidedBy, Decision, Digest, ErrorCode, PermissionView, PlanEntryView, ToolCallView,
};
use crate::journal::{ChangeSet, Journal, JournalError};
use crate::model::{
    Checkpoint, CheckpointId, CheckpointKind, EngineStatus, Plan, ProjectId, RiskClass, Session,
    SessionId, Settings, Turn, TurnId, TurnPhase, UiMode,
};
use crate::policy::{self, CallFacts, ConnectorRegistry, PlanGateVerdict, Verdict};
use crate::store::{NewAudit, Store, StoreError, StoredEvent};
use crate::{plan, prompts};

/// How much of the request goes into a checkpoint label. Long enough to
/// recognise the turn in a list, short enough to read in one glance.
const LABEL_MAX_CHARS: usize = 60;

/// The `stop_reason` a [`CoreEvent::TurnFinished`] carries when the person
/// looked at the plan and said no (§2.4). Not an engine's stop reason: the
/// execute prompt was never sent.
pub const PLAN_REJECTED: &str = "plan_rejected";

/// Where events go once they are on the record. Called in order, from the task
/// running the turn, so a listener that blocks holds the turn up: emit and
/// return.
pub type Observer = Arc<dyn Fn(&StoredEvent) + Send + Sync>;

/// Shows the plan and waits for the person's answer. There is no timeout on
/// this one, because the only thing a timeout could do is answer for them
/// (§2.4, "no timeouts that auto-approve"); a turn nobody comes back to is
/// ended by `cancel`.
pub type ApprovalHandler =
    Arc<dyn Fn(PlanReview) -> futures::future::BoxFuture<'static, Approval> + Send + Sync>;

/// Which loop a turn runs (§1, §5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnMode {
    /// Only the execute prompt, with the request where the plan would go.
    /// Allowed when the Project skips planning (Developer mode). The policy
    /// still applies.
    #[default]
    Direct,
    /// Plan, approve, execute.
    Plan,
    /// A question (Everyday's "Ask a question"): one prompt, with writes held
    /// shut for the whole of it, so the answer costs the person nothing but
    /// the reading (§5, `read_only_intent`).
    ///
    /// This is not `Direct` with a different prompt. Direct mode may write —
    /// that is what it is for — and a question that quietly edited a
    /// document would be the worst thing Eavery could do, because nobody
    /// asked it to change anything and nobody is watching a plan. So the gate
    /// that guards the plan phase guards this too, and the engine is put in
    /// its read-only mode on top: two answers to the same question, because
    /// an engine that ignores the mode still meets the gate.
    Ask,
}

impl TurnMode {
    /// Whether this mode may change anything. A question may not.
    pub fn writes(self) -> bool {
        !matches!(self, TurnMode::Ask)
    }
}

/// What the person is shown when a plan is ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanReview {
    pub turn_id: TurnId,
    pub plan: Plan,
    /// Who the documents were sent to, for the plan card.
    pub vendor: String,
}

/// The person's answer to a plan (§2.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Approval {
    /// Go ahead. `edits` is what they added, appended to the execute prompt
    /// as "changes requested by the user".
    Approved { edits: Option<String> },
    /// Not now. The turn ends `Cancelled`; nothing is executed.
    Rejected,
}

/// The ways a turn reaches the outside world: everything it does, the one
/// question only a person can answer, and the plan they have to say yes to.
#[derive(Clone)]
pub struct TurnCallbacks {
    pub events: Observer,
    /// Asked only about what the policy will not decide on its own. The
    /// engine is blocked until it answers; the timeout that protects it lives
    /// in `eavery-acp` (`06-plan-gate-permissions.md` §3.4), next to the
    /// request it has to answer.
    pub permission: PermissionHandler,
    /// Asked once per plan-mode turn, with the plan.
    pub approval: ApprovalHandler,
}

impl TurnCallbacks {
    /// Shows nothing and refuses everything it is asked about. The right
    /// behaviour when nobody is watching: an unattended run does the
    /// reversible work and stops at the door of anything else — and never
    /// executes a plan nobody read.
    pub fn unattended() -> Self {
        Self {
            events: Arc::new(|_| {}),
            permission: Arc::new(|_| std::future::ready(Decision::RejectOnce).boxed()),
            approval: Arc::new(|_| std::future::ready(Approval::Rejected).boxed()),
        }
    }
}

impl std::fmt::Debug for TurnCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnCallbacks").finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// One thing at a time per Project (C13).
    #[error("this Project is busy: {doing}")]
    Busy {
        doing: &'static str,
        turn_id: Option<TurnId>,
    },
    #[error("your files could not be protected: {source}")]
    Checkpoint {
        #[source]
        source: JournalError,
    },
    #[error("going back did not work: {source}")]
    Restore {
        #[source]
        source: JournalError,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("something went wrong inside Eavery: {0}")]
    Internal(String),
}

impl TurnError {
    pub fn code(&self) -> ErrorCode {
        match self {
            TurnError::Busy { .. } => ErrorCode::TurnAlreadyRunning,
            TurnError::Checkpoint { .. } => ErrorCode::CheckpointFailed,
            TurnError::Restore { .. } => ErrorCode::RestoreFailed,
            TurnError::Engine(error) => error.code(),
            TurnError::Store(_) | TurnError::Internal(_) => ErrorCode::Internal,
        }
    }

    /// What the user can do about it. Every error reaches them as a next
    /// action rather than as a failure (`07-ui-vocabulary.md` §4).
    pub fn next_action(&self) -> Option<String> {
        match self {
            TurnError::Busy { .. } => {
                Some("Wait for the assistant to finish, or press Stop.".to_owned())
            }
            TurnError::Checkpoint { source } | TurnError::Restore { source } => source
                .next_action()
                .or_else(|| Some("Try again in a moment.".to_owned())),
            TurnError::Store(error) => error.next_action(),
            TurnError::Engine(error) => error.next_action(),
            TurnError::Internal(_) => {
                Some("Try again. If it keeps happening, check Diagnostics.".to_owned())
            }
        }
    }

    fn as_event(&self, turn_id: Option<TurnId>) -> CoreEvent {
        if let TurnError::Engine(error) = self {
            return CoreEvent::from_engine_error(error, turn_id);
        }
        CoreEvent::Error {
            turn_id,
            code: self.code(),
            message: self.to_string(),
            next_action: self.next_action(),
        }
    }
}

/// What going back did: the checkpoint it landed on, and every file something
/// else held open and so was left alone. The second list is never dropped — a
/// partial restore the user does not know about is worse than one that failed.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
pub struct RestoreOutcome {
    pub checkpoint: Checkpoint,
    pub skipped_locked: Vec<PathBuf>,
}

/// What a finished turn did.
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    pub turn: Turn,
    /// How the engine's last prompt ended. A plan that was rejected never
    /// sent one; it reports `Cancelled`, and the turn's phase says why.
    pub stop_reason: StopReason,
    pub digest: Digest,
}

/// What a Project is busy with. There is only ever one (C13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Busy {
    Turn(TurnId),
    GoingBack,
}

impl Busy {
    fn doing(self) -> &'static str {
        match self {
            Busy::Turn(_) => "a turn is already running",
            Busy::GoingBack => "it is going back to an earlier checkpoint",
        }
    }

    fn turn_id(self) -> Option<TurnId> {
        match self {
            Busy::Turn(turn_id) => Some(turn_id),
            Busy::GoingBack => None,
        }
    }
}

/// Which prompt is in flight, for the event pump: the phases that allow no
/// writes watch for an engine that changed something without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Planning,
    Asking,
    Executing,
}

impl Phase {
    /// The word the audit row carries for a decision made in this phase.
    fn as_str(self) -> &'static str {
        match self {
            Phase::Planning => "planning",
            Phase::Asking => "asking",
            Phase::Executing => "executing",
        }
    }

    /// Whether a mutation reaching `completed` here means the engine went
    /// round the gate. True wherever writes are shut.
    fn watches_for_bypass(self) -> bool {
        matches!(self, Phase::Planning | Phase::Asking)
    }
}

/// How a turn ended, before it is written down.
enum Ending {
    /// The engine's last prompt returned.
    Stopped(StopReason),
    /// The person said no to the plan, or pressed Stop while it waited.
    PlanRejected,
    /// The engine failed.
    Failed(EngineError),
}

impl Ending {
    fn phase(&self) -> TurnPhase {
        match self {
            Ending::Stopped(StopReason::Cancelled) | Ending::PlanRejected => TurnPhase::Cancelled,
            Ending::Stopped(_) => TurnPhase::Done,
            Ending::Failed(_) => TurnPhase::Failed,
        }
    }

    /// The word the transcript carries.
    fn word(&self) -> String {
        match self {
            Ending::Stopped(reason) => reason.as_str().to_owned(),
            Ending::PlanRejected => PLAN_REJECTED.to_owned(),
            Ending::Failed(_) => "failed".to_owned(),
        }
    }
}

/// One open Project: its history, its engine, and the one turn it may be
/// running.
pub struct ProjectRunner {
    recorder: Arc<Recorder>,
    journal: Arc<Journal>,
    engine: Arc<dyn Engine>,
    engine_id: String,
    facts: EngineFacts,
    /// The engine's own session id, from `session/new`.
    engine_session: String,
    /// The mode the plan phase runs in, when the engine offers one matching
    /// its hint (§2.1). `None` means the client-side gate is the whole gate.
    plan_mode: Option<String>,
    /// The mode the execute phase runs in: the one matching the asking-mode
    /// hint, or whichever the session opened in.
    work_mode: Option<String>,
    /// The mode the engine is in now, as far as Eavery has been told, so a
    /// switch to the mode it is already in is not sent.
    mode_now: Mutex<Option<String>>,
    ask: PermissionHandler,
    approve: ApprovalHandler,
    /// Set while a plan waits for its answer: Stop resolves it without going
    /// through the engine, which has no prompt in flight to cancel.
    awaiting: Mutex<Option<oneshot::Sender<()>>>,
    connectors: Arc<ConnectorRegistry>,
    session: Session,
    busy: Arc<Mutex<Option<Busy>>>,
}

impl std::fmt::Debug for ProjectRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectRunner")
            .field("project_id", &self.session.project_id)
            .field("session_id", &self.session.id)
            .field("engine_id", &self.engine_id)
            .field("plan_mode", &self.plan_mode)
            .field("work_mode", &self.work_mode)
            .field("busy", &self.busy.lock().ok().and_then(|busy| *busy))
            .finish()
    }
}

impl ProjectRunner {
    /// Starts the engine, opens a session on the Project folder, and records
    /// the Session. The Project must already be in the store, and its Journal
    /// open: a folder with no history is a folder Eavery will not work in.
    ///
    /// `facts` is what the engine table says about this engine: the mode
    /// hints the plan gate matches against what `session/new` offers, the
    /// signatures of leaving plan mode, and the vendor for the plan card.
    pub async fn open(
        store: Arc<Store>,
        journal: Arc<Journal>,
        engine: Arc<dyn Engine>,
        engine_id: &str,
        facts: &EngineFacts,
        connectors: &ConnectorRegistry,
        callbacks: TurnCallbacks,
    ) -> Result<Self, TurnError> {
        let project_id = journal.project_id();
        let info = engine.start().await?;
        // `resume` is deliberately none: picking up an engine's earlier
        // session is M7-T05, and a half-resumed conversation is worse than a
        // fresh one.
        let opened = engine
            .open_session(journal.root(), &connectors.specs(), None)
            .await?;

        let session = Session {
            id: uuid::Uuid::new_v4(),
            project_id,
            engine_id: engine_id.to_owned(),
            engine_session_id: Some(opened.session_id.clone()),
            created_at: Utc::now(),
        };
        store.insert_session(&session)?;
        store.set_project_engine(project_id, Some(engine_id))?;

        // §2.1: the plan mode is the one matching the hint; the work mode is
        // the one matching the asking hint, or the one the session opened in.
        // A hint that matches nothing is logged and the client-side gate
        // carries the plan phase on its own.
        let plan_mode =
            pick_mode(&opened.modes, facts.plan_mode_hint.as_deref()).map(|mode| mode.id.clone());
        if facts.plan_mode_hint.is_some() && plan_mode.is_none() {
            tracing::warn!(
                engine = engine_id,
                hint = ?facts.plan_mode_hint,
                modes = ?opened.modes.iter().map(|mode| &mode.id).collect::<Vec<_>>(),
                "no mode matches the plan-mode hint; planning relies on the client-side gate"
            );
        }
        let work_mode = pick_mode(&opened.modes, facts.asking_mode_hint.as_deref())
            .map(|mode| mode.id.clone())
            .or_else(|| opened.current_mode.clone());

        let recorder = Arc::new(Recorder {
            store,
            observer: callbacks.events,
            session_id: session.id,
            project_id,
        });
        recorder.emit(CoreEvent::EngineStatus {
            engine_id: engine_id.to_owned(),
            status: EngineStatus::Ready {
                info,
                modes: opened.modes,
                current_mode: opened.current_mode.clone(),
            },
        })?;

        Ok(Self {
            recorder,
            journal,
            engine,
            engine_id: engine_id.to_owned(),
            facts: facts.clone(),
            engine_session: opened.session_id,
            plan_mode,
            work_mode,
            mode_now: Mutex::new(opened.current_mode),
            ask: callbacks.permission,
            approve: callbacks.approval,
            awaiting: Mutex::new(None),
            connectors: Arc::new(connectors.clone()),
            session,
            busy: Arc::new(Mutex::new(None)),
        })
    }

    pub fn project_id(&self) -> ProjectId {
        self.session.project_id
    }

    pub fn session_id(&self) -> SessionId {
        self.session.id
    }

    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// The Project folder.
    pub fn root(&self) -> &Path {
        self.journal.root()
    }

    /// The turn running right now, if any.
    pub fn running_turn(&self) -> Option<TurnId> {
        self.busy().and_then(Busy::turn_id)
    }

    /// The turn whose plan is waiting for an answer, if any.
    pub fn awaiting_approval(&self) -> Option<TurnId> {
        let waiting = self
            .awaiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if waiting { self.running_turn() } else { None }
    }

    fn busy(&self) -> Option<Busy> {
        *self.busy.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Claims the Project, or says what it is already doing.
    fn claim(&self, what: Busy) -> Result<BusyGuard, TurnError> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(current) = *busy {
            return Err(TurnError::Busy {
                doing: current.doing(),
                turn_id: current.turn_id(),
            });
        }
        *busy = Some(what);
        Ok(BusyGuard {
            busy: Arc::clone(&self.busy),
        })
    }

    /// Claims the Project for a turn and says which turn it will be, without
    /// running it yet.
    ///
    /// This exists for a caller that has to answer before the turn does any
    /// work — a Tauri command returns a `TurnId` while the turn goes on in a
    /// task of its own — so that "this Project is busy" is the answer to
    /// `start_turn` rather than an error arriving later from nowhere.
    pub fn claim_turn(&self) -> Result<TurnTicket, TurnError> {
        let turn_id = uuid::Uuid::new_v4();
        Ok(TurnTicket {
            turn_id,
            _guard: self.claim(Busy::Turn(turn_id))?,
        })
    }

    /// Runs one direct-mode turn, start to finish: protect, prompt, protect,
    /// report. [`Self::run_turn_in`] chooses the loop.
    ///
    /// Returns when the engine's turn ends. A cancelled turn is a normal
    /// return — the work it did up to the cancel is checkpointed and in the
    /// digest — and only an engine that failed is an error.
    pub async fn run_turn(&self, request: &str) -> Result<TurnOutcome, TurnError> {
        self.run_turn_in(TurnMode::Direct, request).await
    }

    /// Runs one turn in the given mode. In plan mode the call stays inside
    /// `AwaitingApproval` until the approval callback answers or `cancel`
    /// is called.
    pub async fn run_turn_in(
        &self,
        mode: TurnMode,
        request: &str,
    ) -> Result<TurnOutcome, TurnError> {
        self.run_claimed(self.claim_turn()?, mode, request).await
    }

    /// Runs the turn a [`TurnTicket`] already claimed. The claim is released
    /// when this returns, whatever it returns.
    pub async fn run_claimed(
        &self,
        ticket: TurnTicket,
        mode: TurnMode,
        request: &str,
    ) -> Result<TurnOutcome, TurnError> {
        let turn_id = ticket.turn_id;
        let _ticket = ticket;

        let mut turn = Turn {
            id: turn_id,
            session_id: self.session.id,
            request: request.to_owned(),
            phase: match mode {
                TurnMode::Plan => TurnPhase::Planning,
                // A question never reaches an execute phase, but it is
                // working from the moment it is asked, and `Executing` is
                // the phase the UI reads as "working on it".
                TurnMode::Direct | TurnMode::Ask => TurnPhase::Executing,
            },
            plan: None,
            pre_checkpoint: None,
            post_checkpoint: None,
            started_at: Utc::now(),
        };
        self.recorder.store.insert_turn(&turn)?;
        self.recorder.emit(CoreEvent::TurnStarted {
            turn_id,
            phase: turn.phase,
        })?;

        // No checkpoint, no turn.
        let pre = match self
            .checkpoint(&label("Before", request), CheckpointKind::PreTurn, turn_id)
            .await
        {
            Ok(checkpoint) => checkpoint,
            Err(error) => {
                self.recorder.emit_or_log(error.as_event(Some(turn_id)));
                turn.phase = TurnPhase::Failed;
                let _ = self.recorder.store.update_turn(&turn);
                self.recorder.emit_or_log(CoreEvent::TurnFinished {
                    turn_id,
                    stop_reason: "failed".to_owned(),
                    digest: None,
                });
                return Err(error);
            }
        };
        turn.pre_checkpoint = Some(pre.id.clone());
        self.recorder.store.update_turn(&turn)?;

        let log = Arc::new(Mutex::new(TurnLog::default()));
        let ending = match mode {
            TurnMode::Direct => self.execute(&mut turn, None, &log).await,
            TurnMode::Plan => self.plan_then_execute(&mut turn, &log).await,
            TurnMode::Ask => self.answer(&mut turn, &log).await,
        };

        self.finish(turn, &pre, ending, &log).await
    }

    /// A question (§5): one prompt, nothing changed.
    ///
    /// The same three locks the plan phase uses, for the same reason —
    /// writes shut at the engine, the engine's read-only mode selected, and
    /// the gate answering every permission request — because "read-only
    /// intent" that rests on the prompt alone is a wish, not a guarantee.
    /// Unlike the plan phase there is no second phase to open writes for, so
    /// they are opened again on the way out and nothing else runs in between.
    async fn answer(&self, turn: &mut Turn, log: &Arc<Mutex<TurnLog>>) -> Ending {
        let turn_id = turn.id;

        self.engine.set_writes_allowed(false).await;
        self.switch_mode(self.plan_mode.as_deref()).await;
        let handler = self.gate_handler(turn_id, Arc::clone(log), Phase::Asking);
        let prompt = prompts::ask_prompt(&self.root_text(), &turn.request);
        let (stop, _reply) = self
            .prompt(turn_id, &prompt, handler, Phase::Asking, log)
            .await;
        self.engine.set_writes_allowed(true).await;

        match stop {
            Ok(reason) => Ending::Stopped(reason),
            Err(error) => Ending::Failed(error),
        }
    }

    /// The plan phase, the wait, and — given a yes — the execute phase
    /// (§2).
    async fn plan_then_execute(&self, turn: &mut Turn, log: &Arc<Mutex<TurnLog>>) -> Ending {
        let turn_id = turn.id;

        // Writes are closed for the whole plan phase and opened again
        // whatever happens in it: a gate left shut by a failed plan would
        // stop the next direct turn from writing anything.
        self.engine.set_writes_allowed(false).await;
        self.switch_mode(self.plan_mode.as_deref()).await;
        let handler = self.gate_handler(turn_id, Arc::clone(log), Phase::Planning);
        let prompt = prompts::plan_prompt(&self.root_text(), &turn.request, &[]);
        let (stop, reply) = self
            .prompt(turn_id, &prompt, handler, Phase::Planning, log)
            .await;
        self.engine.set_writes_allowed(true).await;

        match stop {
            Err(error) => return Ending::Failed(error),
            Ok(StopReason::Cancelled) => return Ending::Stopped(StopReason::Cancelled),
            Ok(StopReason::EndTurn) => {}
            Ok(other) => {
                // The engine stopped early. Whatever it said is still the
                // plan: never fail the turn on the plan (§2.3, rule 3).
                tracing::info!(reason = other.as_str(), "the plan phase stopped early");
            }
        }

        let mut plan = plan::extract(&reply);
        turn.plan = Some(plan.clone());
        turn.phase = TurnPhase::AwaitingApproval;
        self.update_turn_or_log(turn);
        self.recorder.emit_or_log(CoreEvent::PhaseChanged {
            turn_id,
            phase: TurnPhase::AwaitingApproval,
        });
        self.recorder.emit_or_log(CoreEvent::PlanReady {
            turn_id,
            plan: plan.clone(),
            vendor: self.facts.vendor.clone(),
        });

        let review = PlanReview {
            turn_id,
            plan: plan.clone(),
            vendor: self.facts.vendor.clone(),
        };
        match self.await_approval(review).await {
            Approval::Approved { edits } => {
                plan.user_edits = edits
                    .map(|edits| edits.trim().to_owned())
                    .filter(|edits| !edits.is_empty());
                self.recorder.audit(
                    NewAudit::new(DecidedBy::User, "plan_approved")
                        .for_turn(turn_id)
                        .with_detail(serde_json::json!({
                            "summary": plan.summary,
                            "edits": plan.user_edits,
                        })),
                );
                turn.plan = Some(plan.clone());
                turn.phase = TurnPhase::Executing;
                self.update_turn_or_log(turn);
                self.recorder.emit_or_log(CoreEvent::PhaseChanged {
                    turn_id,
                    phase: TurnPhase::Executing,
                });
                self.execute(turn, Some(&plan), log).await
            }
            Approval::Rejected => {
                self.recorder.audit(
                    NewAudit::new(DecidedBy::User, "plan_rejected")
                        .for_turn(turn_id)
                        .with_detail(serde_json::json!({ "summary": plan.summary })),
                );
                Ending::PlanRejected
            }
        }
    }

    /// The execute phase: the execute prompt under the policy handler, with
    /// the approved plan's outbound list to word its questions with. Direct
    /// mode passes no plan and the request stands in for it (§5).
    async fn execute(
        &self,
        turn: &mut Turn,
        plan: Option<&Plan>,
        log: &Arc<Mutex<TurnLog>>,
    ) -> Ending {
        let root = self.root_text();
        let prompt = match plan {
            Some(plan) => prompts::execute_prompt(&root, plan),
            None => prompts::execute_prompt_for(&root, &turn.request, None),
        };
        self.switch_mode(self.work_mode.as_deref()).await;
        let handler = self.permission_handler(
            turn.id,
            Arc::clone(log),
            plan.map(|plan| plan.outbound.clone()),
        );
        let (stop, _reply) = self
            .prompt(turn.id, &prompt, handler, Phase::Executing, log)
            .await;
        match stop {
            Ok(reason) => Ending::Stopped(reason),
            Err(error) => Ending::Failed(error),
        }
    }

    /// Waits for the person's answer to the plan, or for Stop.
    async fn await_approval(&self, review: PlanReview) -> Approval {
        let (cancel, cancelled) = oneshot::channel();
        *self.awaiting.lock().unwrap_or_else(|e| e.into_inner()) = Some(cancel);
        let approval = tokio::select! {
            approval = (self.approve)(review) => approval,
            _ = cancelled => Approval::Rejected,
        };
        *self.awaiting.lock().unwrap_or_else(|e| e.into_inner()) = None;
        approval
    }

    /// The post-turn checkpoint, the digest, and the record of how it ended.
    async fn finish(
        &self,
        mut turn: Turn,
        pre: &Checkpoint,
        ending: Ending,
        log: &Arc<Mutex<TurnLog>>,
    ) -> Result<TurnOutcome, TurnError> {
        let turn_id = turn.id;

        // Taken whatever happened above: a turn that was cancelled or whose
        // engine died halfway still changed files, and those changes are only
        // undoable once they are in the history. A rejected plan changed
        // nothing, and the Journal answers with the point it is already at.
        let post = self
            .checkpoint(
                &label("After", &turn.request),
                CheckpointKind::PostTurn,
                turn_id,
            )
            .await;
        if let Err(error) = &post {
            self.recorder.emit_or_log(error.as_event(Some(turn_id)));
        }

        let digest = self.digest(pre, post.as_ref().ok(), log).await;

        turn.post_checkpoint = post.as_ref().ok().map(|checkpoint| checkpoint.id.clone());
        turn.phase = ending.phase();
        let _ = self.recorder.store.update_turn(&turn);

        if let Ending::Failed(error) = &ending {
            self.recorder
                .emit_or_log(CoreEvent::from_engine_error(error, Some(turn_id)));
        }
        self.recorder.emit_or_log(CoreEvent::TurnFinished {
            turn_id,
            stop_reason: ending.word(),
            digest: Some(digest.clone()),
        });

        match ending {
            Ending::Failed(error) => Err(error.into()),
            Ending::Stopped(stop_reason) => Ok(TurnOutcome {
                turn,
                stop_reason,
                digest,
            }),
            Ending::PlanRejected => Ok(TurnOutcome {
                turn,
                stop_reason: StopReason::Cancelled,
                digest,
            }),
        }
    }

    /// Sends a prompt and turns everything the engine does into events.
    /// Returns how the prompt ended and the engine's whole reply, which the
    /// plan phase parses.
    ///
    /// The two run together: the engine streams while it works, and the
    /// stream ends when `prompt` drops its sender.
    async fn prompt(
        &self,
        turn_id: TurnId,
        text: &str,
        handler: PermissionHandler,
        phase: Phase,
        log: &Arc<Mutex<TurnLog>>,
    ) -> (Result<StopReason, EngineError>, String) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RawAgentEvent>();

        let pump = async {
            let mut calls: HashMap<String, ToolCallView> = HashMap::new();
            let mut reply = String::new();
            let mut reported: HashSet<String> = HashSet::new();
            while let Some(raw) = rx.recv().await {
                match &raw {
                    RawAgentEvent::Text(chunk) => reply.push_str(chunk),
                    RawAgentEvent::ModeChanged(mode) => {
                        *self.mode_now.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(mode.clone());
                    }
                    _ => {}
                }
                let Some(event) = self.core_event(turn_id, raw, &mut calls) else {
                    continue;
                };
                let bypass = match &event {
                    CoreEvent::ToolCallStarted { call, .. }
                    | CoreEvent::ToolCallUpdated { call, .. }
                        if phase.watches_for_bypass() =>
                    {
                        self.bypass_of(call, log, &mut reported)
                    }
                    _ => None,
                };
                self.recorder.emit_or_log(event);
                if let Some(error) = bypass {
                    self.recorder.emit_or_log(error);
                }
            }
            reply
        };

        let (stop, reply) = tokio::join!(
            self.engine.prompt(&self.engine_session, text, tx, handler),
            pump
        );
        (stop, reply)
    }

    /// §2.2, last paragraph: a mutation that reached `completed` during
    /// planning without ever asking means the engine went round its own
    /// asking mode. Reported once per call; the plan phase carries on and the
    /// Journal has whatever it changed.
    fn bypass_of(
        &self,
        call: &ToolCallView,
        log: &Arc<Mutex<TurnLog>>,
        reported: &mut HashSet<String>,
    ) -> Option<CoreEvent> {
        if !policy::bypassed_plan_gate(&call.kind, &call.status) {
            return None;
        }
        if log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .asked
            .contains(&call.id)
        {
            return None;
        }
        if !reported.insert(call.id.clone()) {
            return None;
        }
        tracing::warn!(
            engine = %self.engine_id,
            call = %call.title,
            "the engine changed something during planning without asking"
        );
        Some(CoreEvent::Error {
            turn_id: None,
            code: ErrorCode::PlanGateBypassed,
            message: format!(
                "the assistant changed something while it was only meant to be planning: {}",
                call.title
            ),
            next_action: Some(
                "Check the assistant's permission settings. Your files are protected; Undo takes this back."
                    .to_owned(),
            ),
        })
    }

    /// Asks the engine to stop the turn that is running. A Project with no
    /// turn running is already stopped, which is not an error.
    ///
    /// While a plan waits for its answer there is no prompt in flight, so the
    /// wait itself is what is stopped: the turn ends `Cancelled` without the
    /// execute phase, and nothing goes to the engine.
    pub async fn cancel(&self) -> Result<(), TurnError> {
        if self.running_turn().is_none() {
            return Ok(());
        }
        let waiting = self
            .awaiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(waiting) = waiting {
            let _ = waiting.send(());
            return Ok(());
        }
        self.engine.cancel(&self.engine_session).await?;
        Ok(())
    }

    /// Takes a checkpoint on the user's say-so rather than a turn's.
    pub async fn checkpoint_now(&self, label: &str) -> Result<Checkpoint, TurnError> {
        let checkpoint =
            checkpoint_without_session(&self.recorder.store, Arc::clone(&self.journal), label)
                .await?;
        self.recorder.emit_or_log(CoreEvent::CheckpointCreated {
            checkpoint: checkpoint.clone(),
        });
        Ok(checkpoint)
    }

    /// Goes back to a checkpoint. Refused while a turn is running (C13): the
    /// engine would write over the restored files as it came back.
    ///
    /// Returns the new checkpoint the restore made, and any file that was
    /// held open and so left alone — reported, never silently skipped.
    pub async fn restore(&self, target: &CheckpointId) -> Result<RestoreOutcome, TurnError> {
        let _guard = self.claim(Busy::GoingBack)?;

        let outcome =
            restore_without_session(&self.recorder.store, Arc::clone(&self.journal), target)
                .await?;

        self.recorder.emit_or_log(CoreEvent::Restored {
            to: target.clone(),
            new_checkpoint: outcome.checkpoint.id.clone(),
            skipped_locked: outcome
                .skipped_locked
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        });
        Ok(outcome)
    }

    /// Brings the store's cache of checkpoints up to date with the Journal,
    /// which is the one that knows.
    pub async fn sync_checkpoints(&self, limit: usize) -> Result<Vec<Checkpoint>, TurnError> {
        sync_checkpoints(&self.recorder.store, Arc::clone(&self.journal), limit).await
    }

    /// Stops the engine process. The Project's history and transcript are on
    /// disk; nothing here is lost by it.
    pub async fn shutdown(&self) {
        self.engine.shutdown().await;
    }

    /// `session/set_mode`, unless the engine is already there. A failure is
    /// logged and nothing else (§2.1): the client-side gate holds regardless,
    /// and the execute phase in the wrong mode is the engine asking more
    /// often than it needed to, not less.
    async fn switch_mode(&self, target: Option<&str>) {
        let Some(target) = target else {
            return;
        };
        let already = self
            .mode_now
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_deref()
            == Some(target);
        if already {
            return;
        }
        match self.engine.set_mode(&self.engine_session, target).await {
            Ok(()) => {
                *self.mode_now.lock().unwrap_or_else(|e| e.into_inner()) = Some(target.to_owned());
            }
            Err(error) => {
                tracing::warn!(engine = %self.engine_id, mode = target, %error, "could not switch mode");
            }
        }
    }

    fn root_text(&self) -> String {
        self.journal.root().display().to_string()
    }

    fn update_turn_or_log(&self, turn: &Turn) {
        if let Err(error) = self.recorder.store.update_turn(turn) {
            tracing::error!(%error, "the turn's state could not be written down");
        }
    }

    async fn checkpoint(
        &self,
        label: &str,
        kind: CheckpointKind,
        turn_id: TurnId,
    ) -> Result<Checkpoint, TurnError> {
        let journal = Arc::clone(&self.journal);
        let label = label.to_owned();
        let checkpoint = blocking(move || journal.checkpoint(&label, kind, Some(turn_id), false))
            .await
            .map_err(|source| TurnError::Checkpoint { source })?;

        self.record_checkpoint(&checkpoint)?;
        Ok(checkpoint)
    }

    fn record_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), TurnError> {
        self.recorder.store.upsert_checkpoint(checkpoint)?;
        self.recorder.emit_or_log(CoreEvent::CheckpointCreated {
            checkpoint: checkpoint.clone(),
        });
        Ok(())
    }

    /// What the turn did, in file terms plus what left the machine and what
    /// was refused.
    ///
    /// When the post-turn checkpoint failed there is nothing to diff against,
    /// so the work tree itself is compared with the pre-turn checkpoint: the
    /// changes are real whether or not they were recorded.
    async fn digest(
        &self,
        pre: &Checkpoint,
        post: Option<&Checkpoint>,
        log: &Arc<Mutex<TurnLog>>,
    ) -> Digest {
        let journal = Arc::clone(&self.journal);
        let from = pre.id.clone();
        let to = post.map(|checkpoint| checkpoint.id.clone());
        let changes = blocking(move || match to {
            Some(to) => journal.diff(&from, &to),
            None => journal.diff_worktree(&from),
        })
        .await;

        let changes = match changes {
            Ok(changes) => changes,
            Err(error) => {
                // The turn happened; only the summary of it is missing.
                tracing::error!(%error, "could not work out what the turn changed");
                ChangeSet::default()
            }
        };

        let log = log.lock().unwrap_or_else(|e| e.into_inner());
        Digest {
            files_added: paths_as_text(&changes.added),
            files_changed: paths_as_text(&changes.changed),
            files_removed: paths_as_text(&changes.removed),
            outbound_actions: log.outbound.clone(),
            refused_actions: log.refused.clone(),
            undo_to: Some(pre.id.clone()),
        }
    }

    /// The execute phase's handler: the decision table, with the approved
    /// plan's outbound list when there is one.
    fn permission_handler(
        &self,
        turn_id: TurnId,
        log: Arc<Mutex<TurnLog>>,
        plan_outbound: Option<Vec<String>>,
    ) -> PermissionHandler {
        let decider = Arc::new(Decider {
            recorder: Arc::clone(&self.recorder),
            ask: Arc::clone(&self.ask),
            root: self.journal.root().to_path_buf(),
            connectors: Arc::clone(&self.connectors),
            plan_outbound,
            log,
            turn_id,
        });
        Arc::new(move |request: PermissionView| {
            let decider = Arc::clone(&decider);
            async move { decider.decide(request).await }.boxed()
        })
    }

    /// The handler for the phases that may not write: the client-side gate
    /// (§2.2). `phase` is what the audit row will say the refusal was for.
    fn gate_handler(
        &self,
        turn_id: TurnId,
        log: Arc<Mutex<TurnLog>>,
        phase: Phase,
    ) -> PermissionHandler {
        let gate = Arc::new(Gatekeeper {
            recorder: Arc::clone(&self.recorder),
            root: self.journal.root().to_path_buf(),
            connectors: Arc::clone(&self.connectors),
            exit_signatures: self.facts.plan_exit_signatures.clone(),
            log,
            turn_id,
            phase,
        });
        Arc::new(move |request: PermissionView| {
            let gate = Arc::clone(&gate);
            async move { gate.decide(request) }.boxed()
        })
    }

    /// One raw engine event, turned into the one the rest of Eavery speaks.
    ///
    /// `None` means there is nothing to show: a mode change or an update this
    /// version does not model. Those are logged, never shown, never fatal.
    fn core_event(
        &self,
        turn_id: TurnId,
        raw: RawAgentEvent,
        calls: &mut HashMap<String, ToolCallView>,
    ) -> Option<CoreEvent> {
        match raw {
            RawAgentEvent::Text(text) => Some(CoreEvent::AgentText { turn_id, text }),
            RawAgentEvent::Thought(text) => Some(CoreEvent::AgentThought { turn_id, text }),
            RawAgentEvent::ToolCall(call) => {
                let view = self.tool_call_view(call);
                calls.insert(view.id.clone(), view.clone());
                Some(CoreEvent::ToolCallStarted {
                    turn_id,
                    call: view,
                })
            }
            RawAgentEvent::ToolCallUpdate(update) => {
                let mut view = calls.remove(&update.id).unwrap_or_else(|| {
                    // An update for a call that was never announced. The
                    // engine is within its rights; the transcript shows what
                    // it said rather than nothing.
                    self.tool_call_view(RawToolCall {
                        id: update.id.clone(),
                        ..RawToolCall::default()
                    })
                });
                self.apply_update(&mut view, update);
                calls.insert(view.id.clone(), view.clone());
                Some(CoreEvent::ToolCallUpdated {
                    turn_id,
                    call: view,
                })
            }
            RawAgentEvent::PlanEntries(entries) => Some(CoreEvent::PlanUpdated {
                turn_id,
                entries: entries
                    .into_iter()
                    .map(|entry| PlanEntryView {
                        content: entry.content,
                        priority: entry.priority,
                        status: entry.status,
                    })
                    .collect(),
            }),
            RawAgentEvent::ModeChanged(mode) => {
                tracing::debug!(engine = %self.engine_id, %mode, "the engine changed mode");
                None
            }
            RawAgentEvent::Other(value) => {
                tracing::trace!(engine = %self.engine_id, %value, "an update Eavery does not model");
                None
            }
        }
    }

    fn tool_call_view(&self, call: RawToolCall) -> ToolCallView {
        let kind = if call.kind.is_empty() {
            "other".to_owned()
        } else {
            call.kind
        };
        // An engine that gave no title still has to appear as something in
        // the transcript, and its own id is the only thing left.
        let title = if call.title.is_empty() {
            call.id.clone()
        } else {
            call.title
        };
        let risk = policy::classify(
            &CallFacts {
                kind: &kind,
                title: &title,
                locations: &call.locations,
                raw_input: call.raw_input.as_ref(),
            },
            self.journal.root(),
            &self.connectors,
        );
        ToolCallView {
            risk,
            diff_summary: diff_summary(&call.diff_paths),
            title,
            id: call.id,
            kind,
            status: call.status,
            locations: call.locations,
        }
    }

    /// An absent field in an update means unchanged, never cleared.
    fn apply_update(&self, view: &mut ToolCallView, update: RawToolCallUpdate) {
        if let Some(title) = update.title {
            view.title = title;
        }
        if let Some(kind) = update.kind {
            view.kind = kind;
        }
        if let Some(status) = update.status {
            view.status = status;
        }
        if let Some(locations) = update.locations {
            view.locations = locations;
        }
        if let Some(diff_paths) = update.diff_paths {
            view.diff_summary = diff_summary(&diff_paths);
        }
        // A call that now names files it did not name before is a different
        // risk than it was.
        view.risk = policy::classify(
            &CallFacts::from(&*view),
            self.journal.root(),
            &self.connectors,
        );
    }
}

/// Protecting the folder with no engine attached.
///
/// "Save a point I can come back to" is not something an assistant does, so
/// it must not need one started. Same reasoning as
/// [`restore_without_session`], and the same one thing missing: with no
/// conversation open there is no transcript to record it in.
pub async fn checkpoint_without_session(
    store: &Store,
    journal: Arc<Journal>,
    label: &str,
) -> Result<Checkpoint, TurnError> {
    let label = label.to_owned();
    let checkpoint =
        blocking(move || journal.checkpoint(&label, CheckpointKind::Manual, None, false))
            .await
            .map_err(|source| TurnError::Checkpoint { source })?;
    store.upsert_checkpoint(&checkpoint)?;
    Ok(checkpoint)
}

/// Going back with no engine attached.
///
/// Undo has to work whether or not an assistant is running — it is the button
/// that makes everything else safe to try. The Journal and the store come out
/// of this exactly as they do from [`ProjectRunner::restore`]; the one thing
/// missing is the transcript entry, because with no conversation open there
/// is no transcript to put it in. See `CHANGELOG-plan.md`.
pub async fn restore_without_session(
    store: &Store,
    journal: Arc<Journal>,
    target: &CheckpointId,
) -> Result<RestoreOutcome, TurnError> {
    let target_id = target.clone();
    let restoring = Arc::clone(&journal);
    let (checkpoint, skipped_locked) = blocking(move || restoring.restore(&target_id))
        .await
        .map_err(|source| TurnError::Restore { source })?;

    // A restore leaves two checkpoints behind: the one that preserved the work
    // tree first (D16) and the restore itself. Both come back from the
    // Journal, which is the one that knows.
    sync_checkpoints(store, journal, 10).await?;
    Ok(RestoreOutcome {
        checkpoint,
        skipped_locked,
    })
}

/// Brings the store's cache of checkpoints up to date with the Journal.
pub async fn sync_checkpoints(
    store: &Store,
    journal: Arc<Journal>,
    limit: usize,
) -> Result<Vec<Checkpoint>, TurnError> {
    let checkpoints = blocking(move || journal.list(limit))
        .await
        .map_err(|source| TurnError::Checkpoint { source })?;
    for checkpoint in &checkpoints {
        store.upsert_checkpoint(checkpoint)?;
    }
    Ok(checkpoints)
}

/// Releases the Project when the turn (or the restore) ends, however it ends.
struct BusyGuard {
    busy: Arc<Mutex<Option<Busy>>>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        *self.busy.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// A claim on a Project, and the id of the turn it was claimed for. The claim
/// lasts until this is dropped or handed to [`ProjectRunner::run_claimed`].
#[derive(Debug)]
pub struct TurnTicket {
    turn_id: TurnId,
    /// Held for its `Drop`: dropping the ticket frees the Project.
    _guard: BusyGuard,
}

impl TurnTicket {
    pub fn turn_id(&self) -> TurnId {
        self.turn_id
    }
}

impl std::fmt::Debug for BusyGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BusyGuard").finish_non_exhaustive()
    }
}

/// What the permission handlers saw, for the digest and for the bypass check.
#[derive(Debug, Default)]
struct TurnLog {
    outbound: Vec<String>,
    refused: Vec<String>,
    /// The tool calls that asked permission during planning. A mutation
    /// that completed without being in here went round the gate.
    asked: HashSet<String>,
}

/// Everything answering one permission request in the execute phase needs.
/// It outlives the call that made it — the handler the engine holds is
/// `'static` — so it owns its share of the runner rather than borrowing it.
struct Decider {
    recorder: Arc<Recorder>,
    ask: PermissionHandler,
    root: PathBuf,
    connectors: Arc<ConnectorRegistry>,
    /// The approved plan's `outbound` list, for the wording of an Outbound
    /// question (§3.2). `None` in direct mode, where there is no plan to be
    /// listed in.
    plan_outbound: Option<Vec<String>>,
    log: Arc<Mutex<TurnLog>>,
    turn_id: TurnId,
}

impl Decider {
    /// The decision table in `docs/plan/06-plan-gate-permissions.md` §3.2:
    /// what the pre-turn checkpoint makes reversible goes through silently,
    /// and everything else is the user's call — unless they already gave it
    /// for this Project with "always". A silent decision is still written
    /// down.
    async fn decide(&self, request: PermissionView) -> Decision {
        // The engine's layer could only guess at the risk: it has neither the
        // Project root nor the Connector registry. Reclassify before anyone,
        // including the user, is shown the request.
        let facts = CallFacts::from(&request);
        let connector = self
            .connectors
            .owning(&facts)
            .map(|connector| connector.name().to_owned());
        let risk = policy::classify(&facts, &self.root, &self.connectors);
        // Whether the plan listed it only changes the wording of a question
        // that is asked either way; it never answers it.
        let in_plan = self
            .plan_outbound
            .as_ref()
            .is_some_and(|listed| policy::listed_in_plan(listed, &facts, connector.as_deref()));
        let verdict = policy::decide(risk, in_plan);
        let signature = policy::signature(&facts, connector.as_deref());
        let request = PermissionView {
            risk,
            always: verdict.always(),
            in_plan: verdict.in_plan(),
            connector: connector.clone(),
            ..request
        };

        self.recorder.emit_or_log(CoreEvent::PermissionRequested {
            turn_id: self.turn_id,
            request: request.clone(),
        });

        let mode = self.ui_mode();
        let (decision, by) = match verdict {
            Verdict::Allow => (Decision::AllowOnce, DecidedBy::Policy),
            Verdict::Ask(_) if self.allowed_before(&signature, risk, mode) => {
                (Decision::AllowOnce, DecidedBy::Policy)
            }
            Verdict::Ask(_) => {
                let answered = (self.ask)(request.clone()).await;
                let narrowed = policy::narrow(answered, risk, mode);
                if narrowed != answered {
                    tracing::warn!(
                        title = %request.title,
                        ?risk,
                        "\"always\" is not offered for this; treating it as \"once\""
                    );
                }
                if narrowed == Decision::AllowAlways {
                    self.remember(&signature);
                }
                (narrowed, DecidedBy::User)
            }
        };

        {
            let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
            if is_allow(decision) {
                if risk == RiskClass::Outbound {
                    log.outbound.push(request.title.clone());
                }
            } else {
                log.refused.push(request.title.clone());
            }
        }

        self.recorder.audit(
            NewAudit::new(by, decision_action(decision))
                .for_turn(self.turn_id)
                .with_risk(risk)
                .with_detail(serde_json::json!({
                    "phase": "executing",
                    "tool_call_id": request.tool_call_id,
                    "title": request.title,
                    "kind": request.kind,
                    "locations": request.locations,
                    "decision": decision,
                    "connector": connector,
                    "signature": signature,
                    "in_plan": request.in_plan,
                })),
        );
        self.recorder.emit_or_log(CoreEvent::PermissionResolved {
            turn_id: self.turn_id,
            request_id: request.request_id,
            decision,
            by,
        });
        decision
    }

    fn ui_mode(&self) -> UiMode {
        self.recorder
            .store
            .setting::<Settings>(Settings::KEY)
            .ok()
            .flatten()
            .map(|settings| settings.mode)
            .unwrap_or_default()
    }

    /// Whether the user already said "always" to this call for this Project
    /// (§3.3). The table is applied again at lookup: an "always" given for a
    /// command in Developer mode does not carry into Everyday mode, where
    /// the offer was never made.
    fn allowed_before(&self, signature: &str, risk: RiskClass, mode: UiMode) -> bool {
        policy::always_allowed(risk, mode) && self.remembered().iter().any(|s| s == signature)
    }

    fn remembered(&self) -> Vec<String> {
        self.recorder
            .store
            .setting::<Vec<String>>(&policy::always_key(self.recorder.project_id))
            .unwrap_or_else(|error| {
                tracing::error!(%error, "could not read the \"always\" decisions");
                None
            })
            .unwrap_or_default()
    }

    fn remember(&self, signature: &str) {
        let mut signatures = self.remembered();
        if signatures.iter().any(|s| s == signature) {
            return;
        }
        signatures.push(signature.to_owned());
        if let Err(error) = self
            .recorder
            .store
            .set_setting(&policy::always_key(self.recorder.project_id), &signatures)
        {
            tracing::error!(%error, "could not remember an \"always\" decision");
        }
    }
}

/// The plan phase's answer to every permission request (§2.2). Nobody is
/// asked: reads go through, everything else is refused, and so is any
/// attempt to leave plan mode. Every answer is written down as the plan
/// gate's.
struct Gatekeeper {
    recorder: Arc<Recorder>,
    root: PathBuf,
    connectors: Arc<ConnectorRegistry>,
    exit_signatures: Vec<String>,
    log: Arc<Mutex<TurnLog>>,
    turn_id: TurnId,
    /// Which write-shut phase this is, for the audit row.
    phase: Phase,
}

impl Gatekeeper {
    fn decide(&self, request: PermissionView) -> Decision {
        let facts = CallFacts::from(&request);
        let connector = self
            .connectors
            .owning(&facts)
            .map(|connector| connector.name().to_owned());
        let risk = policy::classify(&facts, &self.root, &self.connectors);
        let signatures: Vec<&str> = self.exit_signatures.iter().map(String::as_str).collect();
        let verdict = policy::plan_gate(&facts, &request.tool_call_id, &signatures);
        let plan_exit = policy::is_plan_exit(&facts, &signatures);
        let request = PermissionView {
            risk,
            always: policy::AlwaysOffer::Never,
            in_plan: None,
            connector: connector.clone(),
            ..request
        };

        self.recorder.emit_or_log(CoreEvent::PermissionRequested {
            turn_id: self.turn_id,
            request: request.clone(),
        });

        let decision = {
            let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
            log.asked.insert(request.tool_call_id.clone());
            match &verdict {
                PlanGateVerdict::Allow => Decision::AllowOnce,
                PlanGateVerdict::Refuse(refusal) => {
                    log.refused.push(refusal.title.clone());
                    Decision::RejectOnce
                }
            }
        };

        self.recorder.audit(
            NewAudit::new(DecidedBy::PlanGate, decision_action(decision))
                .for_turn(self.turn_id)
                .with_risk(risk)
                .with_detail(serde_json::json!({
                    "phase": self.phase.as_str(),
                    "tool_call_id": request.tool_call_id,
                    "title": request.title,
                    "kind": request.kind,
                    "locations": request.locations,
                    "decision": decision,
                    "connector": connector,
                    "plan_exit": plan_exit,
                })),
        );
        self.recorder.emit_or_log(CoreEvent::PermissionResolved {
            turn_id: self.turn_id,
            request_id: request.request_id,
            decision,
            by: DecidedBy::PlanGate,
        });
        decision
    }
}

/// Writes down everything that happens and passes it on.
struct Recorder {
    store: Arc<Store>,
    observer: Observer,
    session_id: SessionId,
    project_id: ProjectId,
}

impl Recorder {
    /// Stores an event and hands it to the listener. Used before the engine
    /// starts, where a database that cannot be written to is a reason not to
    /// begin.
    fn emit(&self, event: CoreEvent) -> Result<StoredEvent, StoreError> {
        let stored = self.store.append_event(self.session_id, &event)?;
        (self.observer)(&stored);
        Ok(stored)
    }

    /// The same, for once the turn is running: a transcript that cannot be
    /// written is worth a loud log and nothing else. The work is what matters,
    /// and it is already under way.
    fn emit_or_log(&self, event: CoreEvent) {
        if let Err(error) = self.emit(event) {
            tracing::error!(%error, "an event could not be written down");
        }
    }

    fn audit(&self, entry: NewAudit) {
        if let Err(error) = self.store.append_audit(&entry.for_project(self.project_id)) {
            tracing::error!(%error, "a decision could not be written down");
        }
    }
}

fn is_allow(decision: Decision) -> bool {
    matches!(decision, Decision::AllowOnce | Decision::AllowAlways)
}

/// The audit log's word for a decision. Stable and machine-readable; the UI
/// translates it, the log does not.
fn decision_action(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "allow_once",
        Decision::AllowAlways => "allow_always",
        Decision::RejectOnce => "reject_once",
        Decision::RejectAlways => "reject_always",
        Decision::Cancelled => "cancelled",
    }
}

fn diff_summary(diff_paths: &[String]) -> Option<String> {
    match diff_paths {
        [] => None,
        [one] => Some(format!("changes in {one}")),
        [first, rest @ ..] => Some(format!("changes in {first} and {} more", rest.len())),
    }
}

fn paths_as_text(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

/// "Before: rename every FY25 to FY26". The request itself is the label,
/// shortened on a character boundary so a checkpoint list reads as a list of
/// what was asked for.
fn label(prefix: &str, request: &str) -> String {
    let request = request.trim();
    let mut short: String = request.chars().take(LABEL_MAX_CHARS).collect();
    if short.chars().count() < request.chars().count() {
        short.push('…');
    }
    format!("{prefix}: {short}")
}

/// Runs a Journal call off the async runtime. Checkpointing a big folder is
/// seconds of hashing, and doing it on a runtime thread freezes everything
/// else the app is doing — in the desktop app, the window.
async fn blocking<T, F>(work: F) -> Result<T, JournalError>
where
    F: FnOnce() -> Result<T, JournalError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => {
            // The only ways this fails are a panic inside the work or the
            // runtime shutting down under it.
            tracing::error!(%error, "a Journal call did not finish");
            Err(JournalError::Cancelled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_the_request_shortened_on_a_character_boundary() {
        assert_eq!(label("Before", "  tidy up  "), "Before: tidy up");

        let long = "é".repeat(LABEL_MAX_CHARS + 10);
        let label = label("After", &long);
        assert!(label.ends_with('…'));
        assert_eq!(label.chars().count(), "After: ".len() + LABEL_MAX_CHARS + 1);
    }

    #[test]
    fn a_diff_summary_names_the_files_or_says_nothing() {
        assert_eq!(diff_summary(&[]), None);
        assert_eq!(
            diff_summary(&["report.docx".into()]).unwrap(),
            "changes in report.docx"
        );
        assert_eq!(
            diff_summary(&["a".into(), "b".into(), "c".into()]).unwrap(),
            "changes in a and 2 more"
        );
    }

    #[test]
    fn how_a_turn_ended_maps_to_a_phase_and_a_word() {
        assert_eq!(
            Ending::Stopped(StopReason::EndTurn).phase(),
            TurnPhase::Done
        );
        assert_eq!(
            Ending::Stopped(StopReason::Cancelled).phase(),
            TurnPhase::Cancelled
        );
        assert_eq!(Ending::PlanRejected.phase(), TurnPhase::Cancelled);
        assert_eq!(Ending::PlanRejected.word(), PLAN_REJECTED);
        let failed = Ending::Failed(EngineError::Timeout {
            engine_id: "x".into(),
            during: "prompt".into(),
            timeout_secs: 1,
        });
        assert_eq!(failed.phase(), TurnPhase::Failed);
        assert_eq!(failed.word(), "failed");
    }
}
