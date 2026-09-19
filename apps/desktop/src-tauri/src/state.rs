//! The application state: one of these, shared by every command (M3-T03).
//!
//! It owns the things that are expensive to make and wrong to make twice — the
//! database, one Journal per open Project, one engine process per Project that
//! has run a turn, and the health-check cache — and it owns the two ways the
//! core reaches the window: the event stream, and the permission queue.
//!
//! Nothing here decides anything. Every rule about what an engine may do, what
//! is checkpointed and what the history says lives in `eavery-core`, where the
//! CLI and the tests reach it too (`docs/plan/03-architecture.md` §1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eavery_core::engine::PermissionHandler;
use eavery_core::error::AppError;
use eavery_core::event::{Decision, ErrorCode, PermissionView};
use eavery_core::journal::{Journal, Watch};
use eavery_core::model::{Project, ProjectId, Settings, TurnId};
use eavery_core::store::{Store, StoredEvent};
use eavery_core::turn::{Approval, Observer, PlanReview, ProjectRunner, TurnCallbacks};
use eavery_engines::health::{HealthCache, HealthOptions};
use eavery_engines::{EngineSpec, Resolver};
use tauri::{AppHandle, Emitter};
use tokio::sync::{broadcast, oneshot};

/// The one event the frontend listens to (`03-architecture.md` §7).
pub const EVENT: &str = "core://event";

/// How many events the in-process stream keeps for a slow subscriber. The
/// store is the real record, so a subscriber that falls behind re-reads rather
/// than losing anything.
const EVENT_BACKLOG: usize = 512;

pub struct AppCore {
    data_dir: PathBuf,
    store: Arc<Store>,
    /// One Journal per open Project. Opening one is cheap after the first
    /// time; the first time takes a checkpoint of the whole folder.
    journals: Mutex<HashMap<ProjectId, Arc<Journal>>>,
    /// One engine process per Project that has run a turn, started on the
    /// first one and kept for the next.
    runners: tokio::sync::Mutex<HashMap<ProjectId, Arc<ProjectRunner>>>,
    engines: HealthCache,
    /// Resolved once: the PATH probe spawns a login shell
    /// (`04-acp-engines.md`, M1-T02).
    resolver: Resolver,
    events: broadcast::Sender<StoredEvent>,
    permissions: Arc<PermissionDesk>,
    plans: Arc<PlanDesk>,
    window: Option<AppHandle>,
}

impl AppCore {
    /// Opens the database in `data_dir`, creating it on first run.
    pub fn open(data_dir: PathBuf, window: Option<AppHandle>) -> Result<Self, AppError> {
        let store = Arc::new(Store::open_in_data_dir(&data_dir)?);
        let (events, _) = broadcast::channel(EVENT_BACKLOG);
        Ok(Self {
            data_dir,
            store,
            journals: Mutex::new(HashMap::new()),
            runners: tokio::sync::Mutex::new(HashMap::new()),
            engines: HealthCache::new(),
            resolver: Resolver::current(),
            events,
            permissions: Arc::new(PermissionDesk::default()),
            plans: Arc::new(PlanDesk::default()),
            window,
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }

    pub fn permissions(&self) -> &PermissionDesk {
        &self.permissions
    }

    /// The plans waiting for an answer. An `Arc`, so a test can play the
    /// turn engine's part with [`PlanDesk::handler`].
    pub fn plans(&self) -> &Arc<PlanDesk> {
        &self.plans
    }

    /// Watches the event stream from inside the process. The window gets the
    /// same events through [`EVENT`]; this is for tests and for anything else
    /// that needs them without a webview.
    pub fn subscribe(&self) -> broadcast::Receiver<StoredEvent> {
        self.events.subscribe()
    }

    pub async fn engine_status(
        &self,
        spec: &'static EngineSpec,
        options: &HealthOptions,
    ) -> eavery_core::model::EngineStatus {
        self.engines.get_or_run(spec, &self.resolver, options).await
    }

    /// Forgets a cached health check, after a sign-in or a "Check again".
    pub async fn forget_engine_status(&self, engine_id: &str) {
        self.engines.invalidate(engine_id).await;
    }

    pub fn project(&self, project_id: ProjectId) -> Result<Project, AppError> {
        self.store
            .project(project_id)?
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "that project is not open"))
    }

    /// The Project's Journal, opened if this is the first time it is asked
    /// for. The first open of a folder takes its first checkpoint, which for a
    /// large folder is slow, so it happens on the blocking pool.
    pub async fn journal(&self, project_id: ProjectId) -> Result<Arc<Journal>, AppError> {
        if let Some(journal) = self.cached_journal(project_id) {
            return Ok(journal);
        }
        let root = self.project(project_id)?.root;
        let journal = self.open_journal(project_id, &root).await?;
        Ok(journal)
    }

