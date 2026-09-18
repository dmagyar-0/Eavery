//! The store's tests (`docs/plan/11-testing-ci.md` §3): migrations and CRUD,
//! against a real database in a temporary directory. SQLite is not mocked —
//! half of what is being tested here is what SQLite itself enforces.

use std::path::Path;

use chrono::{TimeZone, Utc};
use eavery_core::event::{CoreEvent, DecidedBy, Digest};
use eavery_core::model::{
    Checkpoint, CheckpointKind, Plan, PlanStep, Project, ProjectId, RiskClass, Session, SessionId,
    Turn, TurnPhase,
};
use eavery_core::store::{DB_FILE, NewAudit, Store, StoreError, schema_version};

fn project_at(root: impl AsRef<Path>) -> Project {
    Project {
        id: uuid::Uuid::new_v4(),
        name: "Month end".into(),
        root: root.as_ref().to_path_buf(),
        created_at: Utc.with_ymd_and_hms(2026, 9, 17, 9, 0, 0).unwrap(),
        engine_id: None,
    }
}

fn session_for(project_id: ProjectId) -> Session {
    Session {
        id: uuid::Uuid::new_v4(),
        project_id,
        engine_id: "goose".into(),
        engine_session_id: None,
        created_at: Utc.with_ymd_and_hms(2026, 9, 17, 9, 1, 0).unwrap(),
    }
}

fn turn_for(session_id: SessionId) -> Turn {
    Turn {
        id: uuid::Uuid::new_v4(),
        session_id,
        request: "Rename every FY25 to FY26".into(),
        phase: TurnPhase::Planning,
        plan: None,
        pre_checkpoint: None,
        post_checkpoint: None,
        started_at: Utc.with_ymd_and_hms(2026, 9, 17, 9, 2, 0).unwrap(),
    }
}

fn checkpoint_for(project_id: ProjectId, id: &str) -> Checkpoint {
    Checkpoint {
        id: id.into(),
        project_id,
        turn_id: None,
        label: "Before: rename FY25 → FY26".into(),
        kind: CheckpointKind::PreTurn,
        created_at: Utc.with_ymd_and_hms(2026, 9, 17, 9, 2, 0).unwrap(),
        files_changed: 3,
    }
}

/// A Store, a Project, a Session and a Turn, all saved. Most tests need the
/// whole chain because the schema will not have a Turn without a Session.
struct Fixture {
    store: Store,
    project: Project,
    session: Session,
    turn: Turn,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a data folder");
        let store = Store::open_in_data_dir(dir.path()).expect("open the store");
        let project = project_at(dir.path().join("project"));
        let session = session_for(project.id);
        let turn = turn_for(session.id);
        store.insert_project(&project).expect("insert the project");
        store.insert_session(&session).expect("insert the session");
        store.insert_turn(&turn).expect("insert the turn");
        Self {
            store,
            project,
            session,
            turn,
            _dir: dir,
        }
    }
}

// ---- migrations ------------------------------------------------------------

#[test]
fn opening_a_new_database_applies_every_migration() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_data_dir(dir.path()).unwrap();

    assert_eq!(store.version().unwrap(), schema_version());
    assert!(dir.path().join(DB_FILE).exists());
}

/// Opening again must be a no-op, not a second run of the migrations, and
/// everything written before must still be there.
#[test]
fn opening_an_existing_database_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let project = {
        let store = Store::open_in_data_dir(dir.path()).unwrap();
        let project = project_at(dir.path().join("project"));
        store.insert_project(&project).unwrap();
        project
    };

    let store = Store::open_in_data_dir(dir.path()).unwrap();
    assert_eq!(store.version().unwrap(), schema_version());
    assert_eq!(store.list_projects().unwrap().len(), 1);
    assert_eq!(
        store.project(project.id).unwrap().unwrap().name,
        "Month end"
    );
}

/// A database from a newer Eavery is refused rather than written to. Reading
/// it with today's queries would be a guess, and the guess would be about
/// someone's records.
#[test]
fn a_database_from_a_newer_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE);
    {
        let store = Store::open(&path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", schema_version() as i64 + 1)
            .unwrap();
    }

    let error = Store::open(&path).expect_err("a newer schema is refused");
    assert!(
        matches!(error, StoreError::SchemaTooNew { .. }),
        "wrong error: {error}"
    );
    assert!(error.next_action().is_some(), "the user is told what to do");
}

