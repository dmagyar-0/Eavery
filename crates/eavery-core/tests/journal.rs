//! The Journal's tests (`docs/plan/05-git-journal.md` §7), on real files in
//! temporary directories. Nothing about git is mocked: the thing being tested
//! is what ends up on disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use eavery_core::journal::{Journal, JournalError, MAX_FILE_BYTES, Watch};
use eavery_core::model::{CheckpointKind, ProjectId};

struct Fixture {
    project: tempfile::TempDir,
    data: tempfile::TempDir,
    id: ProjectId,
}

impl Fixture {
    fn new() -> Self {
        Self {
            project: tempfile::tempdir().expect("a project folder"),
            data: tempfile::tempdir().expect("a data folder"),
            id: uuid::Uuid::new_v4(),
        }
    }

    fn root(&self) -> &Path {
        self.project.path()
    }

    fn open(&self) -> Journal {
        Journal::open_or_create(self.id, self.root(), self.data.path(), &Watch::default())
            .expect("open the journal")
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("make the folder");
        }
        std::fs::write(&path, contents).expect("write the file");
        path
    }
}

/// Whether this user is subject to file permissions at all. root is not, and a
/// test that stands in for a Windows lock by making a directory read-only
/// would quietly pass by doing nothing.
#[cfg(unix)]
fn permissions_are_enforced() -> bool {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("a folder");
    let closed = dir.path().join("closed");
    std::fs::create_dir(&closed).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o555)).unwrap();
    let denied = std::fs::write(closed.join("probe"), "x").is_err();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
    denied
}

/// Test 1: the Project folder is not a git repository, and opening twice is
/// opening the same Journal.
#[test]
fn opening_twice_reuses_one_journal_and_leaves_no_dotgit_in_the_project() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one");

    let first = fixture.open();
    let opened = first.list(10).unwrap();
    assert_eq!(opened.len(), 1, "opening a Project checkpoints it");
    assert_eq!(opened[0].label, "Project opened");

    let second = fixture.open();
    assert_eq!(
        second.list(10).unwrap().len(),
        1,
        "opening an existing Journal must not checkpoint again"
    );
    assert_eq!(second.list(10).unwrap()[0].id, opened[0].id);

    assert!(
        !fixture.root().join(".git").exists(),
        "a user's own folder must never gain a .git"
    );
    assert!(
        fixture
            .data
            .path()
            .join("journals")
            .join(fixture.id.to_string())
            .join("HEAD")
            .exists()
    );
}

/// A folder of documents is the common case, but a Project can be a git
/// repository someone already works in. Eavery's history sits somewhere else
/// entirely, and their `.git` is not Eavery's to touch.
#[test]
fn a_project_that_is_already_a_git_repository_keeps_its_own_git() {
    let fixture = Fixture::new();
    let theirs = git2::Repository::init(fixture.root()).expect("their repository");
    let their_head = std::fs::read_to_string(theirs.path().join("HEAD")).unwrap();
    fixture.write("a.txt", "one");

    let journal = fixture.open();
    assert_eq!(journal.list(10).unwrap().len(), 1);

    assert!(
        fixture.root().join(".git").is_dir(),
        "their repository is gone"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root().join(".git/HEAD")).unwrap(),
        their_head,
        "their HEAD was rewritten"
    );

    // And Eavery's own history does not include their repository's internals.
    fixture.write("b.txt", "two");
    let after = journal
        .checkpoint("After", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(after.files_changed, 1);
}

/// Test 2: nothing changed means nothing to go back to, unless the caller
/// insists — which the pre-turn checkpoint does, so every turn has an anchor.
#[test]
fn an_unchanged_checkpoint_reuses_head_unless_it_is_forced() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one");
    let journal = fixture.open();
    let opened = journal.list(1).unwrap()[0].clone();

    let unchanged = journal
        .checkpoint("Nothing happened", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(
        unchanged.id, opened.id,
        "an empty commit is not a checkpoint"
    );
    assert_eq!(journal.list(10).unwrap().len(), 1);

    let forced = journal
        .checkpoint("Before: do something", CheckpointKind::PreTurn, None, true)
        .unwrap();
    assert_ne!(forced.id, opened.id);
    assert_eq!(forced.kind, CheckpointKind::PreTurn);
    assert_eq!(forced.label, "Before: do something");
    assert_eq!(journal.list(10).unwrap().len(), 2);
}

#[test]
fn a_checkpoint_carries_its_turn_and_counts_what_changed() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let turn = uuid::Uuid::new_v4();

    fixture.write("a.txt", "one");
    fixture.write("b.txt", "two");
    let checkpoint = journal
        .checkpoint(
            "After: wrote two files",
            CheckpointKind::PostTurn,
            Some(turn),
            false,
        )
        .unwrap();

    assert_eq!(checkpoint.turn_id, Some(turn));
    assert_eq!(checkpoint.kind, CheckpointKind::PostTurn);
    assert_eq!(checkpoint.files_changed, 2);
    assert_eq!(checkpoint.project_id, fixture.id);
}