    fn cached_journal(&self, project_id: ProjectId) -> Option<Arc<Journal>> {
        self.journals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&project_id)
            .cloned()
    }

    pub async fn open_journal(
        &self,
        project_id: ProjectId,
        root: &Path,
    ) -> Result<Arc<Journal>, AppError> {
        let data_dir = self.data_dir.clone();
        let root = root.to_path_buf();
        let journal = tokio::task::spawn_blocking(move || {
            Journal::open_or_create(project_id, &root, &data_dir, &Watch::default())
        })
        .await
        .map_err(|error| AppError::internal(format!("opening a Journal did not finish: {error}")))?
        .map(Arc::new)?;

        self.journals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(project_id, Arc::clone(&journal));
        Ok(journal)
    }

    /// The Project's engine, started if it is not running yet.
    ///
    /// Starting it is what a first turn does, so the window does not spawn an
    /// engine process for a Project the user only looked at.
    pub async fn runner(&self, project_id: ProjectId) -> Result<Arc<ProjectRunner>, AppError> {
        let mut runners = self.runners.lock().await;
        if let Some(runner) = runners.get(&project_id) {
            return Ok(Arc::clone(runner));
        }

        let project = self.project(project_id)?;
        let engine_id = self.engine_for(&project)?;
        let spec = eavery_engines::find(&engine_id)
            .ok_or_else(|| AppError::new(ErrorCode::EngineUnavailable, "no such assistant"))?;
        let resolved = self
            .resolver
            .resolve(spec)
            .map_err(|error| eavery_engines::health::not_installed(spec, error))
            .map_err(|status| unavailable(spec, &status))?;

        let journal = self.journal(project_id).await?;
        let engine = Arc::new(eavery_acp::AcpEngine::new(
            resolved.launch_spec().cwd(journal.root()),
        ));
        let runner = Arc::new(
            ProjectRunner::open(
                Arc::clone(&self.store),
                journal,
                engine,
                spec.id,
                &spec.facts(),
                // Connectors arrive with M6-T08.
                &eavery_core::policy::ConnectorRegistry::default(),
                self.callbacks(),
            )
            .await?,
        );
        runners.insert(project_id, Arc::clone(&runner));
        Ok(runner)
    }

    /// The Project's engine if it is already running, without starting one.
    pub async fn running_runner(&self, project_id: ProjectId) -> Option<Arc<ProjectRunner>> {
        self.runners.lock().await.get(&project_id).map(Arc::clone)
    }

    /// The runner for the Project a turn is running on, if any.
    pub async fn runner_of_turn(&self, turn_id: TurnId) -> Option<Arc<ProjectRunner>> {
        self.runners
            .lock()
            .await
            .values()
            .find(|runner| runner.running_turn() == Some(turn_id))
            .map(Arc::clone)
    }

    /// Stops every engine process. Called on the way out (M3-T09).
    pub async fn shutdown(&self) {
        let runners: Vec<Arc<ProjectRunner>> =
            self.runners.lock().await.drain().map(|(_, r)| r).collect();
        for runner in runners {
            runner.shutdown().await;
        }
    }

    /// The engine a Project should use: its own, then the default from
    /// Settings, then whichever the table calls first.
    fn engine_for(&self, project: &Project) -> Result<String, AppError> {
        if let Some(engine_id) = &project.engine_id {
            return Ok(engine_id.clone());
        }
        if let Some(engine_id) = self.settings()?.default_engine {
            return Ok(engine_id);
        }
        eavery_engines::visible()
            .next()
            .map(|spec| spec.id.to_owned())
            .ok_or_else(|| {
                AppError::new(ErrorCode::EngineUnavailable, "no assistant is configured")
                    .with_next_action("Choose an assistant in Settings.")
            })
    }

    pub fn settings(&self) -> Result<Settings, AppError> {
        Ok(self.store.setting(Settings::KEY)?.unwrap_or_default())
    }

    pub fn set_settings(&self, settings: &Settings) -> Result<(), AppError> {
        self.store.set_setting(Settings::KEY, settings)?;
        Ok(())
    }

    /// How a turn reaches the window: every event, the questions only a
    /// person can answer, and the plan they have to say yes to.
    fn callbacks(&self) -> TurnCallbacks {
        TurnCallbacks {
            events: self.observer(),
            permission: self.permissions.handler(),
            approval: self.plans.handler(),
        }
    }

    fn observer(&self) -> Observer {
        let window = self.window.clone();
        let events = self.events.clone();
        Arc::new(move |stored: &StoredEvent| {
            // A send with no subscribers is not an error: the store has the
            // event either way, and the window may not be listening yet.
            let _ = events.send(stored.clone());
            if let Some(window) = &window
                && let Err(error) = window.emit(EVENT, stored)
            {
                tracing::error!(%error, "an event did not reach the window");
            }
        })
    }
}