// ---- projects --------------------------------------------------------------

#[test]
fn a_project_round_trips() {
    let fixture = Fixture::new();
    let stored = fixture.store.project(fixture.project.id).unwrap().unwrap();

    assert_eq!(stored.id, fixture.project.id);
    assert_eq!(stored.name, fixture.project.name);
    assert_eq!(stored.root, fixture.project.root);
    assert_eq!(stored.created_at, fixture.project.created_at);
    assert_eq!(stored.engine_id, None);
}

#[test]
fn a_folder_can_only_be_opened_as_one_project() {
    let fixture = Fixture::new();
    let again = project_at(&fixture.project.root);

    assert!(fixture.store.insert_project(&again).is_err());
    assert_eq!(
        fixture
            .store
            .project_by_root(&fixture.project.root)
            .unwrap()
            .unwrap()
            .id,
        fixture.project.id
    );
}

#[test]
fn a_folder_that_was_never_opened_has_no_project() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .store
            .project_by_root(Path::new("/nowhere/at/all"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_engine_a_project_last_used_is_remembered() {
    let fixture = Fixture::new();
    fixture
        .store
        .set_project_engine(fixture.project.id, Some("goose"))
        .unwrap();

    assert_eq!(
        fixture
            .store
            .project(fixture.project.id)
            .unwrap()
            .unwrap()
            .engine_id
            .as_deref(),
        Some("goose")
    );
}

#[test]
fn setting_the_engine_of_a_project_that_is_not_there_says_so() {
    let fixture = Fixture::new();
    let error = fixture
        .store
        .set_project_engine(uuid::Uuid::new_v4(), Some("goose"))
        .expect_err("no such project");
    assert!(matches!(error, StoreError::NotFound { .. }), "{error}");
}

#[test]
fn projects_are_listed_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_data_dir(dir.path()).unwrap();

    let mut older = project_at(dir.path().join("older"));
    older.created_at = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let mut newer = project_at(dir.path().join("newer"));
    newer.created_at = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
    store.insert_project(&older).unwrap();
    store.insert_project(&newer).unwrap();

    let ids: Vec<_> = store
        .list_projects()
        .unwrap()
        .into_iter()
        .map(|project| project.id)
        .collect();
    assert_eq!(ids, vec![newer.id, older.id]);
}

/// Removing a Project forgets the conversation. The files and the Journal are
/// not this crate's to delete, and the audit log is nobody's.
#[test]
fn removing_a_project_takes_its_conversation_with_it() {
    let fixture = Fixture::new();
    fixture
        .store
        .append_event(
            fixture.session.id,
            &CoreEvent::TurnStarted {
                turn_id: fixture.turn.id,
                phase: TurnPhase::Planning,
            },
        )
        .unwrap();
    fixture
        .store
        .upsert_checkpoint(&checkpoint_for(fixture.project.id, "abc123"))
        .unwrap();
    fixture
        .store
        .append_audit(&NewAudit::new(DecidedBy::User, "allow_once").for_project(fixture.project.id))
        .unwrap();

    fixture.store.remove_project(fixture.project.id).unwrap();

    assert!(fixture.store.project(fixture.project.id).unwrap().is_none());
    assert!(fixture.store.session(fixture.session.id).unwrap().is_none());
    assert!(fixture.store.turn(fixture.turn.id).unwrap().is_none());
    assert!(
        fixture
            .store
            .list_events(fixture.session.id, None, None)
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .store
            .checkpoints_for_project(fixture.project.id, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .store
            .list_audit(Some(fixture.project.id), None)
            .unwrap()
            .len(),
        1,
        "the decisions taken about a removed Project stay on the record"
    );
}

/// Paths are stored as text. A name that is not Unicode is refused at the door
/// rather than stored as something else and handed back as a folder nobody has.
#[cfg(unix)]
#[test]
fn a_project_root_that_is_not_unicode_is_refused() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    let fixture = Fixture::new();
    let root = PathBuf::from(OsString::from_vec(b"/tmp/\xff\xfe".to_vec()));
    let error = fixture
        .store
        .insert_project(&project_at(&root))
        .expect_err("a non-Unicode root is refused");

    assert!(matches!(error, StoreError::NonUtf8Path(_)), "{error}");
    assert!(error.next_action().is_some());
}

// ---- sessions and turns ----------------------------------------------------

#[test]
fn a_session_round_trips_and_remembers_the_engines_own_id() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .store
            .session(fixture.session.id)
            .unwrap()
            .unwrap()
            .engine_session_id,
        None
    );

    fixture
        .store
        .set_engine_session_id(fixture.session.id, Some("acp-session-7"))
        .unwrap();

    let stored = fixture.store.session(fixture.session.id).unwrap().unwrap();
    assert_eq!(stored.engine_session_id.as_deref(), Some("acp-session-7"));
    assert_eq!(stored.project_id, fixture.project.id);
    assert_eq!(stored.created_at, fixture.session.created_at);
}