/// Test 5: a file too big to hash on every turn is not protected, and says so
/// rather than being dropped silently.
#[test]
fn a_file_over_the_size_guard_is_not_tracked() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("small.txt", "small");
    let big = fixture.root().join("big.bin");
    let file = std::fs::File::create(&big).unwrap();
    file.set_len(MAX_FILE_BYTES + 1).unwrap();
    drop(file);

    let checkpoint = journal
        .checkpoint("After", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(
        checkpoint.files_changed, 1,
        "only the small file should be in the tree"
    );
}

/// Test 6: Office lock files and desktop clutter are never history.
#[test]
fn office_lock_files_and_os_clutter_are_never_tracked() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("report.docx", "real");
    fixture.write("~$report.docx", "word's lock file");
    fixture.write(".DS_Store", "finder");
    fixture.write("Thumbs.db", "explorer");
    fixture.write("half.crdownload", "a download in progress");
    fixture.write("report.docx.eavery-tmp", "the connector's temp file");

    let checkpoint = journal
        .checkpoint("After", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(
        checkpoint.files_changed, 1,
        "only report.docx belongs in the tree"
    );
}

/// Test 12: checkpointing an engine's state folder is harmless; restoring it
/// would rewind the engine's own memory, so it is never tracked.
#[test]
fn engine_state_folders_are_never_tracked() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("notes.txt", "mine");
    fixture.write(".claude/settings.json", "{}");
    fixture.write(".codex/config.toml", "");
    fixture.write(".goose/state", "");
    fixture.write(".gemini/settings.json", "{}");
    fixture.write("node_modules/left-pad/index.js", "");

    let checkpoint = journal
        .checkpoint("After", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(checkpoint.files_changed, 1);
}

/// Test 9: the Journal is about the files, not the path they sit at.
#[test]
fn moving_the_project_folder_moves_the_journal_with_it() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one");
    let journal = fixture.open();
    let opened = journal.list(1).unwrap()[0].clone();
    drop(journal);

    let moved = fixture.data.path().join("moved-project");
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::rename(fixture.root().join("a.txt"), moved.join("a.txt")).unwrap();

    let journal =
        Journal::open_or_create(fixture.id, &moved, fixture.data.path(), &Watch::default())
            .expect("reopen at the new path");
    assert_eq!(
        journal.list(10).unwrap().len(),
        1,
        "the history has to survive the move"
    );
    assert_eq!(journal.list(1).unwrap()[0].id, opened.id);

    std::fs::write(moved.join("a.txt"), "two").unwrap();
    let after = journal
        .checkpoint("After", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(
        after.files_changed, 1,
        "a checkpoint at the new path has to see the new path's files"
    );
}

/// The first checkpoint of a real folder is the slow one. It has to be
/// watchable, and it has to stop when asked.
#[test]
fn the_first_checkpoint_reports_progress() {
    let fixture = Fixture::new();
    for index in 0..5 {
        fixture.write(&format!("file-{index}.txt"), "x");
    }

    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let watch = Watch::default().on_progress(move |_path, staged| {
        counter.store(staged, Ordering::SeqCst);
    });

    let journal =
        Journal::open_or_create(fixture.id, fixture.root(), fixture.data.path(), &watch).unwrap();
    assert_eq!(seen.load(Ordering::SeqCst), 5);
    assert_eq!(journal.list(1).unwrap()[0].files_changed, 5);
}

#[test]
fn a_cancelled_first_checkpoint_stops_and_says_so() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one");

    let cancel = Arc::new(AtomicBool::new(true));
    let watch = Watch::default().with_cancel(cancel);

    let error = Journal::open_or_create(fixture.id, fixture.root(), fixture.data.path(), &watch)
        .unwrap_err();
    assert!(matches!(error, JournalError::Cancelled), "{error:?}");
}

#[test]
fn the_journal_reports_its_own_size() {
    let fixture = Fixture::new();
    fixture.write("a.txt", "one");
    let journal = fixture.open();
    assert!(
        journal.size_on_disk().unwrap() > 0,
        "a journal with a commit in it is not zero bytes"
    );
}

