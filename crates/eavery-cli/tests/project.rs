//! The Project commands, end to end: open a folder, run a turn in it against
//! the fake engine, look at what changed, and go back (M2-T08).
//!
//! This is the M2 exit test as a test. Everything is real except the model:
//! a real database, a real Journal on real files, a real engine process over
//! real pipes.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

/// A turn that changes one file, adds another, and asks before the edit.
fn edit_script() -> Value {
    json!({
        "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"}, "loadSession": false },
        "session": {},
        "turns": [{ "match": "fy26", "actions": [
            {"thought": "Looking at the report"},
            {"request_permission": {"toolCallId": "t1", "title": "Edit report.txt",
                                    "kind": "edit", "locations": ["{{cwd}}/report.txt"],
                                    "expect": "allow_once"}},
            {"fs_write": {"path": "{{cwd}}/report.txt", "text": "FY26\n"}},
            {"fs_write": {"path": "{{cwd}}/summary.txt", "text": "one line\n"}},
            {"tool_call_update": {"id": "t1", "status": "completed"}},
            {"text": "Changed one number and added a summary."},
            {"stop": "end_turn"}
        ]}]
    })
}

struct Workspace {
    dir: tempfile::TempDir,
    /// The Project folder as the store holds it, and so as every command
    /// prints it back.
    project: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp folder");
        let project = dir.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::write(project.join("report.txt"), "FY25\n").unwrap();

        Self {
            project: eavery_core::paths::canonicalize(&project).expect("resolve the folder"),
            dir,
        }
    }

    /// The canonical form, which is what the CLI stores and prints.
    fn project(&self) -> &Path {
        &self.project
    }

    /// The folder as it was typed, before anything resolved it.
    ///
    /// On Windows these two differ: a temp directory arrives as a short 8.3
    /// path (`C:\Users\RUNNER~1\...`) and resolves to the long one. Passing
    /// this form to the commands is the point — someone typing a path types
    /// whatever their shell gave them, and the Project has to be found anyway.
    fn project_as_typed(&self) -> PathBuf {
        self.dir.path().join("project")
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn script(&self, script: Value) -> PathBuf {
        let path = self.dir.path().join("script.json");
        std::fs::write(&path, script.to_string()).expect("write the script");
        path
    }

    /// Runs the CLI against this workspace's own data directory, so nothing
    /// touches the real one.
    fn cli(&self, args: &[&str]) -> Output {
        let _ = common::fake_agent();
        let mut command = Command::new(env!("CARGO_BIN_EXE_eavery-cli"));
        command
            .arg("--data-dir")
            .arg(self.data())
            .args(args)
            .stdin(std::process::Stdio::null());
        command.output().expect("run eavery-cli")
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.project().join(relative)).expect("read the file")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn succeeds(output: &Output) -> String {
    let printed = stdout(output);
    assert!(
        output.status.success(),
        "the CLI failed ({}):\n{printed}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    printed
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Everything after the first run of the checkpoint id, which is what the
/// tables print and what `--to` takes.
fn short_id(line: &str) -> &str {
    line.split_whitespace().next().expect("an id")
}

#[test]
fn opening_a_folder_protects_it_and_remembers_it() {
    let workspace = Workspace::new();

    let printed =
        succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));
    assert!(printed.contains("folder   1 files"), "{printed}");
    assert!(printed.contains("project  "), "{printed}");
    assert!(printed.contains("Project opened"), "{printed}");

    // The Project folder itself stays clean: the git directory lives in
    // Eavery's data folder, never beside the user's documents.
    assert!(!workspace.project().join(".git").exists());

    let listed = succeeds(&workspace.cli(&["project", "list"]));
    assert!(listed.contains("project"), "{listed}");
    assert!(listed.contains(&path(workspace.project())), "{listed}");

    // Opening again is not a second Project.
    let again =
        succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));
    assert!(again.contains("already open as"), "{again}");
    assert_eq!(
        succeeds(&workspace.cli(&["project", "list"]))
            .lines()
            .count(),
        1,
        "one Project, however many times the folder is opened"
    );
}

/// The M2 exit test: a turn changes the folder, the change is reported, and
/// Undo puts it back exactly as it was.
#[test]
fn a_turn_changes_the_folder_and_undo_puts_it_back() {
    let workspace = Workspace::new();
    let script = workspace.script(edit_script());
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let printed = succeeds(&workspace.cli(&[
        "run",
        "--project",
        &path(&workspace.project_as_typed()),
        "--engine",
        "fake",
        "--script",
        &path(&script),
        "make it FY26",
    ]));

    // The transcript, the permission the policy answered for itself, and the
    // digest.
    for expected in [
        "turn     started",
        "thought  Looking at the report",
        "ask      Edit report.txt (Reversible)",
        "answer   AllowOnce (by Policy)",
        "text     Changed one number and added a summary.",
        "changed  report.txt",
        "added    summary.txt",
        "sent     nothing left this computer",
        "undo     eavery-cli undo --to ",
    ] {
        assert!(
            printed.contains(expected),
            "missing {expected:?}:\n{printed}"
        );
    }
    assert_eq!(workspace.read("report.txt"), "FY26\n");
    assert_eq!(workspace.read("summary.txt"), "one line\n");

    // Both ends of the turn are in the history.
    let history =
        succeeds(&workspace.cli(&["history", "--project", &path(&workspace.project_as_typed())]));
    assert!(history.contains("after"), "{history}");
    assert!(history.contains("After: make it FY26"), "{history}");

    // And Undo puts the folder back, byte for byte.
    let undone =
        succeeds(&workspace.cli(&["undo", "--project", &path(&workspace.project_as_typed())]));
    assert!(undone.contains("back to  "), "{undone}");
    assert_eq!(
        workspace.read("report.txt"),
        "FY25\n",
        "the file is back as it was"
    );
    assert!(
        !workspace.project().join("summary.txt").exists(),
        "and the file the turn added is gone"
    );
}

