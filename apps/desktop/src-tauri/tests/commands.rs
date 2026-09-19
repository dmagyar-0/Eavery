//! The IPC surface, over the real IPC path (`docs/plan/11-testing-ci.md` §3).
//!
//! Tauri's mock runtime runs the commands exactly as the window does —
//! through `register`, with the arguments deserialised and the answers
//! serialised — with no webview and no display, so this runs in CI on all
//! three OSes. What it is actually checking is that every command is
//! registered, takes the arguments the frontend sends, and answers in a shape
//! the frontend can read, including when it fails.

use std::path::{Path, PathBuf};

use eavery_desktop_lib::state::AppCore;
use serde_json::{Value, json};
use tauri::Manager;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{MockRuntime, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;

struct Fixture {
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp folder");
        let app = eavery_desktop_lib::register(mock_builder())
            .build(mock_context(noop_assets()))
            .expect("build the app");
        let core = AppCore::open(dir.path().join("data"), None).expect("open the core");
        app.manage(core);

        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("a webview");
        Self { app, webview, dir }
    }

    /// A folder with one file in it, ready to be opened as a Project.
    fn project_folder(&self) -> PathBuf {
        let root = self.dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("report.txt"), "FY25\n").unwrap();
        eavery_core::paths::canonicalize(&root).unwrap()
    }

    fn call(&self, command: &str, args: Value) -> Result<Value, Value> {
        tauri::test::get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().expect("a JSON answer"))
    }

    fn ok(&self, command: &str, args: Value) -> Value {
        self.call(command, args)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn err(&self, command: &str, args: Value) -> Value {
        self.call(command, args)
            .expect_err(&format!("{command} should have failed"))
    }
}

/// Every failure reaches the frontend as the same three fields, and the third
/// is the one that matters: Everyday mode renders an error as something to do
/// about it (`07-ui-vocabulary.md` §4).
fn assert_is_app_error(error: &Value) {
    assert!(
        error["code"].is_string(),
        "an error without a code: {error}"
    );
    assert!(
        error["message"].is_string(),
        "an error without a message: {error}"
    );
    assert!(
        error.get("next_action").is_some(),
        "an error without a next_action field: {error}"
    );
}

#[test]
fn opening_a_folder_makes_a_project_and_protects_it() {
    let fixture = Fixture::new();
    let root = fixture.project_folder();

    let project = fixture.ok("open_project", json!({ "path": root }));
    assert_eq!(project["name"], "project");
    assert!(project["id"].is_string());
    let project_id = project["id"].clone();

    // The Project folder itself stays clean: the git directory lives under
    // Eavery's data folder (`05-git-journal.md` §1).
    assert!(!Path::new(&root).join(".git").exists());

    let projects = fixture.ok("list_projects", json!({}));
    assert_eq!(projects.as_array().unwrap().len(), 1);

    // Opening the same folder again is the same Project, not a second one.
    let again = fixture.ok("open_project", json!({ "path": root }));
    assert_eq!(again["id"], project_id);
    assert_eq!(
        fixture
            .ok("list_projects", json!({}))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // And it has a checkpoint to go back to from the moment it is open.
    let checkpoints = fixture.ok("list_checkpoints", json!({ "projectId": project_id }));
    let checkpoints = checkpoints.as_array().unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0]["label"], "Project opened");
    assert!(checkpoints[0]["id"].is_string());

    assert!(
        fixture
            .ok("journal_size", json!({ "projectId": project_id }))
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        fixture
            .ok("unprotected_files", json!({ "projectId": project_id }))
            .as_array()
            .unwrap()
            .len(),
        0,
        "a folder with one small text file in it is completely protected"
    );
}

