//! The Project commands: open a folder, run a turn in it, look at its history,
//! and go back.
//!
//! These are the first commands that touch someone's real files, so each one
//! goes through the same two things the GUI will: the Journal, which protects
//! the folder, and the store, which remembers what happened
//! (`docs/plan/10-task-breakdown.md` M2-T08).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use eavery_core::journal::{self, Journal, Watch};
use eavery_core::model::{CheckpointId, Project, ProjectId};
use eavery_core::store::Store;
use eavery_core::turn::{self, ProjectRunner, TurnCallbacks, TurnMode};

use crate::render;
use crate::{RunArgs, println_flush};

/// Opens a folder as a Project, or says what is already there.
///
/// The first open is where the folder is counted and the first checkpoint is
/// taken; on a big folder that is the slow part, so it prints as it goes.
pub async fn open(data_dir: &Path, path: &Path, name: Option<&str>) -> Result<ExitCode> {
    let root = eavery_core::paths::canonicalize(path)
        .with_context(|| format!("{} is not a folder", path.display()))?;
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }

    let store = open_store(data_dir)?;
    if let Some(existing) = store.project_by_root(&root)? {
        println_flush(format!("already open as {}", existing.id));
        for line in render::projects_table(std::slice::from_ref(&existing)) {
            println_flush(line);
        }
        return Ok(ExitCode::SUCCESS);
    }

    // The guards from `05-git-journal.md` §4, before anything is created: a
    // folder that is too big to protect is one Eavery refuses rather than one
    // it half-protects.
    let scan = journal::scan_project(&root).context("counting the folder")?;
    println_flush(format!(
        "folder   {} files, {}",
        scan.files,
        render::bytes(scan.bytes)
    ));
    if scan.too_many_files() {
        bail!(
            "{} has {} files, and Eavery protects up to {}. Open a subfolder instead.",
            root.display(),
            scan.files,
            journal::MAX_FILES
        );
    }
    if scan.is_large() {
        println_flush(format!(
            "note     this folder is over {}; the first checkpoint will take a while",
            render::bytes(journal::WARN_TOTAL_BYTES)
        ));
    }

    let project = Project {
        id: uuid::Uuid::new_v4(),
        name: name
            .map(str::to_owned)
            .or_else(|| {
                root.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| root.display().to_string()),
        root: root.clone(),
        created_at: chrono::Utc::now(),
        engine_id: None,
    };
    store.insert_project(&project)?;

    let journal = open_journal(data_dir, &project, watching())?;
    let checkpoints = turn::sync_checkpoints(&store, Arc::clone(&journal), 10).await?;

    println_flush(format!("project  {}", project.id));
    println_flush(format!(
        "history  {} bytes on disk",
        journal.size_on_disk()?
    ));
    for line in render::checkpoints_table(&checkpoints) {
        println_flush(line);
    }
    for line in render::unprotected(&journal.unprotected()?) {
        println_flush(line);
    }
    Ok(ExitCode::SUCCESS)
}

pub fn list(data_dir: &Path) -> Result<ExitCode> {
    let store = open_store(data_dir)?;
    let projects = store.list_projects()?;
    if projects.is_empty() {
        println_flush("no projects yet: `eavery-cli project open <folder>`");
        return Ok(ExitCode::SUCCESS);
    }
    for line in render::projects_table(&projects) {
        println_flush(line);
    }
    Ok(ExitCode::SUCCESS)
}

