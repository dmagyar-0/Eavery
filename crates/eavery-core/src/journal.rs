//! The Journal: checkpoints and Undo, on libgit2 (`docs/plan/05-git-journal.md`).
//!
//! This is the licence to operate on someone's real files. It must be correct
//! before it is fast, and it must never be skippable.
//!
//! Two properties are worth stating up front, because everything else follows
//! from them:
//!
//! - **The Project folder never contains a `.git`.** The git directory lives
//!   under Eavery's data directory, with `core.worktree` pointing back at the
//!   Project. A user who has their own git repository in that folder is
//!   untouched by any of this.
//! - **History only moves forward.** A restore writes an old tree into the
//!   work tree as a *new* commit, after committing whatever is there now, so
//!   nothing a user did — including edits Eavery never saw — can be lost by
//!   going back.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{Checkpoint, CheckpointId, CheckpointKind, ProjectId, TurnId};

/// Files above this are not checkpointed: hashing them on every turn would
/// cost more than the protection is worth. They are listed as unprotected
/// rather than silently dropped.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Above this, `open_project` asks the user to pick a subfolder first.
pub const WARN_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Above this, `open_project` refuses.
pub const MAX_FILES: usize = 200_000;

/// Text diffs are produced up to this size. Past it the file is still listed
/// as changed; it is only the patch that is left out, because nobody reads a
/// three megabyte diff and building one costs more than the page it fills.
pub const MAX_TEXT_DIFF_BYTES: u64 = 256 * 1024;

/// The one branch. There is no remote, and nothing else writes here.
pub const BRANCH: &str = "eavery";
const BRANCH_REF: &str = "refs/heads/eavery";

/// Every commit is Eavery's, whoever the user is. The Journal is not their
/// git history and must never look like it.
const AUTHOR_NAME: &str = "Eavery";
const AUTHOR_EMAIL: &str = "eavery@localhost";

const KIND_TRAILER: &str = "Eavery-Kind:";
const TURN_TRAILER: &str = "Eavery-Turn:";

/// `docs/plan/05-git-journal.md` §3. Written on every open, because a rule
/// added in a later version has to reach Projects opened by an earlier one.
const EXCLUDE: &str = "\
# Written by Eavery on every open. Edits here are lost.
~$*
.~lock.*#
.DS_Store
Thumbs.db
desktop.ini
*.tmp
*.crdownload
*.partial
*.eavery-tmp
node_modules/
.git/
.claude/
.codex/
.goose/
.gemini/
";

/// Why a file is not protected by Undo. Shown in the "Not protected" panel,
/// which exists so that "your files are protected" is never a claim Eavery
/// cannot back up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum UnprotectedReason {
    /// Over [`MAX_FILE_BYTES`].
    TooLarge { bytes: u64 },
    /// A cloud placeholder. Reading it would pull the whole file down from the
    /// provider, so it is skipped without being opened.
    NotDownloaded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Unprotected {
    /// Relative to the Project root.
    pub path: PathBuf,
    #[serde(flatten)]
    pub reason: UnprotectedReason,
}

/// What changed between two points.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ChangeSet {
    pub added: Vec<PathBuf>,
    pub changed: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
    /// Unified diffs, for text files only and only under 256 KB. A binary file
    /// appears in the lists above and nowhere else.
    pub text_diffs: Vec<(PathBuf, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error("the Journal could not be opened: {0}")]
    Git(#[from] git2::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no checkpoint {0}")]
    NoSuchCheckpoint(String),
    #[error("protecting this Project was cancelled")]
    Cancelled,
}

impl JournalError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        JournalError::Io {
            path: path.into(),
            source,
        }
    }

    /// What the user can do about it (`05-git-journal.md` §5). A checkpoint
    /// that fails stops the turn, so this text is the whole of what they get.
    pub fn next_action(&self) -> Option<String> {
        let JournalError::Io { source, .. } = self else {
            return None;
        };
        Some(match classify(source) {
            IoTrouble::Locked => {
                "Close any files from this Project that are open in Word or Excel, \
                 then try again."
                    .to_owned()
            }
            IoTrouble::Permission => {
                "Eavery cannot write to this folder. Choose a folder you own.".to_owned()
            }
            IoTrouble::DiskFull => {
                "There is not enough disk space to protect this Project.".to_owned()
            }
            IoTrouble::Other => return None,
        })
    }
}