#[test]
fn a_checkpoint_can_be_taken_and_gone_back_to() {
    let fixture = Fixture::new();
    let root = fixture.project_folder();
    let project_id = fixture.ok("open_project", json!({ "path": root }))["id"].clone();
    let opened =
        fixture.ok("list_checkpoints", json!({ "projectId": project_id }))[0]["id"].clone();

    std::fs::write(Path::new(&root).join("report.txt"), "FY26\n").unwrap();

    // What has changed since the Project was opened, before anything is
    // committed: the user's own edit counts.
    let changes = fixture.ok(
        "diff_summary",
        json!({ "projectId": project_id, "from": opened }),
    );
    assert_eq!(changes["changed"], json!(["report.txt"]));

    let taken = fixture.ok(
        "checkpoint_now",
        json!({ "projectId": project_id, "label": "Before I try something" }),
    );
    assert_eq!(taken["kind"], "manual");

    let restored = fixture.ok(
        "restore_checkpoint",
        json!({ "projectId": project_id, "checkpointId": opened }),
    );
    assert_eq!(restored["checkpoint"]["kind"], "restore");
    assert_eq!(
        restored["skipped_locked"],
        json!([]),
        "nothing was open, so nothing was skipped"
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&root).join("report.txt")).unwrap(),
        "FY25\n",
        "the file is back as it was"
    );

    // And the way forward is still there: history only moves forward.
    let checkpoints = fixture.ok("list_checkpoints", json!({ "projectId": project_id }));
    let labels: Vec<&str> = checkpoints
        .as_array()
        .unwrap()
        .iter()
        .map(|checkpoint| checkpoint["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"Before I try something"), "{labels:?}");
}

#[test]
fn settings_round_trip() {
    let fixture = Fixture::new();

    let settings = fixture.ok("get_settings", json!({}));
    assert_eq!(settings["mode"], "everyday", "Everyday is the default");
    assert_eq!(settings["default_engine"], Value::Null);

    fixture.ok(
        "set_settings",
        json!({ "settings": { "mode": "developer", "default_engine": "goose" } }),
    );
    let settings = fixture.ok("get_settings", json!({}));
    assert_eq!(settings["mode"], "developer");
    assert_eq!(settings["default_engine"], "goose");
}

#[test]
fn the_assistants_are_listed_with_what_they_would_do_right_now() {
    let fixture = Fixture::new();
    let engines = fixture.ok("list_engines", json!({ "all": true }));
    let engines = engines.as_array().unwrap();

    assert!(!engines.is_empty());
    for engine in engines {
        assert!(engine["id"].is_string());
        assert!(
            engine["status"]["state"].is_string(),
            "every engine reports a state: {engine}"
        );
        assert!(
            !engine["display_name"].as_str().unwrap().is_empty(),
            "and a name a person could read"
        );
    }
}

#[test]
fn a_project_that_is_not_open_is_an_error_the_frontend_can_render() {
    let fixture = Fixture::new();
    let error = fixture.err(
        "list_checkpoints",
        json!({ "projectId": uuid::Uuid::new_v4() }),
    );
    assert_is_app_error(&error);
}

#[test]
fn going_back_to_a_checkpoint_that_does_not_exist_says_so() {
    let fixture = Fixture::new();
    let root = fixture.project_folder();
    let project_id = fixture.ok("open_project", json!({ "path": root }))["id"].clone();

    let error = fixture.err(
        "restore_checkpoint",
        json!({ "projectId": project_id, "checkpointId": "0".repeat(40) }),
    );
    assert_is_app_error(&error);
    assert_eq!(error["code"], "restore_failed");
}

/// Approval only ever comes through `approve_plan` / `reject_plan`, and an
/// answer to a plan nobody is waiting on means the window and the core
/// disagree about where the turn is — reported, never swallowed, and never
/// turned into a yes for some other turn.
#[test]
fn answering_a_plan_nobody_is_waiting_on_is_an_error() {
    let fixture = Fixture::new();
    let turn_id = uuid::Uuid::new_v4();

    let error = fixture.err(
        "approve_plan",
        json!({ "turnId": turn_id, "edits": "skip the cover page" }),
    );
    assert_is_app_error(&error);
    assert!(
        error["message"].as_str().unwrap().contains("plan"),
        "{error}"
    );
    let error = fixture.err("reject_plan", json!({ "turnId": turn_id }));
    assert_is_app_error(&error);
    assert_eq!(fixture.app.state::<AppCore>().plans().waiting(), 0);
}

/// The plan desk, from the core's side: what a plan-mode turn hands it is
/// what `approve_plan` answers, with the edits, and a second answer to the
/// same plan is the error above rather than a second yes.
#[test]
fn a_plan_waiting_on_the_desk_is_answered_by_the_command() {
    use eavery_core::turn::{Approval, PlanReview};

    let fixture = Fixture::new();
    let core = fixture.app.state::<AppCore>();
    let turn_id = uuid::Uuid::new_v4();
    let review = PlanReview {
        turn_id,
        plan: eavery_core::model::Plan {
            summary: "Update the report".into(),
            ..Default::default()
        },
        vendor: "local".into(),
    };

    // What the turn engine does: hand the plan to the desk and wait.
    let waiting = {
        let handler = core.plans().handler();
        let review = review.clone();
        std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move { handler(review).await })
        })
    };
    // Until the wait is registered there is nothing to answer.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while core.plans().waiting() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the desk never saw the plan"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    fixture.ok(
        "approve_plan",
        json!({ "turnId": turn_id, "edits": "skip the cover page" }),
    );
    assert_eq!(
        waiting.join().unwrap(),
        Approval::Approved {
            edits: Some("skip the cover page".into())
        }
    );
    assert_eq!(core.plans().waiting(), 0);
    let error = fixture.err("reject_plan", json!({ "turnId": turn_id }));
    assert_is_app_error(&error);
}

/// An answer to a question nobody asked means the window and the engine
/// disagree about what is pending, which is worth reporting rather than
/// swallowing.
#[test]
fn answering_a_permission_nobody_asked_for_is_an_error() {
    let fixture = Fixture::new();
    let error = fixture.err(
        "answer_permission",
        json!({ "requestId": "no-such-request", "decision": "allow_once" }),
    );
    assert_is_app_error(&error);
}

