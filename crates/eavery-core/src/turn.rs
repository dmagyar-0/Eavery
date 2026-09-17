//! The turn state machine: what happens between a person asking for something
//! and their folder being different.
//!
//! Direct mode only (M2-T07). The plan gate — plan, approve, execute — is
//! M4-T05 and slots in ahead of the prompt below; everything else here is
//! already what that milestone needs.
//!
//! ```text
//! request
//!    │
//!    ▼
//! [pre-turn checkpoint] ── fails ──▶ Error; STOP, the turn never runs
//!    │
//!    ▼
//! prompt ── permission handler: reversible goes through, the rest is asked
//!    │
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
//! `docs/plan/06-plan-gate-permissions.md` §3.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::{
    Engine, EngineError, McpServerSpec, PermissionHandler, RawAgentEvent, RawToolCall,
    RawToolCallUpdate, StopReason,
};
use crate::event::{
    CoreEvent, DecidedBy, Decision, Digest, ErrorCode, PermissionView, PlanEntryView, ToolCallView,
};
use crate::journal::{ChangeSet, Journal, JournalError};
use crate::model::{
    Checkpoint, CheckpointId, CheckpointKind, EngineStatus, ProjectId, Session, SessionId, Turn,
    TurnId, TurnPhase,
};
use crate::store::{NewAudit, Store, StoreError, StoredEvent};

/// How much of the request goes into a checkpoint label. Long enough to
/// recognise the turn in a list, short enough to read in one glance.
const LABEL_MAX_CHARS: usize = 60;

/// Where events go once they are on the record. Called in order, from the task
/// running the turn, so a listener that blocks holds the turn up: emit and
/// return.
pub type Observer = Arc<dyn Fn(&StoredEvent) + Send + Sync>;

/// The two ways a turn reaches the outside world: everything it does, and the
/// one question only a person can answer.
#[derive(Clone)]
pub struct TurnCallbacks {
    pub events: Observer,
    /// Asked only about what the policy will not decide on its own. The
    /// engine is blocked until it answers; the timeout that protects it lives
    /// in `eavery-acp` (`06-plan-gate-permissions.md` §3.4), next to the
    /// request it has to answer.
    pub permission: PermissionHandler,
}

