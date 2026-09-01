# 05 — The Journal: Checkpoints and Undo on libgit2

The Journal is the licence to operate on someone's real files. It must be
correct before it is fast, and it must never be skippable.

## 1. Design

- One git repository per Project. Its **git directory** is
  `<data_dir>/journals/<project-id>/`. Its **work tree** is the Project root.
  The Project folder never contains `.git`.
- Checkpoints are commits on a single branch `eavery`. There is no remote.
- Restore is forward-only: a restore creates a new commit whose tree equals the
  target checkpoint's tree, then checks that tree out into the work tree.
- Author for every commit: `Eavery <eavery@localhost>`. Message is the label.
- The index is rebuilt from the work tree on every checkpoint (`add_all` with
  the exclude rules). Eavery never relies on a stale index.

## 2. Public API (`eavery-core::journal`)

```rust
pub struct Journal { repo: git2::Repository, root: PathBuf, project_id: ProjectId }

impl Journal {
    /// Open or create. Creates <data_dir>/journals/<id>/, sets core.worktree = root,
    /// writes info/exclude, creates an initial empty checkpoint labelled "Project opened".
    pub fn open_or_create(project_id: ProjectId, root: &Path, data_dir: &Path) -> Result<Self, JournalError>;

    /// Stage everything (respecting excludes and the size guard), commit if the tree
    /// differs from HEAD or `force` is true. Returns the checkpoint (existing HEAD if nothing changed and !force).
    pub fn checkpoint(&self, label: &str, kind: CheckpointKind, force: bool) -> Result<Checkpoint, JournalError>;

    pub fn list(&self, limit: usize) -> Result<Vec<Checkpoint>, JournalError>;

    /// Files added/changed/removed between two checkpoints, plus unified text diffs for text files under 256 KB.
    pub fn diff(&self, from: &CheckpointId, to: &CheckpointId) -> Result<ChangeSet, JournalError>;

    /// Files that differ between a checkpoint and the current work tree (uncommitted state).
    pub fn diff_worktree(&self, from: &CheckpointId) -> Result<ChangeSet, JournalError>;

    /// Forward-only restore. Writes files from `target` into the work tree, deletes files that
    /// did not exist in `target` (only files the Journal tracks), then commits with kind Restore.
    /// Returns the new checkpoint and the list of files that could not be written (locked).
    pub fn restore(&self, target: &CheckpointId) -> Result<(Checkpoint, Vec<PathBuf>), JournalError>;

    /// Files skipped by the size guard or excludes, for the "Not protected" panel.
    pub fn unprotected(&self) -> Result<Vec<Unprotected>, JournalError>;
}
```

`ChangeSet { added: Vec<PathBuf>, changed: Vec<PathBuf>, removed: Vec<PathBuf>, text_diffs: Vec<(PathBuf, String)> }`.

## 3. Implementation notes with git2

Creating the detached repository:

```rust
let mut opts = git2::RepositoryInitOptions::new();
opts.workdir_path(root);         // work tree = project folder
opts.no_dotgit_dir(true);        // git dir is the path we pass, not path/.git
opts.initial_head("eavery");
let repo = git2::Repository::init_opts(&git_dir, &opts)?;
```

Opening it later:

```rust
let repo = git2::Repository::open(&git_dir)?;
// Verify repo.workdir() == root; if the project moved, update core.worktree via repo.config().
```

Excludes: write `<git_dir>/info/exclude` on every open (idempotent):

```
~$*
.~lock.*#
.DS_Store
Thumbs.db
desktop.ini
*.tmp
*.crdownload
*.partial
node_modules/
.git/
.eavery-tmp/
```

Checkpoint:

```rust
let mut index = repo.index()?;
index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, Some(&mut |path, _| {
    if size_of(root.join(path)) > MAX_FILE_BYTES { skipped.push(path.to_owned()); 1 } else { 0 } // 1 = skip
}))?;
index.update_all(["*"].iter(), None)?;     // pick up deletions
index.write()?;
let tree_id = index.write_tree()?;
let head = repo.head().ok().and_then(|h| h.target());
if !force && head.and_then(|h| repo.find_commit(h).ok()).map(|c| c.tree_id()) == Some(tree_id) {
    return Ok(existing_head_checkpoint);
}
let sig = git2::Signature::now("Eavery", "eavery@localhost")?;
let parents: Vec<git2::Commit> = head.map(|h| repo.find_commit(h)).transpose()?.into_iter().collect();
let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
let oid = repo.commit(Some("refs/heads/eavery"), &sig, &sig, label, &repo.find_tree(tree_id)?, &parent_refs)?;
```