enum IoTrouble {
    /// Something else holds the file open. On Windows this is the common case
    /// and it is not an error the user can be blamed for.
    Locked,
    Permission,
    DiskFull,
    Other,
}

fn classify(error: &std::io::Error) -> IoTrouble {
    // Windows: 32 is ERROR_SHARING_VIOLATION, 33 ERROR_LOCK_VIOLATION. Excel
    // holds both while a workbook is open.
    if matches!(error.raw_os_error(), Some(32) | Some(33)) {
        return IoTrouble::Locked;
    }
    match error.kind() {
        // On Unix a locked file is not a thing; a file Eavery may not write is.
        std::io::ErrorKind::PermissionDenied => IoTrouble::Permission,
        std::io::ErrorKind::StorageFull => IoTrouble::DiskFull,
        _ => IoTrouble::Other,
    }
}

/// How far a long checkpoint has got, and whether it should stop.
///
/// The first checkpoint of a big folder takes real time, and the plan requires
/// it to be watchable and cancellable: `open_project` returns only when it
/// finishes or is cancelled (`05-git-journal.md` §4).
#[derive(Clone, Default)]
pub struct Watch {
    #[allow(clippy::type_complexity)]
    progress: Option<Arc<dyn Fn(&Path, usize) + Send + Sync>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("progress", &self.progress.is_some())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

impl Watch {
    /// Called with each file as it is staged, and how many have been staged so
    /// far.
    #[must_use]
    pub fn on_progress(mut self, progress: impl Fn(&Path, usize) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(progress));
        self
    }

    #[must_use]
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
}

/// One Project's history.
pub struct Journal {
    /// `git2::Repository` is `Send` but not `Sync`, and every method here takes
    /// `&self` because the turn engine holds one Journal per Project and calls
    /// it from whichever task is running.
    repo: std::sync::Mutex<git2::Repository>,
    root: PathBuf,
    git_dir: PathBuf,
    project_id: ProjectId,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal")
            .field("project_id", &self.project_id)
            .field("root", &self.root)
            .finish()
    }
}

impl Journal {
    /// The git directory for a Project: `<data_dir>/journals/<project-id>/`.
    pub fn git_dir(data_dir: &Path, project_id: ProjectId) -> PathBuf {
        data_dir.join("journals").join(project_id.to_string())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_id(&self) -> ProjectId {
        self.project_id
    }

    /// Opens the Project's Journal, creating it on first use.
    ///
    /// Creating one takes the first checkpoint, which for a large folder is
    /// the slow part; pass a [`Watch`] to show progress and to allow a cancel.
    /// Opening an existing one is cheap and never checkpoints.
    pub fn open_or_create(
        project_id: ProjectId,
        root: &Path,
        data_dir: &Path,
        watch: &Watch,
    ) -> Result<Self, JournalError> {
        let root = crate::paths::canonical_or_self(root);
        let git_dir = Self::git_dir(data_dir, project_id);

        let existed = git_dir.join("HEAD").exists();
        let repo = if existed {
            let repo = git2::Repository::open(&git_dir)?;
            // The Project may have been moved since it was last opened. The
            // Journal follows it: the history is about the files, not the path.
            if repo.workdir().map(crate::paths::canonical_or_self) != Some(root.clone()) {
                tracing::info!(
                    project = %project_id,
                    from = ?repo.workdir(),
                    to = %root.display(),
                    "the Project moved; pointing its Journal at the new folder"
                );
                attach_worktree(&repo, &root)?;
            }
            repo
        } else {
            std::fs::create_dir_all(&git_dir).map_err(|error| JournalError::io(&git_dir, error))?;
            let mut options = git2::RepositoryInitOptions::new();
            options
                // Without this the git directory would be `<git_dir>/.git`.
                .no_dotgit_dir(true)
                // Initialised without a work tree, and given one below.
                .bare(true)
                .mkpath(true)
                .initial_head(BRANCH);
            let repo = git2::Repository::init_opts(&git_dir, &options)?;
            attach_worktree(&repo, &root)?;
            // Reopened so the handle picks up the work tree from the config it
            // was just given, exactly as every later open will.
            drop(repo);
            git2::Repository::open(&git_dir)?
        };

        let journal = Journal {
            repo: std::sync::Mutex::new(repo),
            root,
            git_dir,
            project_id,
        };
        journal.write_excludes()?;

        if !existed {
            journal.checkpoint_watched(
                "Project opened",
                CheckpointKind::Manual,
                None,
                true,
                watch,
            )?;
        }
        Ok(journal)
    }

    /// (Re)writes `info/exclude`. Idempotent, and done on every open so a rule
    /// added later reaches Projects opened earlier.
    fn write_excludes(&self) -> Result<(), JournalError> {
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info).map_err(|error| JournalError::io(&info, error))?;
        let exclude = info.join("exclude");
        if std::fs::read_to_string(&exclude).ok().as_deref() == Some(EXCLUDE) {
            return Ok(());
        }
        std::fs::write(&exclude, EXCLUDE).map_err(|error| JournalError::io(&exclude, error))
    }