/// Stopping a turn that has already finished is not a failure: the composer
/// may still have its Stop button up when the last event arrives.
#[test]
fn stopping_a_turn_that_is_not_running_is_not_an_error() {
    let fixture = Fixture::new();
    fixture.ok("cancel_turn", json!({ "turnId": uuid::Uuid::new_v4() }));
}

#[test]
fn a_project_can_be_forgotten_without_losing_its_folder() {
    let fixture = Fixture::new();
    let root = fixture.project_folder();
    let project_id = fixture.ok("open_project", json!({ "path": root }))["id"].clone();

    fixture.ok("remove_project", json!({ "projectId": project_id }));
    assert_eq!(
        fixture
            .ok("list_projects", json!({}))
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(
        Path::new(&root).join("report.txt").exists(),
        "removing a Project must not touch the user's files"
    );

    // Opening it again brings its history back: the Journal was never deleted.
    let reopened = fixture.ok("open_project", json!({ "path": root }));
    assert_ne!(reopened["id"], project_id, "a new row for the same folder");
    let checkpoints = fixture.ok("list_checkpoints", json!({ "projectId": reopened["id"] }));
    assert!(
        !checkpoints.as_array().unwrap().is_empty(),
        "the folder's history is still there"
    );

    drop(fixture.app);
}

/// Developer mode names the git directory; it lives under Eavery's data folder
/// and never inside the Project (`05-git-journal.md` §1).
#[test]
fn the_journal_can_be_described() {
    let fixture = Fixture::new();
    let root = fixture.project_folder();
    let project_id = fixture.ok("open_project", json!({ "path": root }))["id"].clone();

    let info = fixture.ok("journal_info", json!({ "projectId": project_id }));
    let path = info["path"].as_str().expect("a path");
    assert!(
        Path::new(path).starts_with(fixture.dir.path().join("data")),
        "the history lives in the data folder: {path}"
    );
    assert!(!Path::new(path).starts_with(&root));
    assert!(info["size_bytes"].as_u64().unwrap() > 0);
    assert!(info["loose_objects"].as_u64().is_some());
}

/// The panel must answer on a fresh install, before anything has been logged,
/// and say where things are even then.
#[test]
fn diagnostics_answer_before_there_is_a_log() {
    let fixture = Fixture::new();
    let diagnostics = fixture.ok("diagnostics", json!({}));
    assert_eq!(diagnostics["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        Path::new(diagnostics["data_dir"].as_str().unwrap()),
        fixture.dir.path().join("data")
    );
    assert!(
        diagnostics["log_path"]
            .as_str()
            .unwrap()
            .ends_with("eavery.log")
    );
    assert_eq!(diagnostics["log_tail"], json!([]));

    // Once there is a log, the tail is the end of it.
    let data_dir = fixture.dir.path().join("data");
    let mut log = eavery_core::diagnostics::open_log(&data_dir).unwrap();
    use std::io::Write;
    for n in 1..=5 {
        writeln!(log, "line {n}").unwrap();
    }
    drop(log);
    let diagnostics = fixture.ok("diagnostics", json!({ "lines": 2 }));
    assert_eq!(diagnostics["log_tail"], json!(["line 4", "line 5"]));
}

/// The Documents pane's listing: the folder as names and paths, with the
/// folders the Journal ignores left out of it (M5-T04).
#[test]
fn the_project_folder_is_listed_for_the_documents_pane() {
    let fixture = Fixture::new();
    let root = fixture.project_folder();
    std::fs::create_dir_all(Path::new(&root).join("reports/2026")).unwrap();
    std::fs::write(Path::new(&root).join("reports/2026/q1.txt"), "Q1\n").unwrap();
    std::fs::create_dir_all(Path::new(&root).join("node_modules/left-pad")).unwrap();
    std::fs::write(
        Path::new(&root).join("node_modules/left-pad/index.js"),
        "x\n",
    )
    .unwrap();

    let project_id = fixture.ok("open_project", json!({ "path": root }))["id"].clone();
    let tree = fixture.ok("list_documents", json!({ "projectId": project_id }));

    let entries = tree["entries"].as_array().unwrap();
    let names: Vec<&str> = entries
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["reports", "report.txt"],
        "folders first, and node_modules is not a document"
    );
    assert_eq!(tree["truncated"], false);
    assert_eq!(tree["files"], 2);

    // A nested file carries the path the digest would name it by, so the
    // pane can mark it as changed by comparing the two.
    let buried = &entries[0]["children"][0]["children"][0];
    assert_eq!(buried["path"], "reports/2026/q1.txt");
    assert_eq!(buried["directory"], false);
}

#[test]
fn listing_the_folder_of_a_project_that_is_not_open_is_an_error() {
    let fixture = Fixture::new();
    let error = fixture.err(
        "list_documents",
        json!({ "projectId": uuid::Uuid::new_v4().to_string() }),
    );
    assert_is_app_error(&error);
}