impl TurnCallbacks {
    /// Shows nothing and refuses everything it is asked about. The right
    /// behaviour when nobody is watching: an unattended run does the
    /// reversible work and stops at the door of anything else.
    pub fn unattended() -> Self {
        Self {
            events: Arc::new(|_| {}),
            permission: Arc::new(|_| std::future::ready(Decision::RejectOnce).boxed()),
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

/// One open Project: its history, its engine, and the one turn it may be
/// running.
pub struct ProjectRunner {
    recorder: Arc<Recorder>,
    journal: Arc<Journal>,
    engine: Arc<dyn Engine>,
    engine_id: String,
    /// The engine's own session id, from `session/new`.
    engine_session: String,
    ask: PermissionHandler,
    session: Session,
    busy: Arc<Mutex<Option<Busy>>>,
}

impl std::fmt::Debug for ProjectRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectRunner")
            .field("project_id", &self.session.project_id)
            .field("session_id", &self.session.id)
            .field("engine_id", &self.engine_id)
            .field("busy", &self.busy.lock().ok().and_then(|busy| *busy))
            .finish()
    }
}

impl ProjectRunner {
    /// Starts the engine, opens a session on the Project folder, and records
    /// the Session. The Project must already be in the store, and its Journal
    /// open: a folder with no history is a folder Eavery will not work in.
    pub async fn open(
        store: Arc<Store>,
        journal: Arc<Journal>,
        engine: Arc<dyn Engine>,
        engine_id: &str,
        connectors: &[McpServerSpec],
        callbacks: TurnCallbacks,
    ) -> Result<Self, TurnError> {
        let project_id = journal.project_id();
        let info = engine.start().await?;
        // `resume` is deliberately none: picking up an engine's earlier
        // session is M7-T05, and a half-resumed conversation is worse than a
        // fresh one.
        let opened = engine
            .open_session(journal.root(), connectors, None)
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
                current_mode: opened.current_mode,
            },
        })?;

        Ok(Self {
            recorder,
            journal,
            engine,
            engine_id: engine_id.to_owned(),
            engine_session: opened.session_id,
            ask: callbacks.permission,
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

    /// Runs one turn, start to finish: protect, prompt, protect, report.
    ///
    /// Returns when the engine's turn ends. A cancelled turn is a normal
    /// return — the work it did up to the cancel is checkpointed and in the
    /// digest — and only an engine that failed is an error.
    pub async fn run_turn(&self, request: &str) -> Result<TurnOutcome, TurnError> {
        self.run_claimed(self.claim_turn()?, request).await
    }

    /// Runs the turn a [`TurnTicket`] already claimed. The claim is released
    /// when this returns, whatever it returns.
    pub async fn run_claimed(
        &self,
        ticket: TurnTicket,
        request: &str,
    ) -> Result<TurnOutcome, TurnError> {
        let turn_id = ticket.turn_id;
        let _ticket = ticket;

        let mut turn = Turn {
            id: turn_id,
            session_id: self.session.id,
            request: request.to_owned(),
            // Direct mode goes straight to work. M4-T05 starts in `Planning`.
            phase: TurnPhase::Executing,
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
        let stop = self.prompt(turn_id, request, &log).await;

        // Taken whatever happened above: a turn that was cancelled or whose
        // engine died halfway still changed files, and those changes are only
        // undoable once they are in the history.
        let post = self
            .checkpoint(&label("After", request), CheckpointKind::PostTurn, turn_id)
            .await;
        if let Err(error) = &post {
            self.recorder.emit_or_log(error.as_event(Some(turn_id)));
        }

        let digest = self.digest(&pre, post.as_ref().ok(), &log).await;

        turn.post_checkpoint = post.as_ref().ok().map(|checkpoint| checkpoint.id.clone());
        turn.phase = match &stop {
            Ok(StopReason::Cancelled) => TurnPhase::Cancelled,
            Ok(_) => TurnPhase::Done,
            Err(_) => TurnPhase::Failed,
        };
        let _ = self.recorder.store.update_turn(&turn);

        if let Err(error) = &stop {
            self.recorder
                .emit_or_log(CoreEvent::from_engine_error(error, Some(turn_id)));
        }
        self.recorder.emit_or_log(CoreEvent::TurnFinished {
            turn_id,
            stop_reason: match &stop {
                Ok(reason) => reason.as_str().to_owned(),
                Err(_) => "failed".to_owned(),
            },
            digest: Some(digest.clone()),
        });

        let stop_reason = stop?;
        Ok(TurnOutcome {
            turn,
            stop_reason,
            digest,
        })
    }

    /// Sends the prompt and turns everything the engine does into events.
    ///
    /// The two run together: the engine streams while it works, and the
    /// stream ends when `prompt` drops its sender.
    async fn prompt(
        &self,
        turn_id: TurnId,
        request: &str,
        log: &Arc<Mutex<TurnLog>>,
    ) -> Result<StopReason, EngineError> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<RawAgentEvent>();
        let handler = self.permission_handler(turn_id, Arc::clone(log));

        let pump = async {
            let mut calls: HashMap<String, ToolCallView> = HashMap::new();
            while let Some(raw) = rx.recv().await {
                if let Some(event) = self.core_event(turn_id, raw, &mut calls) {
                    self.recorder.emit_or_log(event);
                }
            }
        };

        let (stop, ()) = tokio::join!(
            self.engine
                .prompt(&self.engine_session, request, tx, handler),
            pump
        );
        stop
    }

    /// Asks the engine to stop the turn that is running. A Project with no
    /// turn running is already stopped, which is not an error.
    pub async fn cancel(&self) -> Result<(), TurnError> {
        if self.running_turn().is_none() {
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

    fn permission_handler(&self, turn_id: TurnId, log: Arc<Mutex<TurnLog>>) -> PermissionHandler {
        let decider = Arc::new(Decider {
            recorder: Arc::clone(&self.recorder),
            ask: Arc::clone(&self.ask),
            root: self.journal.root().to_path_buf(),
            log,
            turn_id,
        });
        Arc::new(move |request: PermissionView| {
            let decider = Arc::clone(&decider);
            async move { decider.decide(request).await }.boxed()
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
        ToolCallView {
            risk: classify(&call.kind, &call.locations, self.journal.root()),
            diff_summary: diff_summary(&call.diff_paths),
            // An engine that gave no title still has to appear as something in
            // the transcript, and its own id is the only thing left.
            title: if call.title.is_empty() {
                call.id.clone()
            } else {
                call.title
            },
            id: call.id,
            kind: if call.kind.is_empty() {
                "other".to_owned()
            } else {
                call.kind
            },
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
        view.risk = classify(&view.kind, &view.locations, self.journal.root());
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

/// What the permission handler saw, for the digest.
#[derive(Debug, Default)]
struct TurnLog {
    outbound: Vec<String>,
    refused: Vec<String>,
}

/// Everything answering one permission request needs. It outlives the call
/// that made it — the handler the engine holds is `'static` — so it owns its
/// share of the runner rather than borrowing it.
struct Decider {
    recorder: Arc<Recorder>,
    ask: PermissionHandler,
    root: PathBuf,
    log: Arc<Mutex<TurnLog>>,
    turn_id: TurnId,
}

impl Decider {
    /// The M2 rows of the decision table in
    /// `docs/plan/06-plan-gate-permissions.md` §3.2: what the pre-turn
    /// checkpoint makes reversible goes through silently, and everything else
    /// is the user's call. A silent decision is still written down.
    async fn decide(&self, request: PermissionView) -> Decision {
        // The engine's layer could only guess at the risk: it has neither the
        // Project root nor the Connector registry. Reclassify before anyone,
        // including the user, is shown the request.
        let risk = classify(&request.kind, &request.locations, &self.root);
        let request = PermissionView { risk, ..request };

        self.recorder.emit_or_log(CoreEvent::PermissionRequested {
            turn_id: self.turn_id,
            request: request.clone(),
        });

        let (decision, by) = match settled_by_policy(risk) {
            Some(decision) => (decision, DecidedBy::Policy),
            None => ((self.ask)(request.clone()).await, DecidedBy::User),
        };

        {
            let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
            if is_allow(decision) {
                if risk == crate::model::RiskClass::Outbound {
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
                    "tool_call_id": request.tool_call_id,
                    "title": request.title,
                    "kind": request.kind,
                    "locations": request.locations,
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

/// The risk table from `docs/plan/06-plan-gate-permissions.md` §3.1, without
/// the Connector lookup: no Connector registry exists yet (M6-T08), and
/// `policy::classify` (M4-T02) takes this over with one.
///
/// Two rules are worth keeping in sight. A call that names no file is
/// `Destructive`, not `Reversible`: an engine that will not say what it is
/// about to touch is a reason to ask. And anything unrecognised is `Execute`,
/// never `Read`.
fn classify(kind: &str, locations: &[String], root: &Path) -> crate::model::RiskClass {
    use crate::model::RiskClass;
    match kind {
        "read" | "search" | "think" | "other" | "" => RiskClass::Read,
        "edit" | "delete" | "move" => {
            if locations.is_empty() {
                return RiskClass::Destructive;
            }
            if locations
                .iter()
                .all(|path| crate::paths::is_inside(path, root))
            {
                RiskClass::Reversible
            } else {
                // Outside the Project is outside the Journal: nothing here
                // could take it back.
                RiskClass::Destructive
            }
        }
        "fetch" => crate::model::RiskClass::Outbound,
        _ => RiskClass::Execute,
    }
}

/// `None` means only a person can answer this one.
fn settled_by_policy(risk: crate::model::RiskClass) -> Option<Decision> {
    use crate::model::RiskClass;
    match risk {
        // Undo covers it, so asking would be theatre.
        RiskClass::Read | RiskClass::Reversible => Some(Decision::AllowOnce),
        RiskClass::Execute | RiskClass::Outbound | RiskClass::Destructive => None,
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
    use crate::model::RiskClass;

    fn root() -> PathBuf {
        let dir = std::env::temp_dir().join("eavery-classify");
        std::fs::create_dir_all(&dir).unwrap();
        crate::paths::canonical_or_self(&dir)
    }

    #[test]
    fn reads_are_read_whatever_they_read() {
        let root = root();
        assert_eq!(classify("read", &[], &root), RiskClass::Read);
        assert_eq!(classify("search", &[], &root), RiskClass::Read);
        assert_eq!(
            classify("read", &["/somewhere/else.txt".into()], &root),
            RiskClass::Read
        );
    }

    #[test]
    fn an_edit_inside_the_project_is_reversible_and_outside_it_is_not() {
        let root = root();
        let inside = root.join("report.docx").display().to_string();
        assert_eq!(
            classify("edit", std::slice::from_ref(&inside), &root),
            RiskClass::Reversible
        );
        assert_eq!(
            classify("delete", &[inside, "/etc/hosts".into()], &root),
            RiskClass::Destructive,
            "one file outside the Project is enough: Undo could not take it back"
        );
    }

    /// An engine that will not say what it is about to change is a reason to
    /// ask, not a reason to relax.
    #[test]
    fn an_edit_that_names_no_file_is_destructive() {
        assert_eq!(classify("edit", &[], &root()), RiskClass::Destructive);
    }

    #[test]
    fn anything_unrecognised_is_execute_and_never_read() {
        let root = root();
        assert_eq!(classify("execute", &[], &root), RiskClass::Execute);
        assert_eq!(classify("something_new", &[], &root), RiskClass::Execute);
        assert_eq!(classify("fetch", &[], &root), RiskClass::Outbound);
    }

    #[test]
    fn the_policy_answers_for_what_undo_covers_and_nothing_else() {
        assert_eq!(
            settled_by_policy(RiskClass::Read),
            Some(Decision::AllowOnce)
        );
        assert_eq!(
            settled_by_policy(RiskClass::Reversible),
            Some(Decision::AllowOnce)
        );
        for risk in [
            RiskClass::Execute,
            RiskClass::Outbound,
            RiskClass::Destructive,
        ] {
            assert_eq!(
                settled_by_policy(risk),
                None,
                "{risk:?} is not ours to allow"
            );
        }
    }

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
}