Store `kind` in the commit message trailer: `Eavery-Kind: pre_turn` and
`Eavery-Turn: <turn-id>`. `list` parses trailers back. Do not depend on the
SQLite `checkpoints` table for correctness; it is a cache for the UI.

Restore (forward-only), file by file so locked files do not abort the rest:

```rust
let target_tree = repo.find_commit(oid_of(target))?.tree()?;
let head_tree = repo.head()?.peel_to_tree()?;
let diff = repo.diff_tree_to_tree(Some(&head_tree), Some(&target_tree), None)?;
let mut locked = vec![];
for delta in diff.deltas() {
    let path = root.join(delta.new_file().path().or(delta.old_file().path()).unwrap());
    match delta.status() {
        git2::Delta::Deleted => { if let Err(e) = std::fs::remove_file(&path) { if is_lock_error(&e) { locked.push(path) } else { return Err(e.into()) } } }
        _ => {
            let blob = repo.find_blob(delta.new_file().id())?;
            // write to a temp file in the same directory, then rename over the original (atomic on the same volume)
            if let Err(e) = write_atomic(&path, blob.content()) { if is_lock_error(&e) { locked.push(path) } else { return Err(e.into()) } }
        }
    }
}
let cp = self.checkpoint(&format!("Restored: {}", target_label), CheckpointKind::Restore, true)?;
Ok((cp, locked))
```

`is_lock_error`: on Windows, `raw_os_error() == Some(32)` (sharing violation)
or `Some(33)`; on Unix, `PermissionDenied`. Everything else is a real error.

Diff for text files: use `repo.diff_tree_to_tree` with `git2::DiffOptions`
and `diff.print(git2::DiffFormat::Patch, ...)` only for deltas whose blob is
not binary (`blob.is_binary()`) and under 256 KB. Binary files appear only in
the added/changed/removed lists.

## 4. Size and count guards

| Guard | Value | Behaviour |
|---|---|---|
| `MAX_FILE_BYTES` | 50 MB | file skipped, listed in "Not protected" |
| `WARN_TOTAL_BYTES` | 2 GB | at `open_project`, UI asks user to pick a subfolder; can proceed |
| `MAX_FILES` | 200,000 | at `open_project`, refuse; suggest a subfolder |
| Checkpoint time | > 10 s | log a warning with the slowest paths |

## 5. Ordering guarantees in a turn

1. `checkpoint("Before: <request>", PreTurn, force = true)` — forced so the
   turn always has an anchor even if nothing changed since the last one.
2. Engine runs.
3. `checkpoint("After: <request>", PostTurn, force = false)`; if nothing
   changed, `post_checkpoint = pre_checkpoint` and the digest says so.
4. Digest = `diff(pre, post)` + outbound log from the audit table.

If step 1 fails, the turn does not start. Error `next_action` texts:
- lock error → "Close any files from this Project that are open in Word or Excel, then try again."
- permission error → "Eavery cannot write to this folder. Choose a folder you own."
- disk full → "There is not enough disk space to protect this Project."

## 6. Undo semantics in the UI

- "Undo" on a finished turn = `restore(turn.pre_checkpoint)`.
- "Redo" = `restore(turn.post_checkpoint)` (available after an undo).
- The checkpoint list shows every checkpoint including restores, newest
  first, with labels. Selecting one shows `diff(selected, HEAD)` as "what
  would change if you go back here", then a "Go back to this point" button.
- The user's own edits between turns are captured by the next pre-turn
  checkpoint, and are therefore also undoable. Say this in the UI copy.

## 7. Tests (must exist before M2 is done)

Use `tempfile::tempdir()` for both the project root and the data dir.

1. `open_or_create` twice on the same folder returns the same journal and
   creates no `.git` inside the project.
2. Checkpoint with no changes and `force=false` returns HEAD; with `force=true`
   creates a new commit with the same tree.
3. Create a file, checkpoint, modify it, checkpoint, restore first → content
   equals original; `list` has four entries (opened, cp1, cp2, restore).
4. Delete a tracked file, checkpoint, restore earlier → file is back.
5. A file above `MAX_FILE_BYTES` is not in the tree and appears in `unprotected()`.
6. `~$doc.docx` and `.DS_Store` are never tracked.
7. Binary file (`.xlsx` bytes) round-trips byte-for-byte through checkpoint and restore.
8. Simulated lock (Unix: make a file read-only in a read-only dir) → restore
   returns it in the locked list and restores the others.
9. Moving the project folder and reopening with the new path updates the work tree.
10. Property test (optional, `proptest`): random sequences of write/delete/
    checkpoint/restore never lose a checkpointed tree.