/// One turn, end to end, against a real engine.
pub async fn run(data_dir: &Path, args: &RunArgs) -> Result<ExitCode> {
    let store = open_store(data_dir)?;
    let project = find(&store, &args.project)?;
    let journal = open_journal(data_dir, &project, Watch::default())?;

    let engine = Arc::new(eavery_acp::AcpEngine::new(crate::launch_spec(
        &args.engine,
        &args.launch,
        &project.root,
    )?));

    let spec = eavery_engines::find(&args.engine)
        .with_context(|| format!("there is no engine called {}", args.engine))?;
    let runner = ProjectRunner::open(
        Arc::new(store),
        journal,
        engine,
        &args.engine,
        &spec.facts(),
        // Connectors arrive with M6-T08.
        &eavery_core::policy::ConnectorRegistry::default(),
        callbacks(
            args.answer.clone(),
            args.approve.clone(),
            args.edits.clone(),
        ),
    )
    .await?;

    // The turn prints itself: every event reaches the callback above in the
    // order it happened, including the permission questions and the plan.
    let mode = if args.plan {
        TurnMode::Plan
    } else if args.ask {
        TurnMode::Ask
    } else {
        TurnMode::Direct
    };
    let outcome = runner.run_turn_in(mode, &args.request).await;
    runner.shutdown().await;

    match outcome {
        Ok(outcome) => {
            for line in render::digest(&outcome.digest) {
                println_flush(line);
            }
            Ok(match outcome.stop_reason {
                eavery_core::engine::StopReason::Cancelled => ExitCode::from(130),
                _ => ExitCode::SUCCESS,
            })
        }
        Err(error) => {
            println_flush(format!("error    {error}"));
            if let Some(next) = error.next_action() {
                println_flush(format!("next     {next}"));
            }
            Ok(ExitCode::FAILURE)
        }
    }
}

/// What has happened to this folder, newest first.
pub async fn history(data_dir: &Path, project: &str, limit: usize) -> Result<ExitCode> {
    let store = open_store(data_dir)?;
    let project = find(&store, project)?;
    let journal = open_journal(data_dir, &project, Watch::default())?;

    // The Journal is the truth; the store's rows are a cache of it.
    let checkpoints = turn::sync_checkpoints(&store, Arc::clone(&journal), limit).await?;
    if checkpoints.is_empty() {
        println_flush("no checkpoints yet");
        return Ok(ExitCode::SUCCESS);
    }
    for line in render::checkpoints_table(&checkpoints) {
        println_flush(line);
    }
    Ok(ExitCode::SUCCESS)
}

/// Goes back. With no `--to`, back to the point before the last turn.
pub async fn undo(data_dir: &Path, project: &str, to: Option<&str>) -> Result<ExitCode> {
    let store = open_store(data_dir)?;
    let project = find(&store, project)?;
    let journal = open_journal(data_dir, &project, Watch::default())?;

    let target: CheckpointId = match to {
        Some(id) => resolve_checkpoint(&store, Arc::clone(&journal), &project.id, id).await?,
        None => last_pre_turn_checkpoint(&store, &project.id)?,
    };

    let outcome = turn::restore_without_session(&store, Arc::clone(&journal), &target).await?;

    println_flush(format!("back to  {}", render::short(&target)));
    println_flush(format!(
        "now at   {}",
        render::short(&outcome.checkpoint.id)
    ));
    if outcome.skipped_locked.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    // Reported, never silently skipped: a partial restore the user does not
    // know about is worse than one that failed.
    println_flush(format!(
        "locked   {} file(s) were open and were left alone:",
        outcome.skipped_locked.len()
    ));
    for path in &outcome.skipped_locked {
        println_flush(format!("           {}", path.display()));
    }
    println_flush("         Close them and run undo again.");
    Ok(ExitCode::FAILURE)
}

/// What changed between two checkpoints, or since one.
pub async fn diff(
    data_dir: &Path,
    project: &str,
    from: &str,
    to: Option<&str>,
) -> Result<ExitCode> {
    let store = open_store(data_dir)?;
    let project = find(&store, project)?;
    let journal = open_journal(data_dir, &project, Watch::default())?;

    let from = resolve_checkpoint(&store, Arc::clone(&journal), &project.id, from).await?;
    let changes = match to {
        Some(to) => {
            let to = resolve_checkpoint(&store, Arc::clone(&journal), &project.id, to).await?;
            journal.diff(&from, &to)?
        }
        // Nothing to compare with means "and what has happened since", which
        // includes the user's own edits.
        None => journal.diff_worktree(&from)?,
    };

    for line in render::change_set(&changes) {
        println_flush(line);
    }
    Ok(ExitCode::SUCCESS)
}

// ---- shared plumbing -------------------------------------------------------

fn open_store(data_dir: &Path) -> Result<Store> {
    Store::open_in_data_dir(data_dir)
        .with_context(|| format!("opening the database in {}", data_dir.display()))
}

fn open_journal(data_dir: &Path, project: &Project, watch: Watch) -> Result<Arc<Journal>> {
    Ok(Arc::new(
        Journal::open_or_create(project.id, &project.root, data_dir, &watch)
            .with_context(|| format!("opening the history of {}", project.root.display()))?,
    ))
}