    /// Stages the work tree and commits it.
    ///
    /// With `force = false` and nothing changed, the existing HEAD checkpoint
    /// comes back instead of an empty commit: a turn that changed nothing says
    /// so, rather than leaving a history of identical points to go back to.
    pub fn checkpoint(
        &self,
        label: &str,
        kind: CheckpointKind,
        turn_id: Option<TurnId>,
        force: bool,
    ) -> Result<Checkpoint, JournalError> {
        self.checkpoint_watched(label, kind, turn_id, force, &Watch::default())
    }

    fn checkpoint_watched(
        &self,
        label: &str,
        kind: CheckpointKind,
        turn_id: Option<TurnId>,
        force: bool,
        watch: &Watch,
    ) -> Result<Checkpoint, JournalError> {
        let started = std::time::Instant::now();
        let repo = self.repo.lock().expect("journal lock");

        let (tree_id, _skipped) = self.stage(&repo, watch)?;
        if watch.is_cancelled() {
            return Err(JournalError::Cancelled);
        }

        let head = head_commit(&repo)?;
        if !force && head.as_ref().map(git2::Commit::tree_id) == Some(tree_id) {
            // Nothing changed. The caller gets the point they are already at.
            let head = head.expect("a matching tree means there is a HEAD");
            return self.describe(&repo, &head);
        }

        let signature = git2::Signature::now(AUTHOR_NAME, AUTHOR_EMAIL)?;
        let tree = repo.find_tree(tree_id)?;
        let parents: Vec<&git2::Commit> = head.iter().collect();
        let message = message_with_trailers(label, kind, turn_id);
        let oid = repo.commit(
            Some(BRANCH_REF),
            &signature,
            &signature,
            &message,
            &tree,
            &parents,
        )?;

        let elapsed = started.elapsed();
        if elapsed > std::time::Duration::from_secs(10) {
            tracing::warn!(
                project = %self.project_id,
                seconds = elapsed.as_secs(),
                "a checkpoint took a long time"
            );
        }

        let commit = repo.find_commit(oid)?;
        self.describe(&repo, &commit)
    }

    /// Rebuilds the index from the work tree and writes its tree.
    ///
    /// The index is never trusted between calls: an engine, a sync client and
    /// the user all write to this folder, and a stale index would checkpoint a
    /// state that never existed.
    fn stage(
        &self,
        repo: &git2::Repository,
        watch: &Watch,
    ) -> Result<(git2::Oid, Vec<Unprotected>), JournalError> {
        let mut index = repo.index()?;
        let root = self.root.clone();
        let mut skipped: Vec<Unprotected> = Vec::new();
        let mut staged = 0usize;

        {
            let mut filter = |path: &Path, _matched: &[u8]| -> i32 {
                if watch.is_cancelled() {
                    // `add_all` has no abort that leaves the index usable, so a
                    // cancel skips the rest and the caller throws the index
                    // away without writing it.
                    return 1;
                }
                match unprotected_reason(&root.join(path)) {
                    Some(reason) => {
                        skipped.push(Unprotected {
                            path: path.to_path_buf(),
                            reason,
                        });
                        1
                    }
                    None => {
                        staged += 1;
                        if let Some(progress) = &watch.progress {
                            progress(path, staged);
                        }
                        0
                    }
                }
            };
            index.add_all(
                ["*"].iter(),
                git2::IndexAddOption::DEFAULT,
                Some(&mut filter),
            )?;
            // `add_all` adds and updates; it does not notice a file that is
            // gone. This is what turns a deletion into a checkpoint.
            index.update_all(["*"].iter(), None)?;
        }

        if watch.is_cancelled() {
            return Err(JournalError::Cancelled);
        }
        index.write()?;
        let tree_id = index.write_tree()?;
        if !skipped.is_empty() {
            tracing::info!(
                project = %self.project_id,
                count = skipped.len(),
                "some files are not protected by Undo"
            );
        }
        Ok((tree_id, skipped))
    }