#[test]
fn diff_says_what_changed_between_two_points() {
    let workspace = Workspace::new();
    let script = workspace.script(edit_script());
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));
    succeeds(&workspace.cli(&[
        "run",
        "--project",
        &path(&workspace.project_as_typed()),
        "--script",
        &path(&script),
        "make it FY26",
    ]));

    let history =
        succeeds(&workspace.cli(&["history", "--project", &path(&workspace.project_as_typed())]));
    let lines: Vec<&str> = history.lines().collect();
    let after = short_id(lines[0]);
    let before = short_id(lines[1]);

    let printed = succeeds(&workspace.cli(&[
        "diff",
        "--project",
        &path(&workspace.project_as_typed()),
        before,
        after,
    ]));
    assert!(printed.contains("changed  report.txt"), "{printed}");
    assert!(printed.contains("added    summary.txt"), "{printed}");
    // A text file gets its patch, which is what makes a change reviewable.
    assert!(printed.contains("-FY25"), "{printed}");
    assert!(printed.contains("+FY26"), "{printed}");

    // With nothing to compare against, the question is "and since then?".
    let since = succeeds(&workspace.cli(&[
        "diff",
        "--project",
        &path(&workspace.project_as_typed()),
        after,
    ]));
    assert!(since.contains("nothing changed"), "{since}");

    std::fs::write(workspace.project().join("mine.txt"), "my own note\n").unwrap();
    let since = succeeds(&workspace.cli(&[
        "diff",
        "--project",
        &path(&workspace.project_as_typed()),
        after,
    ]));
    assert!(
        since.contains("added    mine.txt"),
        "an edit Eavery never saw still shows up:\n{since}"
    );
}

/// Undo with nothing to undo says so, rather than going back to something
/// arbitrary.
#[test]
fn undo_before_any_turn_says_there_is_nothing_to_undo() {
    let workspace = Workspace::new();
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let output = workspace.cli(&["undo", "--project", &path(&workspace.project_as_typed())]);
    assert!(!output.status.success());
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("nothing to undo"), "{complaint}");
}

#[test]
fn a_project_that_is_not_open_is_not_guessed_at() {
    let workspace = Workspace::new();
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let output = workspace.cli(&["history", "--project", "no-such-project"]);
    assert!(!output.status.success());
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("no project matches"), "{complaint}");
    // And it says which ones there are, so the next command can be right.
    assert!(
        complaint.contains(&path(workspace.project())),
        "{complaint}"
    );
}

/// Nobody is at the terminal, so anything the policy will not answer is
/// refused. The engine's script expects that answer and exits non-zero if it
/// gets another, which is what makes this a test of the whole round trip.
#[test]
fn an_unattended_run_refuses_what_it_cannot_ask_about() {
    let workspace = Workspace::new();
    let script = workspace.script(json!({
        "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"}, "loadSession": false },
        "session": {},
        "turns": [{ "match": "backup", "actions": [
            {"request_permission": {"toolCallId": "t1", "title": "Run the backup script",
                                    "kind": "execute", "locations": [],
                                    "expect": "reject_once"}},
            {"text": "I did not run it."},
            {"stop": "end_turn"}
        ]}]
    }));
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let printed = succeeds(&workspace.cli(&[
        "run",
        "--project",
        &path(&workspace.project_as_typed()),
        "--script",
        &path(&script),
        "run the backup",
    ]));
    assert!(printed.contains("no terminal to ask on"), "{printed}");
    assert!(
        printed.contains("answer   RejectOnce (by User)"),
        "{printed}"
    );
    assert!(
        printed.contains("refused  Run the backup script"),
        "{printed}"
    );
}