/// Test 3: going back to a checkpoint brings the old content back, and the
/// history grows rather than shrinks.
#[test]
fn restoring_an_earlier_checkpoint_brings_the_old_content_back() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("report.txt", "FY25");
    let first = journal
        .checkpoint(
            "After: wrote the report",
            CheckpointKind::PostTurn,
            None,
            false,
        )
        .unwrap();
    fixture.write("report.txt", "FY26");
    journal
        .checkpoint(
            "After: renamed the year",
            CheckpointKind::PostTurn,
            None,
            false,
        )
        .unwrap();

    let (restored, locked) = journal.restore(&first.id).unwrap();

    assert!(locked.is_empty());
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("report.txt")).unwrap(),
        "FY25"
    );
    assert_eq!(restored.kind, CheckpointKind::Restore);
    assert_eq!(restored.label, "Restored: After: wrote the report");

    let history = journal.list(10).unwrap();
    assert_eq!(
        history.len(),
        4,
        "opened, two turns, and the restore: {:?}",
        history.iter().map(|point| &point.label).collect::<Vec<_>>()
    );
    assert_eq!(history[0].id, restored.id, "history moves forward");
}

/// Test 4: a deletion is as recoverable as an edit.
#[test]
fn a_deleted_file_comes_back() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("keep.txt", "here");
    let before = journal
        .checkpoint("After: wrote it", CheckpointKind::PostTurn, None, false)
        .unwrap();

    std::fs::remove_file(fixture.root().join("keep.txt")).unwrap();
    let after = journal
        .checkpoint("After: deleted it", CheckpointKind::PostTurn, None, false)
        .unwrap();
    assert_eq!(after.files_changed, 1);

    journal.restore(&before.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("keep.txt")).unwrap(),
        "here"
    );
}

/// Test 7: the files this product exists for are Word and Excel documents. A
/// checkpoint that changed one byte of a `.xlsx` would be worse than none.
#[test]
fn a_binary_file_round_trips_byte_for_byte() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    // A zip header, a NUL run, and every byte value: enough that any text
    // handling anywhere in the path would corrupt it.
    let mut bytes = b"PK\x03\x04\x14\x00\x00\x00\x08\x00".to_vec();
    bytes.extend((0u8..=255).cycle().take(4096));
    bytes.extend([0u8; 512]);
    fixture.write("budget.xlsx", &bytes);

    let saved = journal
        .checkpoint(
            "After: wrote the workbook",
            CheckpointKind::PostTurn,
            None,
            false,
        )
        .unwrap();
    assert_eq!(saved.files_changed, 1);

    fixture.write("budget.xlsx", b"ruined");
    journal
        .checkpoint("After: ruined it", CheckpointKind::PostTurn, None, false)
        .unwrap();
    journal.restore(&saved.id).unwrap();

    assert_eq!(
        std::fs::read(fixture.root().join("budget.xlsx")).unwrap(),
        bytes,
        "a workbook has to come back exactly as it went in"
    );
}

/// Test 10 (D16): the reason a restore checkpoints first. An edit Eavery never
/// saw is not lost by going back, and can itself be gone back to.
#[test]
fn a_hand_edit_survives_going_back_and_can_be_returned_to() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("a.txt", "one");
    let first = journal
        .checkpoint("After: wrote one", CheckpointKind::PostTurn, None, false)
        .unwrap();
    fixture.write("a.txt", "two");
    journal
        .checkpoint("After: wrote two", CheckpointKind::PostTurn, None, false)
        .unwrap();

    // The user edits the file themselves. Eavery is not watching.
    fixture.write("a.txt", "three, by hand");

    journal.restore(&first.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("a.txt")).unwrap(),
        "one"
    );

    let history = journal.list(10).unwrap();
    let kept = history
        .iter()
        .find(|point| point.label == "Before going back")
        .expect("the hand edit has to have been checkpointed first");

    journal.restore(&kept.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("a.txt")).unwrap(),
        "three, by hand",
        "the hand edit has to be somewhere the user can get back to"
    );
}

/// Test 11: a file made by hand after the last checkpoint is removed by going
/// back — but only after it has been captured, so it is recoverable.
#[test]
fn a_file_created_by_hand_is_captured_before_a_restore_removes_it() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("a.txt", "one");
    let before = journal
        .checkpoint("After: wrote a.txt", CheckpointKind::PostTurn, None, false)
        .unwrap();

    fixture.write("by-hand.txt", "the user's own file");
    journal.restore(&before.id).unwrap();

    assert!(
        !fixture.root().join("by-hand.txt").exists(),
        "going back means going back"
    );

    let kept = journal
        .list(10)
        .unwrap()
        .into_iter()
        .find(|point| point.label == "Before going back")
        .expect("the new file has to have been checkpointed first");
    journal.restore(&kept.id).unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("by-hand.txt")).unwrap(),
        "the user's own file"
    );
}