    /// The newest checkpoints first.
    pub fn list(&self, limit: usize) -> Result<Vec<Checkpoint>, JournalError> {
        let repo = self.repo.lock().expect("journal lock");
        let Some(head) = head_commit(&repo)? else {
            return Ok(Vec::new());
        };

        let mut walk = repo.revwalk()?;
        walk.push(head.id())?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL)?;

        let mut checkpoints = Vec::new();
        for oid in walk.take(limit) {
            let commit = repo.find_commit(oid?)?;
            checkpoints.push(self.describe(&repo, &commit)?);
        }
        Ok(checkpoints)
    }

    /// Turns a commit into the checkpoint the rest of Eavery talks about.
    ///
    /// The commit is the source of truth, not the SQLite table: the table is a
    /// cache for the UI, and a cache that disagreed with history would be a
    /// wrong answer to "what will I get back".
    fn describe(
        &self,
        repo: &git2::Repository,
        commit: &git2::Commit<'_>,
    ) -> Result<Checkpoint, JournalError> {
        let message = commit.message().unwrap_or_default();
        let (label, kind, turn_id) = parse_message(message);

        let parent = commit.parent(0).ok();
        let parent_tree = parent.as_ref().map(git2::Commit::tree).transpose()?;
        let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit.tree()?), None)?;

        Ok(Checkpoint {
            id: commit.id().to_string(),
            project_id: self.project_id,
            turn_id,
            label,
            kind,
            created_at: timestamp(commit.time()),
            files_changed: diff.deltas().len(),
        })
    }

    /// What changed between two checkpoints.
    pub fn diff(&self, from: &CheckpointId, to: &CheckpointId) -> Result<ChangeSet, JournalError> {
        let repo = self.repo.lock().expect("journal lock");
        let from_tree = self.tree_of(&repo, from)?;
        let to_tree = self.tree_of(&repo, to)?;
        let diff = repo.diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)?;
        change_set(&diff)
    }

    /// What has changed since a checkpoint that is not committed yet: the
    /// user's own edits, or an engine's, before the post-turn checkpoint.
    ///
    /// Untracked files count as added, because to the person looking at the
    /// folder they are exactly that. Excluded files stay excluded.
    pub fn diff_worktree(&self, from: &CheckpointId) -> Result<ChangeSet, JournalError> {
        let repo = self.repo.lock().expect("journal lock");
        let from_tree = self.tree_of(&repo, from)?;
        let mut options = git2::DiffOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_typechange(true);
        let diff = repo.diff_tree_to_workdir(Some(&from_tree), Some(&mut options))?;
        change_set(&diff)
    }

    /// Goes back to a checkpoint, without going back in history.
    ///
    /// The work tree is committed first (D16), so edits Eavery never saw —
    /// the user's own, made between turns — are themselves recoverable, and
    /// the diff that drives the restore is against a tree that matches what is
    /// really on disk. Then the target's files are written and the files that
    /// did not exist in it are removed, one at a time, so a file held open by
    /// Word does not abort the rest. The result is a new checkpoint on top,
    /// and the list of files that could not be written.
    ///
    /// Refusing this while a turn is running is the turn engine's job (C13):
    /// the Journal does not know that turns exist.
    pub fn restore(
        &self,
        target: &CheckpointId,
    ) -> Result<(Checkpoint, Vec<PathBuf>), JournalError> {
        // Not forced: if the work tree already matches HEAD there is nothing
        // to preserve and no point adding a checkpoint that says so.
        self.checkpoint("Before going back", CheckpointKind::Manual, None, false)?;

        let (label, locked) = {
            let repo = self.repo.lock().expect("journal lock");
            let target_commit = repo.find_commit(parse_id(target)?)?;
            let target_tree = target_commit.tree()?;
            let head = head_commit(&repo)?.ok_or_else(|| {
                JournalError::NoSuchCheckpoint("there is nothing to go back from".to_owned())
            })?;
            let diff = repo.diff_tree_to_tree(Some(&head.tree()?), Some(&target_tree), None)?;

            let mut locked = Vec::new();
            for delta in diff.deltas() {
                match delta.status() {
                    git2::Delta::Deleted => {
                        let Some(path) = delta.old_file().path() else {
                            continue;
                        };
                        if let Err(error) = self.remove_file(path) {
                            self.record_or_fail(&mut locked, path, error)?;
                        }
                    }
                    _ => {
                        let Some(path) = delta.new_file().path() else {
                            continue;
                        };
                        let blob = repo.find_blob(delta.new_file().id())?;
                        if let Err(error) = self.write_file(path, blob.content()) {
                            self.record_or_fail(&mut locked, path, error)?;
                        }
                    }
                }
            }
            (
                parse_message(target_commit.message().unwrap_or_default()).0,
                locked,
            )
        };

        if !locked.is_empty() {
            tracing::warn!(
                project = %self.project_id,
                count = locked.len(),
                "some files could not be written back; they are still open somewhere"
            );
        }

        // Forced: a restore is a point in history even when the files it wrote
        // happen to match what was already there.
        let checkpoint = self.checkpoint(
            &format!("Restored: {label}"),
            CheckpointKind::Restore,
            None,
            true,
        )?;
        Ok((checkpoint, locked))
    }

    /// A file that is open somewhere else is reported and stepped over; any
    /// other failure stops the restore, because it is not one the user can do
    /// anything about by closing a window.
    fn record_or_fail(
        &self,
        locked: &mut Vec<PathBuf>,
        path: &Path,
        error: std::io::Error,
    ) -> Result<(), JournalError> {
        if is_lock_error(&error) {
            locked.push(path.to_path_buf());
            return Ok(());
        }
        Err(JournalError::io(self.root.join(path), error))
    }

    fn remove_file(&self, relative: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(self.root.join(relative)) {
            // Already gone is the state that was wanted.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Writes through a temporary file in the same directory and renames over
    /// the original, which is atomic on the same volume: a restore interrupted
    /// by a crash or a full disk leaves the old file, never half of the new
    /// one.
    fn write_file(&self, relative: &Path, contents: &[u8]) -> std::io::Result<()> {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!(
            "{}eavery-tmp",
            path.extension()
                .map(|extension| format!("{}.", extension.to_string_lossy()))
                .unwrap_or_default()
        ));
        std::fs::write(&temporary, contents)?;
        match std::fs::rename(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(error) => {
                // The rename is what fails when the original is held open, and
                // leaving the temporary file behind would be litter in the
                // user's folder.
                let _ = std::fs::remove_file(&temporary);
                Err(error)
            }
        }
    }

    fn tree_of<'repo>(
        &self,
        repo: &'repo git2::Repository,
        id: &CheckpointId,
    ) -> Result<git2::Tree<'repo>, JournalError> {
        Ok(repo.find_commit(parse_id(id)?)?.tree()?)
    }

    /// Everything in the Project that Undo does not cover, and why.
    ///
    /// This is what the "Not protected" panel reads. It exists so that "your
    /// files are protected" is a claim Eavery can always back up: a file that
    /// is too big, or that the user's cloud provider has not downloaded, is
    /// named rather than quietly missing from history.
    pub fn unprotected(&self) -> Result<Vec<Unprotected>, JournalError> {
        let repo = self.repo.lock().expect("journal lock");
        let mut found = Vec::new();
        walk_worktree(&self.root, &mut |absolute, relative| {
            // An excluded file is not unprotected, it is uninteresting: a lock
            // file Word will rewrite anyway, or a folder an engine keeps its
            // own state in.
            if repo.is_path_ignored(relative).unwrap_or(false) {
                return;
            }
            if let Some(reason) = unprotected_reason(absolute) {
                found.push(Unprotected {
                    path: relative.to_path_buf(),
                    reason,
                });
            }
        })?;
        found.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(found)
    }

    /// How many loose objects the Journal has accumulated (C12).
    ///
    /// Packing them is not done here: every safe way to reclaim the space
    /// involves deleting the loose copies once a pack contains them, and doing
    /// that by hand against libgit2 is not something to invent in passing.
    /// The count exists so Settings can show it and so the decision is taken
    /// with a number in front of it.
    pub fn loose_object_count(&self) -> usize {
        let objects = self.git_dir.join("objects");
        let Ok(entries) = std::fs::read_dir(&objects) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| {
                // Loose objects live in 256 two-character folders; `pack` and
                // `info` are the other two entries and are not among them.
                entry.file_name().to_str().is_some_and(|name| {
                    name.len() == 2 && name.chars().all(|c| c.is_ascii_hexdigit())
                })
            })
            .map(|entry| {
                std::fs::read_dir(entry.path())
                    .map(Iterator::count)
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Bytes under the git directory (C12: the Journal grows and the user is
    /// entitled to know by how much).
    pub fn size_on_disk(&self) -> Result<u64, JournalError> {
        Ok(directory_size(&self.git_dir))
    }

    /// Where the history is and how big it has got: what Developer mode shows
    /// on the Home screen and in Settings (`07-ui-vocabulary.md` §3).
    pub fn info(&self) -> Result<JournalInfo, JournalError> {
        Ok(JournalInfo {
            path: self.git_dir.clone(),
            size_bytes: self.size_on_disk()?,
            loose_objects: self.loose_object_count(),
        })
    }
}

/// A Journal as Developer mode describes it: where it is, how much room it
/// takes, and how many loose objects it has collected (C12).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct JournalInfo {
    /// The git directory, under Eavery's data folder — never inside the
    /// Project.
    pub path: PathBuf,
    #[ts(type = "number")]
    pub size_bytes: u64,
    #[ts(type = "number")]
    pub loose_objects: usize,
}

/// What a folder holds, before Eavery commits to protecting it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProjectScan {
    pub files: usize,
    pub bytes: u64,
}

