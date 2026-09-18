//! The commands the frontend calls (`docs/plan/03-architecture.md` §7).
//!
//! Every one of them is a thin call into `eavery-core`. There is no logic
//! here on purpose: a rule that lived in a command would be a rule the CLI
//! and the tests could not reach, and this is the layer with no test harness
//! worth the name.
//!
//! Errors come back as `AppError` — a code, a message, and the next action —
//! so a failure reaches the person as something to do about it.

use eavery_core::diagnostics::{self, Diagnostics};
use eavery_core::error::AppError;
use eavery_core::event::{Decision, ErrorCode};
use eavery_core::journal::{self, ChangeSet, JournalInfo, Unprotected};
use eavery_core::model::{
    Checkpoint, CheckpointId, EngineListing, EngineStatus, Project, ProjectId, Session, SessionId,
    Settings, Turn, TurnId,
};
use eavery_core::store::{AuditEntry, StoredEvent};
use eavery_core::turn::{Approval, RestoreOutcome, TurnMode};
use eavery_engines::health::HealthOptions;
use tauri::State;

use crate::state::AppCore;

// ---- projects --------------------------------------------------------------

#[tauri::command]
pub fn list_projects(core: State<'_, AppCore>) -> Result<Vec<Project>, AppError> {
    Ok(core.store().list_projects()?)
}

/// Opens a folder as a Project, protecting it from now on.
///
/// The guards come first: a folder with more files than the Journal can carry
/// is refused rather than half-protected (`docs/plan/05-git-journal.md` §4).
#[tauri::command]
pub async fn open_project(
    core: State<'_, AppCore>,
    path: std::path::PathBuf,
) -> Result<Project, AppError> {
    let root = eavery_core::paths::canonicalize(&path).map_err(|error| {
        AppError::new(ErrorCode::Internal, format!("{}: {error}", path.display()))
            .with_next_action("Choose a folder that still exists.")
    })?;

    if let Some(project) = core.store().project_by_root(&root)? {
        // Already open. Make sure its Journal is there and hand it back.
        core.journal(project.id).await?;
        return Ok(project);
    }

    let scan = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || journal::scan_project(&root))
            .await
            .map_err(|error| AppError::internal(format!("counting the folder: {error}")))??
    };
    if scan.too_many_files() {
        return Err(AppError::new(
            ErrorCode::ProjectTooLarge,
            format!(
                "this folder has {} files, and Eavery protects up to {}",
                scan.files,
                journal::MAX_FILES
            ),
        )
        .with_next_action("Open one of the folders inside it instead."));
    }

    let project = Project {
        id: uuid::Uuid::new_v4(),
        name: root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string()),
        root: root.clone(),
        created_at: chrono::Utc::now(),
        engine_id: None,
    };
    core.store().insert_project(&project)?;
    core.open_journal(project.id, &root).await?;
    Ok(project)
}

/// Forgets a Project. Deletes neither the user's files nor their history:
/// opening the same folder again brings all of it back.
#[tauri::command]
pub fn remove_project(core: State<'_, AppCore>, project_id: ProjectId) -> Result<(), AppError> {
    core.store().remove_project(project_id)?;
    Ok(())
}

#[tauri::command]
pub fn set_project_engine(
    core: State<'_, AppCore>,
    project_id: ProjectId,
    engine_id: String,
) -> Result<(), AppError> {
    if eavery_engines::find(&engine_id).is_none() {
        return Err(AppError::new(
            ErrorCode::EngineUnavailable,
            format!("there is no assistant called {engine_id}"),
        ));
    }
    core.store()
        .set_project_engine(project_id, Some(&engine_id))?;
    Ok(())
}

// ---- engines ---------------------------------------------------------------

/// Every assistant Eavery knows about, and whether it would work right now.
///
/// Answers are cached for ten minutes (`04-acp-engines.md` §9): opening
/// Settings must not spawn every engine on the machine.
#[tauri::command]
pub async fn list_engines(
    core: State<'_, AppCore>,
    all: Option<bool>,
) -> Result<Vec<EngineListing>, AppError> {
    let all = all.unwrap_or(false);
    let mut listings = Vec::new();
    for spec in eavery_engines::ENGINES.iter() {
        if !all && (!spec.visible() || spec.experimental) {
            continue;
        }
        let program = core
            .resolver()
            .resolve(spec)
            .ok()
            .map(|resolved| resolved.program.display().to_string());
        listings.push(EngineListing {
            id: spec.id.to_owned(),
            display_name: spec.display_name.to_owned(),
            vendor: spec.vendor.to_owned(),
            experimental: spec.experimental,
            status: core.engine_status(spec, &HealthOptions::default()).await,
            program,
        });
    }
    Ok(listings)
}