/// Test 8: a file something else is holding is reported, and the rest of the
/// restore still happens. On Unix the stand-in for a Windows lock is a
/// directory Eavery may not write to.
#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_written_is_reported_and_the_others_are_restored() {
    use std::os::unix::fs::PermissionsExt;

    if !permissions_are_enforced() {
        // root ignores directory permissions, so there is no way to hold a
        // file shut here. CI runs as an ordinary user and does check this.
        eprintln!("skipped: running as a user that permissions do not apply to");
        return;
    }

    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("open/held.txt", "original");
    fixture.write("free.txt", "original");
    let before = journal
        .checkpoint("After: wrote both", CheckpointKind::PostTurn, None, false)
        .unwrap();

    fixture.write("open/held.txt", "changed");
    fixture.write("free.txt", "changed");
    journal
        .checkpoint("After: changed both", CheckpointKind::PostTurn, None, false)
        .unwrap();

    let locked_dir = fixture.root().join("open");
    let original = std::fs::metadata(&locked_dir).unwrap().permissions();
    std::fs::set_permissions(&locked_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let (_, locked) = journal.restore(&before.id).unwrap();

    std::fs::set_permissions(&locked_dir, original).unwrap();

    assert_eq!(locked, vec![PathBuf::from("open/held.txt")]);
    assert_eq!(
        std::fs::read_to_string(fixture.root().join("free.txt")).unwrap(),
        "original",
        "one file being held must not stop the others coming back"
    );
    assert_eq!(
        std::fs::read_to_string(locked_dir.join("held.txt")).unwrap(),
        "changed",
        "the held file is untouched, and said so"
    );
}

#[test]
fn a_restore_to_a_checkpoint_that_does_not_exist_is_refused() {
    let fixture = Fixture::new();
    let journal = fixture.open();
    let error = journal.restore(&"not-a-checkpoint".to_owned()).unwrap_err();
    assert!(
        matches!(error, JournalError::NoSuchCheckpoint(_)),
        "{error:?}"
    );
}

#[test]
fn a_diff_between_checkpoints_lists_what_changed_and_shows_the_text() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("keep.txt", "FY25\n");
    fixture.write("gone.txt", "temporary\n");
    let before = journal
        .checkpoint("After: first", CheckpointKind::PostTurn, None, false)
        .unwrap();

    fixture.write("keep.txt", "FY26\n");
    fixture.write("new.txt", "added\n");
    std::fs::remove_file(fixture.root().join("gone.txt")).unwrap();
    let after = journal
        .checkpoint("After: second", CheckpointKind::PostTurn, None, false)
        .unwrap();

    let changes = journal.diff(&before.id, &after.id).unwrap();
    assert_eq!(changes.added, vec![PathBuf::from("new.txt")]);
    assert_eq!(changes.changed, vec![PathBuf::from("keep.txt")]);
    assert_eq!(changes.removed, vec![PathBuf::from("gone.txt")]);

    let patch = changes
        .text_diffs
        .iter()
        .find(|(path, _)| path == Path::new("keep.txt"))
        .map(|(_, text)| text.clone())
        .expect("a text file has a text diff");
    assert!(patch.contains("-FY25"), "{patch}");
    assert!(patch.contains("+FY26"), "{patch}");
}

/// A binary file appears in the lists and nowhere else: a patch of a `.xlsx`
/// is noise nobody can read.
#[test]
fn a_binary_file_has_no_text_diff() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("budget.xlsx", b"PK\x03\x04\x00\x01\x02\x00\x00\x00binary");
    let before = journal
        .checkpoint("After: first", CheckpointKind::PostTurn, None, false)
        .unwrap();
    fixture.write("budget.xlsx", b"PK\x03\x04\x00\x01\x02\x00\x00\x00changed");
    let after = journal
        .checkpoint("After: second", CheckpointKind::PostTurn, None, false)
        .unwrap();

    let changes = journal.diff(&before.id, &after.id).unwrap();
    assert_eq!(changes.changed, vec![PathBuf::from("budget.xlsx")]);
    assert!(changes.text_diffs.is_empty(), "{:?}", changes.text_diffs);
}

/// What an engine has done so far, before the post-turn checkpoint exists.
#[test]
fn a_worktree_diff_sees_uncommitted_work() {
    let fixture = Fixture::new();
    let journal = fixture.open();

    fixture.write("a.txt", "one\n");
    let point = journal
        .checkpoint("After: wrote a.txt", CheckpointKind::PostTurn, None, false)
        .unwrap();

    fixture.write("a.txt", "two\n");
    fixture.write("b.txt", "new\n");
    fixture.write("~$a.docx", "a lock file nobody wants to hear about");

    let changes = journal.diff_worktree(&point.id).unwrap();
    assert_eq!(changes.changed, vec![PathBuf::from("a.txt")]);
    assert_eq!(
        changes.added,
        vec![PathBuf::from("b.txt")],
        "an excluded file is not a change anyone asked about"
    );
    assert!(journal.diff_worktree(&point.id).unwrap().removed.is_empty());
}
