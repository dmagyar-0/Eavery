//! The M0 exit test, as a test: the CLI round-trips a prompt through the fake
//! agent, including one permission request, and prints what happened.
//!
//! It runs the real binary against a real child engine over real pipes. The
//! only thing faked is the model.

mod common;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};

/// The script for the exit test: a thought, a read, the engine's own plan, a
/// permission request, a write through the client, and a closing message.
fn exit_test_script() -> Value {
    json!({
        "initialize": { "agentInfo": {"name": "fake", "version": "0.0.1"}, "loadSession": false },
        "session": { "modes": { "currentModeId": "work", "availableModes": [
            {"id": "work", "name": "Work"}, {"id": "plan", "name": "Plan"} ] } },
        "turns": [{ "match": "notes", "actions": [
            {"thought": "Looking around"},
            {"tool_call": {"id": "t1", "title": "List the folder", "kind": "read",
                           "status": "completed", "locations": ["{{cwd}}"]}},
            {"plan": [{"content": "Write notes.txt", "priority": "high", "status": "in_progress"}]},
            {"request_permission": {"toolCallId": "t2", "title": "Create notes.txt",
                                    "kind": "edit", "locations": ["{{cwd}}/notes.txt"],
                                    "expect": "allow_once"}},
            {"fs_write": {"path": "{{cwd}}/notes.txt", "text": "hello from the fake engine"}},
            {"tool_call_update": {"id": "t2", "status": "completed"}},
            {"text": "Created notes.txt with one line."},
            {"stop": "end_turn"}
        ]}]
    })
}

fn write_script(dir: &Path, script: Value) -> std::path::PathBuf {
    let path = dir.join("script.json");
    std::fs::write(&path, script.to_string()).expect("write the script");
    path
}

/// Runs the CLI. It finds the fake agent beside its own executable, which is
/// where Cargo and an installer both put it; this only makes sure it is built.
/// PATH is deliberately left alone: replacing it breaks process creation on
/// Windows.
fn cli(args: &[&str]) -> Output {
    let _ = common::fake_agent();
    Command::new(env!("CARGO_BIN_EXE_eavery-cli"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run eavery-cli")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The M0 exit test.
#[test]
fn the_cli_round_trips_a_prompt_including_a_permission_request() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let script = write_script(dir.path(), exit_test_script());

    let output = cli(&[
        "prompt",
        "--engine",
        "fake",
        "--script",
        &script.to_string_lossy(),
        "--cwd",
        &project.to_string_lossy(),
        "--answer",
        "allow",
        "write some notes",
    ]);

    let printed = stdout(&output);
    assert!(output.status.success(), "the CLI failed:\n{printed}");

    for expected in [
        "engine   fake 0.0.1 (protocol v1",
        "session  sess_fake_1",
        "modes    [work] plan",
        "thought  Looking around",
        "tool     [completed] List the folder (read)",
        "plan     1 step(s)",
        "- [in_progress] Write notes.txt",
        "answer   AllowOnce for Create notes.txt",
        "text     Created notes.txt with one line.",
        "done     end_turn",
    ] {
        assert!(
            printed.contains(expected),
            "missing {expected:?} in:\n{printed}"
        );
    }

    assert_eq!(
        std::fs::read_to_string(project.join("notes.txt")).unwrap(),
        "hello from the fake engine",
        "the write should have gone through the client and landed in the project"
    );
}

/// The events must print in the order the engine sent them, not in whatever
/// order the tasks happened to run.
#[test]
fn the_transcript_is_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let script = write_script(dir.path(), exit_test_script());

    let output = cli(&[
        "prompt",
        "--script",
        &script.to_string_lossy(),
        "--cwd",
        &project.to_string_lossy(),
        "--answer",
        "allow",
        "write some notes",
    ]);
    let printed = stdout(&output);

    let positions: Vec<usize> = [
        "thought  ",
        "tool     [completed] List",
        "plan     ",
        "answer   ",
        "text     ",
        "done     ",
    ]
    .iter()
    .map(|needle| {
        printed
            .find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in:\n{printed}"))
    })
    .collect();

    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the transcript is out of order:\n{printed}"
    );
}

/// A rejection reaches the engine, and the fake agent proves it by exiting 3
/// when the answer is not what its script expected.
#[test]
fn rejecting_reaches_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let script = write_script(dir.path(), exit_test_script());

    let output = cli(&[
        "prompt",
        "--script",
        &script.to_string_lossy(),
        "--cwd",
        &project.to_string_lossy(),
        "--answer",
        "reject",
        "write some notes",
    ]);
    let printed = stdout(&output);
    assert!(
        printed.contains("answer   RejectOnce for Create notes.txt"),
        "{printed}"
    );
}

/// With no terminal and no `--answer`, the only safe answer is no. Saying yes
/// on an absent user's behalf is the one thing that must never happen.
#[test]
fn an_unattended_run_refuses_rather_than_assuming_consent() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let script = write_script(dir.path(), exit_test_script());

    let output = cli(&[
        "prompt",
        "--script",
        &script.to_string_lossy(),
        "--cwd",
        &project.to_string_lossy(),
        "write some notes",
    ]);
    let printed = stdout(&output);
    assert!(printed.contains("no terminal to ask on"), "{printed}");
    assert!(printed.contains("answer   RejectOnce"), "{printed}");
}

/// The engine runs in the project folder, so a script path relative to where
/// the user typed the command must be resolved before it is handed over. The
/// shipped demo script is quoted as a relative path in the README.
#[test]
fn a_relative_script_path_resolves_against_the_callers_directory() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();

    let output = cli(&[
        "prompt",
        "--script",
        // Relative to the crate root, which is the working directory Cargo
        // gives a test, and therefore the one the CLI inherits.
        "../eavery-core/tests/scripts/hello.json",
        "--cwd",
        &project.to_string_lossy(),
        "--answer",
        "allow",
        "write some notes",
    ]);
    let printed = stdout(&output);
    assert!(output.status.success(), "the CLI failed:\n{printed}");
    assert!(
        project.join("notes.txt").exists(),
        "the demo script did not run:\n{printed}"
    );
}

#[test]
fn a_missing_script_is_reported_before_anything_is_spawned() {
    let dir = tempfile::tempdir().unwrap();
    let output = cli(&[
        "prompt",
        "--script",
        &dir.path().join("nope.json").to_string_lossy(),
        "--cwd",
        &dir.path().to_string_lossy(),
        "anything",
    ]);
    assert!(!output.status.success());
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("no script at"), "{complaint}");
}

#[test]
fn asking_for_an_engine_that_does_not_exist_yet_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let output = cli(&[
        "prompt",
        "--engine",
        "goose",
        "--cwd",
        &dir.path().to_string_lossy(),
        "anything",
    ]);
    assert!(!output.status.success());
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(
        complaint.contains("only the `fake` engine exists"),
        "{complaint}"
    );
}

/// An engine that dies mid-turn fails the command, and what it said on the way
/// out is printed rather than swallowed.
#[test]
fn a_crashed_engine_fails_the_command_and_prints_its_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        json!({"turns": [{"actions": [{"text": "about to die"}, {"exit": 1}]}]}),
    );
    let output = cli(&[
        "prompt",
        "--script",
        &script.to_string_lossy(),
        "--cwd",
        &dir.path().to_string_lossy(),
        "crash please",
    ]);
    assert!(!output.status.success());
    let printed = stdout(&output);
    assert!(printed.contains("text     about to die"), "{printed}");
    assert!(printed.contains("error    "), "{printed}");
}