impl ProjectScan {
    /// Over [`MAX_FILES`]: opening this folder is refused, and the user is
    /// asked for a subfolder.
    pub fn too_many_files(&self) -> bool {
        self.files > MAX_FILES
    }

    /// Over [`WARN_TOTAL_BYTES`]: the first checkpoint will take a while and
    /// the Journal will be large. The user is asked, and may go ahead.
    pub fn is_large(&self) -> bool {
        self.bytes > WARN_TOTAL_BYTES
    }
}

/// Counts what is in a folder, for the `open_project` guards
/// (`05-git-journal.md` §4).
///
/// Runs before any Journal exists, so it cannot ask git what is excluded; it
/// skips the directories the exclude list names, which is what the counts are
/// actually sensitive to — a `node_modules` is the difference between four
/// hundred files and forty thousand.
pub fn scan_project(root: &Path) -> Result<ProjectScan, JournalError> {
    let mut scan = ProjectScan::default();
    walk_worktree(root, &mut |absolute, _relative| {
        scan.files += 1;
        scan.bytes += std::fs::metadata(absolute)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
    })?;
    Ok(scan)
}

/// Directories never worth walking into: the user's own git repository, and
/// the state folders engines keep beside the work. These are the entries of
/// the exclude list that are whole directories, and skipping them here is what
/// keeps a scan of a developer's folder from taking a minute.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".claude",
    ".codex",
    ".goose",
    ".gemini",
];