#[test]
fn the_latest_session_is_the_one_to_resume() {
    let fixture = Fixture::new();
    let mut later = session_for(fixture.project.id);
    later.created_at = Utc.with_ymd_and_hms(2026, 9, 18, 9, 0, 0).unwrap();
    fixture.store.insert_session(&later).unwrap();

    assert_eq!(
        fixture
            .store
            .latest_session(fixture.project.id)
            .unwrap()
            .unwrap()
            .id,
        later.id
    );
    assert_eq!(
        fixture
            .store
            .sessions_for_project(fixture.project.id)
            .unwrap()
            .len(),
        2
    );
}

/// The schema will not hold a Session whose Project is not there. Without
/// `PRAGMA foreign_keys = ON` this insert succeeds, which is why the test is
/// about the store and not about SQLite.
#[test]
fn a_session_without_a_project_is_refused() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .store
            .insert_session(&session_for(uuid::Uuid::new_v4()))
            .is_err()
    );
}

#[test]
fn a_turn_round_trips_through_every_phase_with_its_plan() {
    let fixture = Fixture::new();
    let mut turn = fixture.turn.clone();
    turn.phase = TurnPhase::AwaitingApproval;
    turn.plan = Some(Plan {
        summary: "Update the report".into(),
        steps: vec![PlanStep::new("Open report"), PlanStep::new("FY25 → FY26")],
        files_touched: vec!["report.docx".into()],
        will_not_do: vec!["send any email".into()],
        raw_markdown: "I will update the report.".into(),
        ..Plan::default()
    });
    turn.pre_checkpoint = Some("abc123".into());
    fixture.store.update_turn(&turn).unwrap();

    let stored = fixture.store.turn(turn.id).unwrap().unwrap();
    assert_eq!(stored.phase, TurnPhase::AwaitingApproval);
    assert_eq!(stored.plan, turn.plan);
    assert_eq!(stored.pre_checkpoint.as_deref(), Some("abc123"));
    assert_eq!(stored.post_checkpoint, None);
    assert_eq!(stored.started_at, turn.started_at);

    turn.phase = TurnPhase::Done;
    turn.post_checkpoint = Some("def456".into());
    fixture.store.update_turn(&turn).unwrap();
    let stored = fixture.store.turn(turn.id).unwrap().unwrap();
    assert_eq!(stored.phase, TurnPhase::Done);
    assert_eq!(stored.post_checkpoint.as_deref(), Some("def456"));
}

#[test]
fn updating_a_turn_that_is_not_there_says_so() {
    let fixture = Fixture::new();
    let error = fixture
        .store
        .update_turn(&turn_for(fixture.session.id))
        .expect_err("no such turn");
    assert!(matches!(error, StoreError::NotFound { .. }), "{error}");
}

#[test]
fn a_sessions_turns_read_forwards() {
    let fixture = Fixture::new();
    let mut second = turn_for(fixture.session.id);
    second.started_at = Utc.with_ymd_and_hms(2026, 9, 17, 10, 0, 0).unwrap();
    fixture.store.insert_turn(&second).unwrap();

    let ids: Vec<_> = fixture
        .store
        .turns_for_session(fixture.session.id)
        .unwrap()
        .into_iter()
        .map(|turn| turn.id)
        .collect();
    assert_eq!(ids, vec![fixture.turn.id, second.id]);
}