/// Checks one assistant now. `deep` sends a real prompt, which costs the user
/// a request and a wait, so it is never the default.
#[tauri::command]
pub async fn run_health_check(
    core: State<'_, AppCore>,
    engine_id: String,
    deep: Option<bool>,
) -> Result<EngineStatus, AppError> {
    let spec = eavery_engines::find(&engine_id).ok_or_else(|| {
        AppError::new(
            ErrorCode::EngineUnavailable,
            format!("there is no assistant called {engine_id}"),
        )
    })?;
    // Asked for on purpose, so the cached answer is not what they wanted.
    core.forget_engine_status(spec.id).await;
    let options = if deep.unwrap_or(false) {
        HealthOptions::deep()
    } else {
        HealthOptions::default()
    };
    Ok(core.engine_status(spec, &options).await)
}

// ---- turns -----------------------------------------------------------------

/// Starts a turn and returns as soon as it has an id; the turn goes on in a
/// task of its own and reports itself through `core://event`.
///
/// `mode` is `plan` (the plan gate: plan, approve, execute) or `direct`
/// (`docs/plan/06-plan-gate-permissions.md` §1 and §5); left out, it is
/// direct. A plan-mode turn stops at `AwaitingApproval` and waits for
/// [`approve_plan`] or [`reject_plan`].
///
/// The Project is claimed before this returns, so "the assistant is already
/// working" is the answer to this call rather than an error arriving from
/// nowhere a moment later (C13).
#[tauri::command]
pub async fn start_turn(
    core: State<'_, AppCore>,
    project_id: ProjectId,
    request: String,
    mode: Option<TurnMode>,
) -> Result<TurnId, AppError> {
    let mode = mode.unwrap_or_default();
    let runner = core.runner(project_id).await?;
    let ticket = runner.claim_turn()?;
    let turn_id = ticket.turn_id();

    tokio::spawn(async move {
        // Everything the turn does — including anything that goes wrong — is
        // already an event by the time this returns, so there is nothing to
        // hand back to.
        if let Err(error) = runner.run_claimed(ticket, mode, &request).await {
            tracing::warn!(%error, %turn_id, "the turn did not finish");
        }
    });
    Ok(turn_id)
}

/// Go ahead with the plan, with `edits` when the person added any (§2.4,
/// "approve with changes"). Approval is explicit and only ever comes from
/// here: nothing times out into a yes.
#[tauri::command]
pub fn approve_plan(
    core: State<'_, AppCore>,
    turn_id: TurnId,
    edits: Option<String>,
) -> Result<(), AppError> {
    core.plans().answer(turn_id, Approval::Approved { edits })
}

/// Not now. The turn ends `Cancelled` and nothing is executed.
#[tauri::command]
pub fn reject_plan(core: State<'_, AppCore>, turn_id: TurnId) -> Result<(), AppError> {
    core.plans().answer(turn_id, Approval::Rejected)
}

/// Answers a permission request the person was asked about.
#[tauri::command]
pub fn answer_permission(
    core: State<'_, AppCore>,
    request_id: String,
    decision: Decision,
) -> Result<(), AppError> {
    core.permissions().answer(&request_id, decision)
}

/// Stop. The turn ends where it is, and its post-turn checkpoint is still
/// taken, so what it did is still undoable.
#[tauri::command]
pub async fn cancel_turn(core: State<'_, AppCore>, turn_id: TurnId) -> Result<(), AppError> {
    match core.runner_of_turn(turn_id).await {
        Some(runner) => Ok(runner.cancel().await?),
        // Already finished. Nothing to stop is not a failure.
        None => Ok(()),
    }
}

#[tauri::command]
pub fn list_turns(core: State<'_, AppCore>, session_id: SessionId) -> Result<Vec<Turn>, AppError> {
    Ok(core.store().turns_for_session(session_id)?)
}

// ---- history ---------------------------------------------------------------

/// The Project's checkpoints, newest first, refreshed from the Journal —
/// which is the one that knows; the database only caches it.
#[tauri::command]
pub async fn list_checkpoints(
    core: State<'_, AppCore>,
    project_id: ProjectId,
    limit: Option<usize>,
) -> Result<Vec<Checkpoint>, AppError> {
    let journal = core.journal(project_id).await?;
    Ok(eavery_core::turn::sync_checkpoints(core.store(), journal, limit.unwrap_or(50)).await?)
}

/// Takes a checkpoint because the person asked for one.
///
/// Through the runner when an engine is already going, so it lands in the
/// transcript; otherwise straight at the Journal. "Save a point I can come
/// back to" is not an assistant's doing and must not start one.
#[tauri::command]
pub async fn checkpoint_now(
    core: State<'_, AppCore>,
    project_id: ProjectId,
    label: String,
) -> Result<Checkpoint, AppError> {
    if let Some(runner) = core.running_runner(project_id).await {
        return Ok(runner.checkpoint_now(&label).await?);
    }
    let journal = core.journal(project_id).await?;
    Ok(eavery_core::turn::checkpoint_without_session(core.store(), journal, &label).await?)
}