/// Prints the first checkpoint as it is taken. On a folder of Word documents
/// this is the only sign that anything is happening.
fn watching() -> Watch {
    Watch::default().on_progress(|path, count| {
        if count % 100 == 0 {
            println_flush(format!("protect  {count} files… {}", path.display()));
        }
    })
}

/// A Project by id or by folder. Typing the folder is what anyone actually
/// has to hand.
fn find(store: &Store, wanted: &str) -> Result<Project> {
    if let Ok(id) = uuid::Uuid::parse_str(wanted)
        && let Some(project) = store.project(id)?
    {
        return Ok(project);
    }
    if let Ok(root) = eavery_core::paths::canonicalize(wanted)
        && let Some(project) = store.project_by_root(&root)?
    {
        return Ok(project);
    }

    let known: Vec<String> = store
        .list_projects()?
        .into_iter()
        .map(|project| format!("  {}  {}", project.id, project.root.display()))
        .collect();
    if known.is_empty() {
        bail!("no project matches `{wanted}`, and none are open yet");
    }
    bail!(
        "no project matches `{wanted}`. Open projects:\n{}",
        known.join("\n")
    )
}

/// A checkpoint by full id or by the short form the tables print.
async fn resolve_checkpoint(
    store: &Store,
    journal: Arc<Journal>,
    project_id: &ProjectId,
    wanted: &str,
) -> Result<CheckpointId> {
    if store.checkpoint(&wanted.to_owned())?.is_some() {
        return Ok(wanted.to_owned());
    }
    let checkpoints = turn::sync_checkpoints(store, journal, 200).await?;
    let matching: Vec<&eavery_core::model::Checkpoint> = checkpoints
        .iter()
        .filter(|checkpoint| {
            &checkpoint.project_id == project_id && checkpoint.id.starts_with(wanted)
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => bail!("no checkpoint starting `{wanted}`; `eavery-cli history` lists them"),
        many => bail!(
            "`{wanted}` matches {} checkpoints; type more of it",
            many.len()
        ),
    }
}

/// The point before the last turn: what Undo means when nobody says otherwise.
fn last_pre_turn_checkpoint(store: &Store, project_id: &ProjectId) -> Result<CheckpointId> {
    for session in store.sessions_for_project(*project_id)? {
        if let Some(checkpoint) = store
            .turns_for_session(session.id)?
            .into_iter()
            .rev()
            .find_map(|turn| turn.pre_checkpoint)
        {
            return Ok(checkpoint);
        }
    }
    bail!("nothing to undo: no turn has run in this project yet. Use --to to pick a checkpoint.")
}

/// Prints the turn as it happens, and asks the terminal about anything the
/// policy will not decide — and about the plan.
fn callbacks(
    fixed: Option<String>,
    approve: Option<String>,
    edits: Option<String>,
) -> TurnCallbacks {
    TurnCallbacks {
        // Called in order, from the task running the turn, so printing
        // straight out keeps the transcript in the order it happened.
        events: Arc::new(|stored| {
            if let Some(line) = render::core_event(&stored.event) {
                println_flush(line);
            }
        }),
        permission: Arc::new(move |view| {
            let fixed = fixed.clone();
            Box::pin(async move { crate::answer_permission(&view, fixed.as_deref()).await })
        }),
        approval: Arc::new(move |_review| {
            let approve = approve.clone();
            let edits = edits.clone();
            Box::pin(async move {
                let answer = crate::answer_plan(approve.as_deref(), edits.as_deref()).await;
                println_flush(render::plan_answered(&answer));
                answer
            })
        }),
    }
}

/// Where Eavery keeps its database, its journals and its logs
/// (`docs/plan/03-architecture.md` §8).
pub fn data_dir(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }
    // The tests and the manual milestone runs need a folder that is not the
    // real one; this is how they say so.
    if let Some(from_env) = std::env::var_os("EAVERY_DATA_DIR") {
        return Ok(PathBuf::from(from_env));
    }
    let dirs = directories::ProjectDirs::from("dev", "eavery", "Eavery")
        .context("this system has no home directory; pass --data-dir")?;
    Ok(dirs.data_dir().to_path_buf())
}