/// Every file under `root`, with its path relative to `root`.
fn walk_worktree(root: &Path, visit: &mut dyn FnMut(&Path, &Path)) -> Result<(), JournalError> {
    fn walk(
        root: &Path,
        dir: &Path,
        visit: &mut dyn FnMut(&Path, &Path),
    ) -> Result<(), JournalError> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            // A folder that cannot be read is not a reason to fail the whole
            // scan; it is one the user will see in "Not protected" soon
            // enough.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
            Err(error) => return Err(JournalError::io(dir, error)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                let name = entry.file_name();
                if SKIPPED_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    continue;
                }
                walk(root, &path, visit)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                visit(&path, relative);
            }
        }
        Ok(())
    }
    walk(root, root, visit)
}

/// Whether a failure to write one file should be reported and stepped over
/// rather than failing the whole restore.
fn is_lock_error(error: &std::io::Error) -> bool {
    matches!(classify(error), IoTrouble::Locked | IoTrouble::Permission)
}

/// The checkpoint id a caller passed, as an object id.
fn parse_id(id: &CheckpointId) -> Result<git2::Oid, JournalError> {
    git2::Oid::from_str(id).map_err(|_| JournalError::NoSuchCheckpoint(id.clone()))
}

/// Turns a diff into the three lists and the patches.
///
/// A patch is produced only for a text file under [`MAX_TEXT_DIFF_BYTES`].
/// `Patch::from_diff` answers `None` for a binary delta, which is also how a
/// binary file is recognised: the same rule git itself uses, rather than a
/// guess from the extension.
fn change_set(diff: &git2::Diff<'_>) -> Result<ChangeSet, JournalError> {
    let mut changes = ChangeSet::default();
    for (index, delta) in diff.deltas().enumerate() {
        let new_path = delta.new_file().path().map(Path::to_path_buf);
        let old_path = delta.old_file().path().map(Path::to_path_buf);
        match delta.status() {
            git2::Delta::Added | git2::Delta::Untracked | git2::Delta::Copied => {
                changes.added.extend(new_path.clone());
            }
            git2::Delta::Deleted => changes.removed.extend(old_path.clone()),
            git2::Delta::Renamed => {
                changes.removed.extend(old_path.clone());
                changes.added.extend(new_path.clone());
            }
            _ => changes
                .changed
                .extend(new_path.clone().or(old_path.clone())),
        }

        let Some(path) = new_path.or(old_path) else {
            continue;
        };
        let too_big = delta.new_file().size().max(delta.old_file().size()) > MAX_TEXT_DIFF_BYTES;
        if too_big {
            continue;
        }
        let Some(mut patch) = git2::Patch::from_diff(diff, index)? else {
            continue;
        };
        // A binary delta still produces a patch — "Binary files a/x and b/x
        // differ" — with no hunks in it. No hunks means there is nothing a
        // person could read, which is also true of a mode-only change.
        if patch.num_hunks() == 0 {
            continue;
        }
        let text = patch.to_buf()?;
        // A patch that is not UTF-8 is not one anybody can read either; the
        // file still appears in the lists above.
        if let Ok(text) = text.as_str() {
            changes.text_diffs.push((path, text.to_owned()));
        }
    }
    Ok(changes)
}