/// Goes back. Refused while a turn is running on that Project (C13).
///
/// The files something else held open come back in the answer, never dropped:
/// a partial restore the user does not know about is worse than one that
/// failed.
#[tauri::command]
pub async fn restore_checkpoint(
    core: State<'_, AppCore>,
    project_id: ProjectId,
    checkpoint_id: CheckpointId,
) -> Result<RestoreOutcome, AppError> {
    // Through the runner when one is running, so the restore lands in the
    // transcript; otherwise straight at the Journal, because Undo must work
    // whether or not an assistant was ever started.
    if let Some(runner) = core.running_runner(project_id).await {
        return Ok(runner.restore(&checkpoint_id).await?);
    }
    let journal = core.journal(project_id).await?;
    Ok(eavery_core::turn::restore_without_session(core.store(), journal, &checkpoint_id).await?)
}

/// What changed between two checkpoints, or since one.
#[tauri::command]
pub async fn diff_summary(
    core: State<'_, AppCore>,
    project_id: ProjectId,
    from: CheckpointId,
    to: Option<CheckpointId>,
) -> Result<ChangeSet, AppError> {
    let journal = core.journal(project_id).await?;
    let changes = tokio::task::spawn_blocking(move || match to {
        Some(to) => journal.diff(&from, &to),
        None => journal.diff_worktree(&from),
    })
    .await
    .map_err(|error| AppError::internal(format!("working out the changes: {error}")))??;
    Ok(changes)
}

/// The transcript. `after` is the last `seq` the frontend has, so a window
/// that missed events asks for exactly what it missed.
#[tauri::command]
pub fn list_events(
    core: State<'_, AppCore>,
    session_id: SessionId,
    after: Option<u64>,
    limit: Option<usize>,
) -> Result<Vec<StoredEvent>, AppError> {
    Ok(core.store().list_events(session_id, after, limit)?)
}

#[tauri::command]
pub fn list_sessions(
    core: State<'_, AppCore>,
    project_id: ProjectId,
) -> Result<Vec<Session>, AppError> {
    Ok(core.store().sessions_for_project(project_id)?)
}

/// Every decision taken, newest first. Developer mode shows it; it is
/// append-only, and nothing in the app can rewrite it.
#[tauri::command]
pub fn list_audit(
    core: State<'_, AppCore>,
    project_id: Option<ProjectId>,
    limit: Option<usize>,
) -> Result<Vec<AuditEntry>, AppError> {
    Ok(core.store().list_audit(project_id, limit)?)
}

/// How much room the Project's history takes up.
#[tauri::command]
pub async fn journal_size(
    core: State<'_, AppCore>,
    project_id: ProjectId,
) -> Result<u64, AppError> {
    let journal = core.journal(project_id).await?;
    Ok(tokio::task::spawn_blocking(move || journal.size_on_disk())
        .await
        .map_err(|error| AppError::internal(format!("measuring the history: {error}")))??)
}

/// Where the Project's history is kept and how big it has got. Developer
/// mode shows it; Everyday mode never names a git directory.
#[tauri::command]
pub async fn journal_info(
    core: State<'_, AppCore>,
    project_id: ProjectId,
) -> Result<JournalInfo, AppError> {
    let journal = core.journal(project_id).await?;
    Ok(tokio::task::spawn_blocking(move || journal.info())
        .await
        .map_err(|error| AppError::internal(format!("measuring the history: {error}")))??)
}

/// Everything in the Project that Undo does not cover, and why. This is what
/// makes "your files are protected" a claim Eavery can always qualify.
#[tauri::command]
pub async fn unprotected_files(
    core: State<'_, AppCore>,
    project_id: ProjectId,
) -> Result<Vec<Unprotected>, AppError> {
    let journal = core.journal(project_id).await?;
    Ok(tokio::task::spawn_blocking(move || journal.unprotected())
        .await
        .map_err(|error| AppError::internal(format!("checking the folder: {error}")))??)
}

// ---- settings --------------------------------------------------------------

#[tauri::command]
pub fn get_settings(core: State<'_, AppCore>) -> Result<Settings, AppError> {
    core.settings()
}

#[tauri::command]
pub fn set_settings(core: State<'_, AppCore>, settings: Settings) -> Result<(), AppError> {
    core.set_settings(&settings)
}

// ---- diagnostics -----------------------------------------------------------

/// What the Diagnostics panel shows (`03-architecture.md` §9): the version,
/// where the data and the log live, and the log's tail. Reading it never
/// fails — the panel is for when something already has.
#[tauri::command]
pub async fn diagnostics(
    core: State<'_, AppCore>,
    lines: Option<usize>,
) -> Result<Diagnostics, AppError> {
    let data_dir = core.data_dir().to_path_buf();
    let lines = lines.unwrap_or(diagnostics::DEFAULT_TAIL_LINES);
    tokio::task::spawn_blocking(move || {
        Diagnostics::read(&data_dir, env!("CARGO_PKG_VERSION"), lines)
    })
    .await
    .map_err(|error| AppError::internal(format!("reading the log: {error}")))
}
