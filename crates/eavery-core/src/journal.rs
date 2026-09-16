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

use crate::model::{Checkpoint, CheckpointKind, ProjectId, TurnId};

/// Files above this are not checkpointed: hashing them on every turn would
/// cost more than the protection is worth. They are listed as unprotected
/// rather than silently dropped.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// Above this, `open_project` asks the user to pick a subfolder first.
pub const WARN_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Above this, `open_project` refuses.
pub const MAX_FILES: usize = 200_000;

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

    /// Bytes under the git directory (C12: the Journal grows and the user is
    /// entitled to know by how much).
    pub fn size_on_disk(&self) -> Result<u64, JournalError> {
        Ok(directory_size(&self.git_dir))
    }
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

    #[cfg(not(windows))]
    #[test]
    fn an_icloud_stub_is_a_placeholder_and_its_neighbours_are_not() {
        assert!(is_cloud_placeholder(Path::new("/p/.report.pdf.icloud")));
        assert!(!is_cloud_placeholder(Path::new("/p/report.pdf")));
        assert!(!is_cloud_placeholder(Path::new("/p/notes.icloud")));
    }
}