/// Points a Journal at its Project folder, through `core.worktree`.
///
/// The work tree is attached this way, rather than through `init_opts`'
/// `workdir_path`, and no gitlink is written into the Project. Two reasons,
/// either of them sufficient:
///
/// - A Project folder must never contain a `.git` (`05-git-journal.md` §1).
///   The Journal is opened by its git directory and finds the work tree from
///   here; a link in the other direction would mean a user's own `git status`,
///   in their own documents folder, answering for Eavery's history.
/// - A Project can already be a git repository. `init_opts` with a
///   `workdir_path` refuses outright in that case — "cannot overwrite gitlink
///   file" — so anyone who works in git could not have their folders
///   protected at all.
fn attach_worktree(repo: &git2::Repository, root: &Path) -> Result<(), JournalError> {
    let mut config = repo.config()?;
    // git writes paths into config with forward slashes on every platform,
    // including Windows, where this also keeps a UNC path in its git form.
    config.set_str("core.worktree", &root.to_string_lossy().replace('\\', "/"))?;
    config.set_bool("core.bare", false)?;
    // In memory, for the handle in hand: the config above is what the next
    // open reads.
    repo.set_workdir(root, false)?;
    Ok(())
}

fn head_commit(repo: &git2::Repository) -> Result<Option<git2::Commit<'_>>, JournalError> {
    match repo.find_reference(BRANCH_REF) {
        Ok(reference) => Ok(Some(reference.peel_to_commit()?)),
        // An unborn branch is the normal state of a repository that has been
        // created and not yet committed to.
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Whether this file is checkpointed at all, and why not.
///
/// A cloud placeholder is judged by its metadata alone. Reading one would make
/// the provider download the whole file, which is how a checkpoint of a synced
/// folder turns into a multi-gigabyte download nobody asked for.
fn unprotected_reason(path: &Path) -> Option<UnprotectedReason> {
    if is_cloud_placeholder(path) {
        return Some(UnprotectedReason::NotDownloaded);
    }
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.is_file() && metadata.len() > MAX_FILE_BYTES {
        return Some(UnprotectedReason::TooLarge {
            bytes: metadata.len(),
        });
    }
    None
}

#[cfg(windows)]
fn is_cloud_placeholder(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS. OneDrive
    // sets one or both on a file that is not on this machine.
    const OFFLINE: u32 = 0x0000_1000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
    std::fs::metadata(path)
        .map(|metadata| metadata.file_attributes() & (OFFLINE | RECALL_ON_DATA_ACCESS) != 0)
        .unwrap_or(false)
}

/// iCloud evicts `report.pdf` by leaving `.report.pdf.icloud` in its place, so
/// the stub is what is on disk and the real name is absent.
#[cfg(not(windows))]
fn is_cloud_placeholder(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.ends_with(".icloud"))
}

fn message_with_trailers(label: &str, kind: CheckpointKind, turn_id: Option<TurnId>) -> String {
    let mut message = format!("{}\n\n{KIND_TRAILER} {}\n", label.trim(), kind_str(kind));
    if let Some(turn_id) = turn_id {
        message.push_str(&format!("{TURN_TRAILER} {turn_id}\n"));
    }
    message
}

