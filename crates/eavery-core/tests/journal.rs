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