// ---- events ----------------------------------------------------------------

#[test]
fn events_keep_their_order_and_their_turn() {
    let fixture = Fixture::new();
    let first = fixture
        .store
        .append_event(
            fixture.session.id,
            &CoreEvent::TurnStarted {
                turn_id: fixture.turn.id,
                phase: TurnPhase::Planning,
            },
        )
        .unwrap();
    let second = fixture
        .store
        .append_event(
            fixture.session.id,
            &CoreEvent::AgentText {
                turn_id: fixture.turn.id,
                text: "Looking at the report".into(),
            },
        )
        .unwrap();

    assert!(second.seq > first.seq, "seq goes forwards");
    assert_eq!(first.turn_id, Some(fixture.turn.id));

    let stored = fixture
        .store
        .list_events(fixture.session.id, None, None)
        .unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].seq, first.seq);
    assert!(matches!(stored[0].event, CoreEvent::TurnStarted { .. }));
    match &stored[1].event {
        CoreEvent::AgentText { text, .. } => assert_eq!(text, "Looking at the report"),
        other => panic!("wrong event: {other:?}"),
    }
}

/// An event carrying a whole `Digest` is the one that has to survive a restart
/// intact: it is what the Undo button is drawn from.
#[test]
fn an_event_comes_back_as_the_event_it_was() {
    let fixture = Fixture::new();
    let finished = CoreEvent::TurnFinished {
        turn_id: fixture.turn.id,
        stop_reason: "end_turn".into(),
        digest: Some(Digest {
            files_changed: vec!["report.docx".into()],
            outbound_actions: vec![],
            undo_to: Some("abc123".into()),
            ..Digest::default()
        }),
    };
    fixture
        .store
        .append_event(fixture.session.id, &finished)
        .unwrap();

    let stored = fixture
        .store
        .list_events(fixture.session.id, None, None)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&stored[0].event).unwrap(),
        serde_json::to_value(&finished).unwrap()
    );
}

/// The gap case from `03-architecture.md` §7: the UI missed some events and
/// asks for everything after the last `seq` it has.
#[test]
fn a_ui_that_missed_events_can_ask_for_what_it_missed() {
    let fixture = Fixture::new();
    let mut seqs = Vec::new();
    for index in 0..5 {
        seqs.push(
            fixture
                .store
                .append_event(
                    fixture.session.id,
                    &CoreEvent::AgentText {
                        turn_id: fixture.turn.id,
                        text: format!("chunk {index}"),
                    },
                )
                .unwrap()
                .seq,
        );
    }

    let missed = fixture
        .store
        .list_events(fixture.session.id, Some(seqs[1]), None)
        .unwrap();
    assert_eq!(
        missed.iter().map(|event| event.seq).collect::<Vec<_>>(),
        seqs[2..].to_vec()
    );

    let two = fixture
        .store
        .list_events(fixture.session.id, Some(seqs[1]), Some(2))
        .unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].seq, seqs[2]);
}

/// `seq` is global, so a Project's transcript is a filtered view of one
/// sequence rather than a sequence of its own.
#[test]
fn one_sessions_events_do_not_appear_in_another() {
    let fixture = Fixture::new();
    let other = session_for(fixture.project.id);
    fixture.store.insert_session(&other).unwrap();

    let mine = fixture
        .store
        .append_event(
            fixture.session.id,
            &CoreEvent::AgentText {
                turn_id: fixture.turn.id,
                text: "mine".into(),
            },
        )
        .unwrap();
    let theirs = fixture
        .store
        .append_event(
            other.id,
            &CoreEvent::AgentText {
                turn_id: fixture.turn.id,
                text: "theirs".into(),
            },
        )
        .unwrap();

    assert_ne!(mine.seq, theirs.seq);
    let stored = fixture
        .store
        .list_events(fixture.session.id, None, None)
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].seq, mine.seq);
}