fn unavailable(spec: &EngineSpec, status: &eavery_core::model::EngineStatus) -> AppError {
    AppError::new(
        ErrorCode::EngineUnavailable,
        eavery_engines::health::describe(spec, status),
    )
    .with_next_action("Choose a different assistant in Settings, or install this one.")
}

/// The permission queue: what has been asked, and what is waiting for an
/// answer.
///
/// A request sits here from the moment the engine asks until the person
/// answers it. Nothing ever answers on their behalf — the timeout that
/// protects the engine from a question nobody comes back to lives in
/// `eavery-acp`, and it answers `cancelled`, never `allow`.
#[derive(Default)]
pub struct PermissionDesk {
    waiting: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
}

impl PermissionDesk {
    /// The handler a turn is given. It owns only this desk, so an engine
    /// holding it does not keep the whole application state alive.
    pub fn handler(self: &Arc<Self>) -> PermissionHandler {
        let desk = Arc::clone(self);
        Arc::new(move |view: PermissionView| {
            let desk = Arc::clone(&desk);
            Box::pin(async move { desk.ask(view).await })
        })
    }

    async fn ask(&self, view: PermissionView) -> Decision {
        let (answer, answered) = oneshot::channel();
        self.waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(view.request_id.clone(), answer);

        match answered.await {
            Ok(decision) => decision,
            // The sender was dropped: the window went away, or the desk was
            // cleared. Not an answer, so not a yes.
            Err(_) => Decision::Cancelled,
        }
    }

    /// Answers a waiting request. A request that is not waiting is an error
    /// worth reporting: it means the UI and the engine disagree about what is
    /// pending.
    pub fn answer(&self, request_id: &str, decision: Decision) -> Result<(), AppError> {
        let sender = self
            .waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(request_id);

        match sender {
            Some(sender) => sender
                .send(decision)
                .map_err(|_| AppError::internal("nothing was waiting for that answer any more")),
            None => Err(AppError::new(
                ErrorCode::Internal,
                "that request has already been answered",
            )),
        }
    }

    /// How many requests are waiting. The composer shows this; the tests
    /// assert on it.
    pub fn waiting(&self) -> usize {
        self.waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

/// The plans waiting for an answer, keyed by turn.
///
/// The same shape as [`PermissionDesk`], for the same reason: the turn
/// engine holds the plan phase open until a person answers, and the window
/// answers through a command (`approve_plan`, `reject_plan`) rather than a
/// callback it holds. Nothing here ever answers yes on anyone's behalf: a
/// window that goes away is a no.
#[derive(Default)]
pub struct PlanDesk {
    waiting: Mutex<HashMap<TurnId, oneshot::Sender<Approval>>>,
}

impl PlanDesk {
    pub fn handler(self: &Arc<Self>) -> eavery_core::turn::ApprovalHandler {
        let desk = Arc::clone(self);
        Arc::new(move |review: PlanReview| {
            let desk = Arc::clone(&desk);
            Box::pin(async move { desk.ask(review).await })
        })
    }

    async fn ask(&self, review: PlanReview) -> Approval {
        let (answer, answered) = oneshot::channel();
        self.waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(review.turn_id, answer);

        let approval = match answered.await {
            Ok(approval) => approval,
            Err(_) => Approval::Rejected,
        };
        // Whether it was answered or the wait was abandoned (Stop, or the
        // window going away), the turn is no longer waiting.
        self.waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&review.turn_id);
        approval
    }

    /// Answers a waiting plan. A turn with no plan waiting is an error worth
    /// reporting: the window and the core disagree about where the turn is.
    pub fn answer(&self, turn_id: TurnId, approval: Approval) -> Result<(), AppError> {
        let sender = self
            .waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&turn_id);

        match sender {
            Some(sender) => sender
                .send(approval)
                .map_err(|_| AppError::internal("that plan is no longer waiting for an answer")),
            None => Err(AppError::new(
                ErrorCode::Internal,
                "that turn has no plan waiting for an answer",
            )),
        }
    }

    /// How many plans are waiting.
    pub fn waiting(&self) -> usize {
        self.waiting
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}