/// The plan gate, over real ACP (`06-plan-gate-permissions.md` §7, tests 1
/// and 2): the planning turn is refused an edit, a write through the client
/// and an attempt to leave plan mode — the script exits non-zero if any of
/// them got through — and nothing changes until the plan is approved.
#[test]
fn a_plan_is_shown_and_nothing_runs_until_it_is_approved() {
    let workspace = Workspace::new();
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../eavery-core/tests/scripts/plan.json");
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let printed = succeeds(&workspace.cli(&[
        "run",
        "--project",
        &path(&workspace.project_as_typed()),
        "--script",
        &path(&script),
        "--plan",
        "--approve",
        "yes",
        "--edits",
        "keep the heading as it is",
        "rename FY25 to FY26",
    ]));

    assert!(printed.contains("turn     started (Planning)"), "{printed}");
    assert!(printed.contains("plan     Update the report"), "{printed}");
    assert!(printed.contains("1. Open report.txt"), "{printed}");
    assert!(
        printed.contains("sends    nothing leaves this computer"),
        "{printed}"
    );
    assert!(
        printed.contains("vendor   your documents are sent to local"),
        "{printed}"
    );
    assert!(
        printed.contains("answer   RejectOnce (by PlanGate)"),
        "{printed}"
    );
    assert!(printed.contains("phase    AwaitingApproval"), "{printed}");
    assert!(
        printed.contains("approve  yes, with changes: keep the heading as it is"),
        "{printed}"
    );
    assert!(printed.contains("phase    Executing"), "{printed}");
    assert!(
        printed.contains("answer   AllowOnce (by Policy)"),
        "{printed}"
    );
    assert!(printed.contains("changed  report.txt"), "{printed}");
    assert!(printed.contains("done     end_turn"), "{printed}");
    assert_eq!(workspace.read("report.txt"), "FY26\n");
}

/// Nobody at the terminal, no `--approve`: the plan is shown and not carried
/// out. A plan nobody read is never executed.
#[test]
fn a_plan_nobody_approves_runs_nothing() {
    let workspace = Workspace::new();
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../eavery-core/tests/scripts/plan.json");
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let output = workspace.cli(&[
        "run",
        "--project",
        &path(&workspace.project_as_typed()),
        "--script",
        &path(&script),
        "--plan",
        "rename FY25 to FY26",
    ]);
    let printed = stdout(&output);
    assert_eq!(
        output.status.code(),
        Some(130),
        "a turn that did not run ends as cancelled:\n{printed}"
    );
    assert!(printed.contains("plan     Update the report"), "{printed}");
    assert!(printed.contains("no terminal to ask on"), "{printed}");
    assert!(printed.contains("approve  no"), "{printed}");
    assert!(printed.contains("done     plan_rejected"), "{printed}");
    assert!(printed.contains("files    nothing changed"), "{printed}");
    assert!(!printed.contains("phase    Executing"), "{printed}");
    assert_eq!(workspace.read("report.txt"), "FY25\n");
}

/// §7, test 3: in the execute phase an edit inside the Project goes through
/// on its own, an edit outside it is asked about, and so is anything that
/// leaves the computer. With nobody to ask, both are refused.
#[test]
fn the_execute_phase_asks_about_what_undo_cannot_reach() {
    let workspace = Workspace::new();
    let outside = workspace.dir.path().join("elsewhere.txt");
    let script = workspace.script(json!({
        "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"}, "loadSession": false },
        "session": {},
        "turns": [
            { "match": "write a plan", "actions": [
                {"text": "```eavery-plan\n{\"summary\":\"Tidy up\",\"steps\":[\"Edit the report\"],\"outbound\":[\"Send the report to example.com\"]}\n```"},
                {"stop": "end_turn"}
            ]},
            { "match": "approved this plan", "actions": [
                {"request_permission": {"toolCallId": "t1", "title": "Edit report.txt",
                                        "kind": "edit", "locations": ["{{cwd}}/report.txt"],
                                        "expect": "allow_once"}},
                {"request_permission": {"toolCallId": "t2", "title": "Edit elsewhere.txt",
                                        "kind": "edit", "locations": [path(&outside)],
                                        "expect": "reject_once"}},
                {"request_permission": {"toolCallId": "t3", "title": "Send the report to example.com",
                                        "kind": "fetch", "locations": [],
                                        "expect": "reject_once"}},
                {"text": "Done what I could."},
                {"stop": "end_turn"}
            ]}
        ]
    }));
    succeeds(&workspace.cli(&["project", "open", &path(&workspace.project_as_typed())]));

    let printed = succeeds(&workspace.cli(&[
        "run",
        "--project",
        &path(&workspace.project_as_typed()),
        "--script",
        &path(&script),
        "--plan",
        "--approve",
        "yes",
        "tidy up",
    ]));

    assert!(
        printed.contains("sends    Send the report to example.com"),
        "{printed}"
    );
    assert!(
        printed.contains("ask      Edit report.txt (Reversible)"),
        "{printed}"
    );
    assert!(
        printed.contains("ask      Edit elsewhere.txt (Destructive)"),
        "{printed}"
    );
    assert!(
        printed.contains("ask      Send the report to example.com (Outbound)"),
        "{printed}"
    );
    assert!(
        printed.contains("refused  Edit elsewhere.txt; Send the report to example.com"),
        "{printed}"
    );
    assert!(
        printed.contains("sent     nothing left this computer"),
        "{printed}"
    );
}