/// A restore happens between turns; an engine's status is not about one. Both
/// are stored with no turn rather than with a wrong one.
#[test]
fn an_event_that_belongs_to_no_turn_is_stored_without_one() {
    let fixture = Fixture::new();
    fixture
        .store
        .append_event(
            fixture.session.id,
            &CoreEvent::Restored {
                to: "abc123".into(),
                new_checkpoint: "def456".into(),
                skipped_locked: vec!["Budget.xlsx".into()],
            },
        )
        .unwrap();

    let stored = fixture
        .store
        .list_events(fixture.session.id, None, None)
        .unwrap();
    assert_eq!(stored[0].turn_id, None);
}

// ---- checkpoints -----------------------------------------------------------

/// The Journal is the truth and the rows here are a cache of it, so writing
/// the same checkpoint twice must converge instead of failing.
#[test]
fn caching_the_same_checkpoint_twice_updates_it() {
    let fixture = Fixture::new();
    let mut checkpoint = checkpoint_for(fixture.project.id, "abc123");
    fixture.store.upsert_checkpoint(&checkpoint).unwrap();

    checkpoint.label = "After: rename FY25 → FY26".into();
    checkpoint.kind = CheckpointKind::PostTurn;
    checkpoint.turn_id = Some(fixture.turn.id);
    checkpoint.files_changed = 7;
    fixture.store.upsert_checkpoint(&checkpoint).unwrap();

    let stored = fixture.store.checkpoint(&checkpoint.id).unwrap().unwrap();
    assert_eq!(stored.label, "After: rename FY25 → FY26");
    assert_eq!(stored.kind, CheckpointKind::PostTurn);
    assert_eq!(stored.turn_id, Some(fixture.turn.id));
    assert_eq!(stored.files_changed, 7);
    assert_eq!(
        fixture
            .store
            .checkpoints_for_project(fixture.project.id, None)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn checkpoints_are_listed_newest_first_and_can_be_limited() {
    let fixture = Fixture::new();
    let mut older = checkpoint_for(fixture.project.id, "aaa");
    older.created_at = Utc.with_ymd_and_hms(2026, 9, 17, 9, 0, 0).unwrap();
    let mut newer = checkpoint_for(fixture.project.id, "bbb");
    newer.created_at = Utc.with_ymd_and_hms(2026, 9, 17, 11, 0, 0).unwrap();
    fixture.store.upsert_checkpoint(&older).unwrap();
    fixture.store.upsert_checkpoint(&newer).unwrap();

    let all = fixture
        .store
        .checkpoints_for_project(fixture.project.id, None)
        .unwrap();
    assert_eq!(
        all.iter().map(|cp| cp.id.as_str()).collect::<Vec<_>>(),
        vec!["bbb", "aaa"]
    );

    let one = fixture
        .store
        .checkpoints_for_project(fixture.project.id, Some(1))
        .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].id, "bbb");
}

// ---- audit -----------------------------------------------------------------

#[test]
fn an_audit_row_round_trips() {
    let fixture = Fixture::new();
    let written = fixture
        .store
        .append_audit(
            &NewAudit::new(DecidedBy::Policy, "allow_once")
                .for_project(fixture.project.id)
                .for_turn(fixture.turn.id)
                .with_risk(RiskClass::Reversible)
                .with_detail(serde_json::json!({ "path": "report.docx" })),
        )
        .unwrap();

    let stored = fixture.store.list_audit(None, None).unwrap();
    assert_eq!(stored.len(), 1);
    let entry = &stored[0];
    assert_eq!(entry.seq, written.seq);
    assert_eq!(entry.actor, DecidedBy::Policy);
    assert_eq!(entry.action, "allow_once");
    assert_eq!(entry.risk, Some(RiskClass::Reversible));
    assert_eq!(entry.project_id, Some(fixture.project.id));
    assert_eq!(entry.turn_id, Some(fixture.turn.id));
    assert_eq!(entry.detail["path"], "report.docx");
}