/// The label, the kind and the turn, back out of a commit message. A commit
/// without Eavery's trailers — there should not be one, but a repository is a
/// thing a user can edit — reads as a manual checkpoint.
fn parse_message(message: &str) -> (String, CheckpointKind, Option<TurnId>) {
    let mut kind = CheckpointKind::Manual;
    let mut turn_id = None;
    let mut label_lines: Vec<&str> = Vec::new();

    for line in message.lines() {
        if let Some(value) = line.strip_prefix(KIND_TRAILER) {
            kind = parse_kind(value.trim());
        } else if let Some(value) = line.strip_prefix(TURN_TRAILER) {
            turn_id = value.trim().parse().ok();
        } else {
            label_lines.push(line);
        }
    }
    (label_lines.join("\n").trim().to_owned(), kind, turn_id)
}

fn kind_str(kind: CheckpointKind) -> &'static str {
    match kind {
        CheckpointKind::PreTurn => "pre_turn",
        CheckpointKind::PostTurn => "post_turn",
        CheckpointKind::Manual => "manual",
        CheckpointKind::Restore => "restore",
    }
}

fn parse_kind(value: &str) -> CheckpointKind {
    match value {
        "pre_turn" => CheckpointKind::PreTurn,
        "post_turn" => CheckpointKind::PostTurn,
        "restore" => CheckpointKind::Restore,
        _ => CheckpointKind::Manual,
    }
}

fn timestamp(time: git2::Time) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(time.seconds(), 0).unwrap_or_default()
}

fn directory_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => directory_size(&entry.path()),
            Ok(_) => entry.metadata().map(|metadata| metadata.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips_through_its_trailers() {
        let turn = uuid::Uuid::new_v4();
        let message = message_with_trailers(
            "Before: rename FY25 → FY26",
            CheckpointKind::PreTurn,
            Some(turn),
        );
        let (label, kind, turn_id) = parse_message(&message);

        assert_eq!(label, "Before: rename FY25 → FY26");
        assert_eq!(kind, CheckpointKind::PreTurn);
        assert_eq!(turn_id, Some(turn));
    }

    #[test]
    fn every_kind_survives_the_trip() {
        for kind in [
            CheckpointKind::PreTurn,
            CheckpointKind::PostTurn,
            CheckpointKind::Manual,
            CheckpointKind::Restore,
        ] {
            let message = message_with_trailers("label", kind, None);
            assert_eq!(parse_message(&message).1, kind);
        }
    }

    /// A repository is a thing a user can reach. A commit made by hand must
    /// read as something, not as a parse failure.
    #[test]
    fn a_commit_without_trailers_reads_as_a_manual_checkpoint() {
        let (label, kind, turn_id) = parse_message("did some work by hand\n");
        assert_eq!(label, "did some work by hand");
        assert_eq!(kind, CheckpointKind::Manual);
        assert!(turn_id.is_none());
    }

    #[test]
    fn io_failures_carry_the_action_that_fixes_them() {
        let locked = JournalError::io("a.xlsx", std::io::Error::from_raw_os_error(32));
        assert!(locked.next_action().unwrap().contains("Word or Excel"));

        let denied = JournalError::io(
            "a.txt",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        assert!(denied.next_action().unwrap().contains("folder you own"));

        let full = JournalError::io(
            "a.txt",
            std::io::Error::from(std::io::ErrorKind::StorageFull),
        );
        assert!(full.next_action().unwrap().contains("disk space"));
    }

    #[test]
    fn a_locked_file_is_stepped_over_and_a_missing_one_is_not() {
        // Windows: ERROR_SHARING_VIOLATION, which is what an open workbook
        // looks like. It is not the user doing anything wrong.
        assert!(is_lock_error(&std::io::Error::from_raw_os_error(32)));
        assert!(is_lock_error(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
        assert!(!is_lock_error(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
    }

    #[test]
    fn a_checkpoint_id_that_is_not_one_is_refused_rather_than_guessed() {
        let error = parse_id(&"not-a-commit".to_owned()).unwrap_err();
        assert!(
            matches!(error, JournalError::NoSuchCheckpoint(_)),
            "{error:?}"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn an_icloud_stub_is_a_placeholder_and_its_neighbours_are_not() {
        assert!(is_cloud_placeholder(Path::new("/p/.report.pdf.icloud")));
        assert!(!is_cloud_placeholder(Path::new("/p/report.pdf")));
        assert!(!is_cloud_placeholder(Path::new("/p/notes.icloud")));
    }
}