#[test]
fn the_audit_log_reads_newest_first_and_can_be_filtered_by_project() {
    let fixture = Fixture::new();
    let elsewhere = project_at(fixture.project.root.join("..").join("other"));
    fixture.store.insert_project(&elsewhere).unwrap();

    fixture
        .store
        .append_audit(&NewAudit::new(DecidedBy::User, "first").for_project(fixture.project.id))
        .unwrap();
    fixture
        .store
        .append_audit(&NewAudit::new(DecidedBy::User, "elsewhere").for_project(elsewhere.id))
        .unwrap();
    fixture
        .store
        .append_audit(&NewAudit::new(DecidedBy::PlanGate, "second").for_project(fixture.project.id))
        .unwrap();

    let mine = fixture
        .store
        .list_audit(Some(fixture.project.id), None)
        .unwrap();
    assert_eq!(
        mine.iter()
            .map(|entry| entry.action.as_str())
            .collect::<Vec<_>>(),
        vec!["second", "first"]
    );
    assert_eq!(fixture.store.list_audit(None, None).unwrap().len(), 3);
    assert_eq!(fixture.store.list_audit(None, Some(2)).unwrap().len(), 2);
}

/// The append-only rule is the database's, not a convention in the calling
/// code: a record of decisions that can be quietly rewritten is not a record.
#[test]
fn the_audit_log_cannot_be_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB_FILE);
    let store = Store::open(&path).unwrap();
    store
        .append_audit(&NewAudit::new(DecidedBy::User, "allow_always"))
        .unwrap();
    drop(store);

    let conn = rusqlite::Connection::open(&path).unwrap();
    let updated = conn.execute("UPDATE audit SET action = 'reject_once'", []);
    let deleted = conn.execute("DELETE FROM audit", []);

    assert!(updated.is_err(), "an audit row was rewritten");
    assert!(deleted.is_err(), "an audit row was deleted");
    assert_eq!(
        conn.query_row("SELECT action FROM audit", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "allow_always"
    );
}

// ---- settings --------------------------------------------------------------

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct UiSettings {
    everyday: bool,
    default_engine: Option<String>,
}

#[test]
fn a_setting_round_trips_as_the_type_it_was_written_as() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.store.setting::<UiSettings>("ui").unwrap(),
        None,
        "a setting nobody has written has no value"
    );

    let settings = UiSettings {
        everyday: true,
        default_engine: Some("goose".into()),
    };
    fixture.store.set_setting("ui", &settings).unwrap();
    assert_eq!(
        fixture.store.setting::<UiSettings>("ui").unwrap(),
        Some(settings)
    );

    let changed = UiSettings {
        everyday: false,
        default_engine: None,
    };
    fixture.store.set_setting("ui", &changed).unwrap();
    assert_eq!(
        fixture.store.setting::<UiSettings>("ui").unwrap(),
        Some(changed)
    );

    fixture.store.set_setting("mode", &"developer").unwrap();
    assert_eq!(fixture.store.setting_keys().unwrap(), vec!["mode", "ui"]);

    fixture.store.remove_setting("ui").unwrap();
    assert_eq!(fixture.store.setting::<UiSettings>("ui").unwrap(), None);
    assert_eq!(fixture.store.setting_keys().unwrap(), vec!["mode"]);
}

/// The in-memory database exists so the tests above can be written without a
/// temp folder; it has to be the same database.
#[test]
fn an_in_memory_store_has_the_same_schema() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.version().unwrap(), schema_version());

    let project = project_at(Path::new("/tmp/in-memory"));
    store.insert_project(&project).unwrap();
    assert_eq!(store.list_projects().unwrap().len(), 1);
}

/// One Store, several threads: the turn engine writes events from the task
/// driving the engine while the UI reads the transcript.
#[test]
fn a_store_can_be_shared_between_threads() {
    let fixture = Fixture::new();
    let store = std::sync::Arc::new(fixture.store);
    let session_id = fixture.session.id;
    let turn_id = fixture.turn.id;

    let writers: Vec<_> = (0..4)
        .map(|worker| {
            let store = store.clone();
            std::thread::spawn(move || {
                for index in 0..25 {
                    store
                        .append_event(
                            session_id,
                            &CoreEvent::AgentText {
                                turn_id,
                                text: format!("{worker}-{index}"),
                            },
                        )
                        .unwrap();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }

    let stored = store.list_events(session_id, None, None).unwrap();
    assert_eq!(stored.len(), 100);
    let seqs: Vec<_> = stored.iter().map(|event| event.seq).collect();
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(seqs, sorted, "seq is unique and increasing");
}
