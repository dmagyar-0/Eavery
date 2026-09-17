//! Persistence: one SQLite database per user (decision D8).
//!
//! What lives here is the record of what happened — Projects, Sessions, Turns,
//! every [`CoreEvent`] as it arrives, the audit log, and settings. What does
//! *not* live here is anything the product would be wrong without: the user's
//! files are theirs, and the history that protects them is the Journal's git
//! objects. Losing this database costs the transcript and the settings, not a
//! single byte of anyone's work. The `checkpoints` table says so out loud: it
//! is a cache of what git already knows (`docs/plan/05-git-journal.md` §3).
//!
//! Two rules the schema enforces rather than documents:
//!
//! - **The audit log is append-only.** Two triggers make an UPDATE or a DELETE
//!   on `audit` fail. A record of decisions that can be quietly rewritten is
//!   not a record.
//! - **Removing a Project removes its conversation, not its decisions.**
//!   Sessions, Turns, events and checkpoint rows cascade; audit rows do not.
//!
//! See `docs/plan/03-architecture.md` §8 for the schema this implements.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};
use ts_rs::TS;

use crate::event::{CoreEvent, DecidedBy};
use crate::model::{
    Checkpoint, CheckpointId, CheckpointKind, Plan, Project, ProjectId, RiskClass, Session,
    SessionId, Turn, TurnId, TurnPhase,
};

/// The database file inside Eavery's data directory.
pub const DB_FILE: &str = "eavery.sqlite";

struct Migration {
    name: &'static str,
    sql: &'static str,
}

/// Applied in order; the count is the schema version. A migration that has
/// shipped is never edited — the next one is added instead.
const MIGRATIONS: &[Migration] = &[Migration {
    name: "0001_initial",
    sql: include_str!("migrations/0001_initial.sql"),
}];

/// The schema version this build writes and understands.
pub fn schema_version() -> u32 {
    MIGRATIONS.len() as u32
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the database could not be used: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A row that cannot be turned back into the type it came from. Either the
    /// file was edited by hand or a type changed shape without a migration.
    #[error("a stored {what} could not be read back: {source}")]
    Corrupt {
        what: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(
        "this database was written by a newer version of Eavery \
         (schema {found}; this build understands {understood})"
    )]
    SchemaTooNew { found: u32, understood: u32 },
    /// Paths are stored as text, so a path that is not Unicode cannot be
    /// stored without changing into something else on the way back.
    #[error("{0} is not valid Unicode, so it cannot be stored")]
    NonUtf8Path(PathBuf),
    #[error("there is no {kind} with id {id}")]
    NotFound { kind: &'static str, id: String },
}

impl StoreError {
    fn corrupt(
        what: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        StoreError::Corrupt {
            what: what.into(),
            source: Box::new(source),
        }
    }

    fn not_found(kind: &'static str, id: impl std::fmt::Display) -> Self {
        StoreError::NotFound {
            kind,
            id: id.to_string(),
        }
    }

    /// What the user can do about it. Errors are rendered as next actions
    /// (`docs/plan/07-ui-vocabulary.md` §4), so anything with an answer says it.
    pub fn next_action(&self) -> Option<String> {
        match self {
            StoreError::SchemaTooNew { .. } => {
                Some("Update Eavery to the latest version and open it again.".into())
            }
            StoreError::NonUtf8Path(_) => Some(
                "Rename the folder so its name uses ordinary characters, then open it again."
                    .into(),
            ),
            StoreError::Io { .. } => Some(
                "Check that there is free space on the disk and that Eavery may write to its own \
                 data folder."
                    .into(),
            ),
            _ => None,
        }
    }
}

/// An event as it was stored: the event itself, plus where it sits in the one
/// global sequence the UI counts on.
#[derive(Clone, Debug, Serialize, serde::Deserialize, TS)]
pub struct StoredEvent {
    /// Global across every Session, so a gap is visible to a UI watching one
    /// Project (`docs/plan/03-architecture.md` §7).
    #[ts(type = "number")]
    pub seq: u64,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub at: DateTime<Utc>,
    pub event: CoreEvent,
}

/// One decision, on the record. Written by the policy, the plan gate, or the
/// user; never rewritten by any of them.
#[derive(Clone, Debug, Serialize, serde::Deserialize, TS)]
pub struct AuditEntry {
    #[ts(type = "number")]
    pub seq: u64,
    pub at: DateTime<Utc>,
    pub project_id: Option<ProjectId>,
    pub turn_id: Option<TurnId>,
    pub actor: DecidedBy,
    /// What was decided, in a stable machine-readable form: "allow_once",
    /// "plan_approved", "restore". The UI translates; the log does not.
    pub action: String,
    pub risk: Option<RiskClass>,
    /// Whatever the caller wants kept with the decision: the tool call, the
    /// paths, the host that was contacted.
    #[ts(type = "unknown")]
    pub detail: serde_json::Value,
}

/// An audit row before it has a sequence number and a time.
#[derive(Clone, Debug)]
pub struct NewAudit {
    pub project_id: Option<ProjectId>,
    pub turn_id: Option<TurnId>,
    pub actor: DecidedBy,
    pub action: String,
    pub risk: Option<RiskClass>,
    pub detail: serde_json::Value,
}

impl NewAudit {
    pub fn new(actor: DecidedBy, action: impl Into<String>) -> Self {
        Self {
            project_id: None,
            turn_id: None,
            actor,
            action: action.into(),
            risk: None,
            detail: serde_json::Value::Null,
        }
    }

    pub fn for_project(mut self, project_id: ProjectId) -> Self {
        self.project_id = Some(project_id);
        self
    }

    pub fn for_turn(mut self, turn_id: TurnId) -> Self {
        self.turn_id = Some(turn_id);
        self
    }

    pub fn with_risk(mut self, risk: RiskClass) -> Self {
        self.risk = Some(risk);
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }
}

/// The database. Cheap to clone by reference, safe to share: every call takes
/// the one connection's lock for as long as the statement runs.
pub struct Store {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Opens `<data_dir>/eavery.sqlite`, creating the data directory if it is
    /// not there yet.
    pub fn open_in_data_dir(data_dir: &Path) -> Result<Store, StoreError> {
        std::fs::create_dir_all(data_dir).map_err(|source| StoreError::Io {
            path: data_dir.to_path_buf(),
            source,
        })?;
        Store::open(data_dir.join(DB_FILE))
    }

    /// Opens (or creates) the database at `path` and brings its schema up to
    /// date.
    pub fn open(path: impl AsRef<Path>) -> Result<Store, StoreError> {
        let conn = Connection::open(path.as_ref())?;
        // WAL survives a crash without corruption and lets a reader run while a
        // write is in flight; `NORMAL` is its matching durability setting. The
        // worst a power cut can cost is the last few events, and the events are
        // a transcript, not the user's files.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Store::from_connection(conn)
    }

    /// A database that exists only for the length of the test that made it.
    pub fn open_in_memory() -> Result<Store, StoreError> {
        Store::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Store, StoreError> {
        // Without this the cascades in the schema are decoration: SQLite turns
        // foreign keys off by default, per connection.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&conn)?;
        Ok(Store {
            conn: Mutex::new(conn),
        })
    }

    /// The schema version of the open database. Equal to [`schema_version`]
    /// after a successful open.
    pub fn version(&self) -> Result<u32, StoreError> {
        let conn = self.lock();
        user_version(&conn)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic inside a statement leaves the connection usable: rusqlite
        // rolls back an unfinished transaction when its guard drops. Refusing
        // to open the database again after one panic would be the worse bug.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ---- projects ----------------------------------------------------------

    /// Inserts a Project. Fails if the id or the root is already known; use
    /// [`Store::project_by_root`] first when opening a folder.
    pub fn insert_project(&self, project: &Project) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO projects (id, name, root, created_at, engine_id) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                project.id.to_string(),
                project.name,
                path_text(&project.root)?,
                time_text(project.created_at),
                project.engine_id,
            ],
        )?;
        Ok(())
    }

    pub fn project(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, name, root, created_at, engine_id FROM projects WHERE id = ?1",
            params![id.to_string()],
            |row| Ok(project_from_row(row)),
        )
        .optional()?
        .transpose()
    }

    /// The Project whose root is this folder, if it has been opened before.
    pub fn project_by_root(&self, root: impl AsRef<Path>) -> Result<Option<Project>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, name, root, created_at, engine_id FROM projects WHERE root = ?1",
            params![path_text(root.as_ref())?],
            |row| Ok(project_from_row(row)),
        )
        .optional()?
        .transpose()
    }

    /// Newest first, which is the order the Home screen wants.
    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, root, created_at, engine_id FROM projects \
             ORDER BY created_at DESC, rowid DESC",
        )?;
        collect(stmt.query([])?, project_from_row)
    }

    /// Remembers the engine this Project was last driven with.
    pub fn set_project_engine(
        &self,
        id: ProjectId,
        engine_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE projects SET engine_id = ?2 WHERE id = ?1",
            params![id.to_string(), engine_id],
        )?;
        if changed == 0 {
            return Err(StoreError::not_found("Project", id));
        }
        Ok(())
    }

    /// Forgets a Project and its conversation. Touches neither the user's
    /// files nor the Journal: reopening the same folder brings its whole
    /// history back.
    pub fn remove_project(&self, id: ProjectId) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM projects WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    // ---- sessions ----------------------------------------------------------

    pub fn insert_session(&self, session: &Session) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO sessions (id, project_id, engine_id, engine_session_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.id.to_string(),
                session.project_id.to_string(),
                session.engine_id,
                session.engine_session_id,
                time_text(session.created_at),
            ],
        )?;
        Ok(())
    }

    pub fn session(&self, id: SessionId) -> Result<Option<Session>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, project_id, engine_id, engine_session_id, created_at \
             FROM sessions WHERE id = ?1",
            params![id.to_string()],
            |row| Ok(session_from_row(row)),
        )
        .optional()?
        .transpose()
    }

    /// Newest first.
    pub fn sessions_for_project(&self, project_id: ProjectId) -> Result<Vec<Session>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, engine_id, engine_session_id, created_at FROM sessions \
             WHERE project_id = ?1 ORDER BY created_at DESC, rowid DESC",
        )?;
        collect(
            stmt.query(params![project_id.to_string()])?,
            session_from_row,
        )
    }

    /// The conversation to resume when this Project is opened again (C9).
    pub fn latest_session(&self, project_id: ProjectId) -> Result<Option<Session>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, project_id, engine_id, engine_session_id, created_at FROM sessions \
             WHERE project_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![project_id.to_string()],
            |row| Ok(session_from_row(row)),
        )
        .optional()?
        .transpose()
    }

    /// Records the engine's own session id, which is what `session/load` needs
    /// after a restart.
    pub fn set_engine_session_id(
        &self,
        id: SessionId,
        engine_session_id: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE sessions SET engine_session_id = ?2 WHERE id = ?1",
            params![id.to_string(), engine_session_id],
        )?;
        if changed == 0 {
            return Err(StoreError::not_found("Session", id));
        }
        Ok(())
    }

    // ---- turns -------------------------------------------------------------

    pub fn insert_turn(&self, turn: &Turn) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO turns \
             (id, session_id, request, phase, plan_json, pre_checkpoint, post_checkpoint, started_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                turn.id.to_string(),
                turn.session_id.to_string(),
                turn.request,
                enum_text(&turn.phase, "turn phase")?,
                plan_text(turn.plan.as_ref())?,
                turn.pre_checkpoint,
                turn.post_checkpoint,
                time_text(turn.started_at),
            ],
        )?;
        Ok(())
    }

    /// Writes the whole row back. The turn state machine holds one [`Turn`]
    /// and saves it at each phase change, so there is nothing to merge.
    pub fn update_turn(&self, turn: &Turn) -> Result<(), StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE turns SET session_id = ?2, request = ?3, phase = ?4, plan_json = ?5, \
             pre_checkpoint = ?6, post_checkpoint = ?7, started_at = ?8 WHERE id = ?1",
            params![
                turn.id.to_string(),
                turn.session_id.to_string(),
                turn.request,
                enum_text(&turn.phase, "turn phase")?,
                plan_text(turn.plan.as_ref())?,
                turn.pre_checkpoint,
                turn.post_checkpoint,
                time_text(turn.started_at),
            ],
        )?;
        if changed == 0 {
            return Err(StoreError::not_found("Turn", turn.id));
        }
        Ok(())
    }

    pub fn turn(&self, id: TurnId) -> Result<Option<Turn>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, session_id, request, phase, plan_json, pre_checkpoint, post_checkpoint, \
             started_at FROM turns WHERE id = ?1",
            params![id.to_string()],
            |row| Ok(turn_from_row(row)),
        )
        .optional()?
        .transpose()
    }

    /// Oldest first: this is a transcript, and a transcript reads forwards.
    pub fn turns_for_session(&self, session_id: SessionId) -> Result<Vec<Turn>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, request, phase, plan_json, pre_checkpoint, post_checkpoint, \
             started_at FROM turns WHERE session_id = ?1 ORDER BY started_at, rowid",
        )?;
        collect(stmt.query(params![session_id.to_string()])?, turn_from_row)
    }

    // ---- events ------------------------------------------------------------

    /// Writes an event and gives it its sequence number. Called for every
    /// event as it happens, which is what makes the transcript survive a
    /// restart (C9).
    pub fn append_event(
        &self,
        session_id: SessionId,
        event: &CoreEvent,
    ) -> Result<StoredEvent, StoreError> {
        let at = Utc::now();
        let turn_id = event.turn_id();
        let json = serde_json::to_string(event).map_err(|e| StoreError::corrupt("event", e))?;

        let conn = self.lock();
        conn.execute(
            "INSERT INTO events (session_id, turn_id, at, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                session_id.to_string(),
                turn_id.map(|id| id.to_string()),
                time_text(at),
                json,
            ],
        )?;
        let seq = conn.last_insert_rowid() as u64;
        Ok(StoredEvent {
            seq,
            session_id,
            turn_id,
            at,
            event: event.clone(),
        })
    }

    /// The transcript of one Session, oldest first. `after` is the last `seq`
    /// the caller already has, so a UI that saw a gap asks for what it missed.
    pub fn list_events(
        &self,
        session_id: SessionId,
        after: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT seq, session_id, turn_id, at, json FROM events \
             WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = stmt.query(params![
            session_id.to_string(),
            after.unwrap_or(0) as i64,
            limit.map_or(-1, |n| n as i64),
        ])?;
        collect(rows, stored_event_from_row)
    }

    // ---- checkpoints -------------------------------------------------------

    /// Caches a checkpoint the Journal has just made. Upsert, because the
    /// Journal is the truth and replaying it must converge rather than fail.
    pub fn upsert_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO checkpoints (id, project_id, turn_id, label, kind, created_at, files_changed) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(id) DO UPDATE SET project_id = excluded.project_id, \
             turn_id = excluded.turn_id, label = excluded.label, kind = excluded.kind, \
             created_at = excluded.created_at, files_changed = excluded.files_changed",
            params![
                checkpoint.id,
                checkpoint.project_id.to_string(),
                checkpoint.turn_id.map(|id| id.to_string()),
                checkpoint.label,
                enum_text(&checkpoint.kind, "checkpoint kind")?,
                time_text(checkpoint.created_at),
                checkpoint.files_changed as i64,
            ],
        )?;
        Ok(())
    }

    pub fn checkpoint(&self, id: &CheckpointId) -> Result<Option<Checkpoint>, StoreError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, project_id, turn_id, label, kind, created_at, files_changed \
             FROM checkpoints WHERE id = ?1",
            params![id],
            |row| Ok(checkpoint_from_row(row)),
        )
        .optional()?
        .transpose()
    }

    /// Newest first, the order the Checkpoints panel shows them in.
    pub fn checkpoints_for_project(
        &self,
        project_id: ProjectId,
        limit: Option<usize>,
    ) -> Result<Vec<Checkpoint>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, turn_id, label, kind, created_at, files_changed \
             FROM checkpoints WHERE project_id = ?1 \
             ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query(params![
            project_id.to_string(),
            limit.map_or(-1, |n| n as i64)
        ])?;
        collect(rows, checkpoint_from_row)
    }

    // ---- audit -------------------------------------------------------------

    /// Appends to the audit log. There is no counterpart: nothing in this
    /// crate updates or deletes an audit row, and the database refuses to
    /// anyway.
    pub fn append_audit(&self, entry: &NewAudit) -> Result<AuditEntry, StoreError> {
        let at = Utc::now();
        let detail_json = serde_json::to_string(&entry.detail)
            .map_err(|e| StoreError::corrupt("audit detail", e))?;
        let risk = entry
            .risk
            .map(|risk| enum_text(&risk, "risk class"))
            .transpose()?;

        let conn = self.lock();
        conn.execute(
            "INSERT INTO audit (at, project_id, turn_id, actor, action, risk, detail_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                time_text(at),
                entry.project_id.map(|id| id.to_string()),
                entry.turn_id.map(|id| id.to_string()),
                enum_text(&entry.actor, "actor")?,
                entry.action,
                risk,
                detail_json,
            ],
        )?;
        Ok(AuditEntry {
            seq: conn.last_insert_rowid() as u64,
            at,
            project_id: entry.project_id,
            turn_id: entry.turn_id,
            actor: entry.actor,
            action: entry.action.clone(),
            risk: entry.risk,
            detail: entry.detail.clone(),
        })
    }

    /// The audit log, newest first. `project_id` of `None` reads every
    /// Project's, including rows whose Project has since been removed.
    pub fn list_audit(
        &self,
        project_id: Option<ProjectId>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEntry>, StoreError> {
        let conn = self.lock();
        let limit = limit.map_or(-1, |n| n as i64);
        match project_id {
            Some(project_id) => {
                let mut stmt = conn.prepare(
                    "SELECT seq, at, project_id, turn_id, actor, action, risk, detail_json \
                     FROM audit WHERE project_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )?;
                collect(
                    stmt.query(params![project_id.to_string(), limit])?,
                    audit_from_row,
                )
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT seq, at, project_id, turn_id, actor, action, risk, detail_json \
                     FROM audit ORDER BY seq DESC LIMIT ?1",
                )?;
                collect(stmt.query(params![limit])?, audit_from_row)
            }
        }
    }

    // ---- settings ----------------------------------------------------------

    /// Reads a setting, or nothing if it has never been written.
    pub fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, StoreError> {
        let conn = self.lock();
        let json: Option<String> = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| {
            serde_json::from_str(&json)
                .map_err(|e| StoreError::corrupt(format!("setting {key}"), e))
        })
        .transpose()
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<(), StoreError> {
        let json = serde_json::to_string(value)
            .map_err(|e| StoreError::corrupt(format!("setting {key}"), e))?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO settings (key, value_json) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            params![key, json],
        )?;
        Ok(())
    }

    pub fn remove_setting(&self, key: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// Every setting key that has a value, sorted.
    pub fn setting_keys(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT key FROM settings ORDER BY key")?;
        let rows = stmt.query([])?;
        collect(rows, |row| Ok(row.get::<_, String>(0)?))
    }
}

// ---- migrations ------------------------------------------------------------

fn user_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? as u32)
}

/// Applies every migration the database has not seen, each in its own
/// transaction with the version bump inside it: an interrupted upgrade leaves
/// the database at the last version that fully applied, never between two.
fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let found = user_version(conn)?;
    let understood = schema_version();
    if found > understood {
        return Err(StoreError::SchemaTooNew { found, understood });
    }

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let version = index as u32 + 1;
        tracing::info!(migration = migration.name, version, "applying migration");
        conn.execute_batch(&format!(
            "BEGIN;\n{}\nPRAGMA user_version = {version};\nCOMMIT;",
            migration.sql
        ))?;
    }
    Ok(())
}

// ---- row mapping -----------------------------------------------------------

/// Reads every row with a mapper that may fail for reasons SQLite has no word
/// for — a uuid that will not parse, JSON that is not the shape it was. That
/// is why this exists instead of `query_map`, whose closure can only fail as
/// rusqlite.
fn collect<T>(
    mut rows: rusqlite::Rows<'_>,
    map: impl Fn(&Row<'_>) -> Result<T, StoreError>,
) -> Result<Vec<T>, StoreError> {
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(map(row)?);
    }
    Ok(out)
}

fn project_from_row(row: &Row<'_>) -> Result<Project, StoreError> {
    Ok(Project {
        id: uuid_at(row, 0, "Project id")?,
        name: row.get(1)?,
        root: PathBuf::from(row.get::<_, String>(2)?),
        created_at: time_at(row, 3, "Project created_at")?,
        engine_id: row.get(4)?,
    })
}

fn session_from_row(row: &Row<'_>) -> Result<Session, StoreError> {
    Ok(Session {
        id: uuid_at(row, 0, "Session id")?,
        project_id: uuid_at(row, 1, "Session project_id")?,
        engine_id: row.get(2)?,
        engine_session_id: row.get(3)?,
        created_at: time_at(row, 4, "Session created_at")?,
    })
}

fn turn_from_row(row: &Row<'_>) -> Result<Turn, StoreError> {
    let plan: Option<String> = row.get(4)?;
    Ok(Turn {
        id: uuid_at(row, 0, "Turn id")?,
        session_id: uuid_at(row, 1, "Turn session_id")?,
        request: row.get(2)?,
        phase: enum_at::<TurnPhase>(row, 3, "turn phase")?,
        plan: plan
            .map(|json| {
                serde_json::from_str::<Plan>(&json).map_err(|e| StoreError::corrupt("plan", e))
            })
            .transpose()?,
        pre_checkpoint: row.get(5)?,
        post_checkpoint: row.get(6)?,
        started_at: time_at(row, 7, "Turn started_at")?,
    })
}

fn stored_event_from_row(row: &Row<'_>) -> Result<StoredEvent, StoreError> {
    let json: String = row.get(4)?;
    Ok(StoredEvent {
        seq: row.get::<_, i64>(0)? as u64,
        session_id: uuid_at(row, 1, "event session_id")?,
        turn_id: optional_uuid_at(row, 2, "event turn_id")?,
        at: time_at(row, 3, "event time")?,
        event: serde_json::from_str(&json).map_err(|e| StoreError::corrupt("event", e))?,
    })
}

fn checkpoint_from_row(row: &Row<'_>) -> Result<Checkpoint, StoreError> {
    Ok(Checkpoint {
        id: row.get(0)?,
        project_id: uuid_at(row, 1, "checkpoint project_id")?,
        turn_id: optional_uuid_at(row, 2, "checkpoint turn_id")?,
        label: row.get(3)?,
        kind: enum_at::<CheckpointKind>(row, 4, "checkpoint kind")?,
        created_at: time_at(row, 5, "checkpoint created_at")?,
        files_changed: row.get::<_, i64>(6)?.max(0) as usize,
    })
}

fn audit_from_row(row: &Row<'_>) -> Result<AuditEntry, StoreError> {
    let risk: Option<String> = row.get(6)?;
    let detail: String = row.get(7)?;
    Ok(AuditEntry {
        seq: row.get::<_, i64>(0)? as u64,
        at: time_at(row, 1, "audit time")?,
        project_id: optional_uuid_at(row, 2, "audit project_id")?,
        turn_id: optional_uuid_at(row, 3, "audit turn_id")?,
        actor: enum_at::<DecidedBy>(row, 4, "actor")?,
        action: row.get(5)?,
        risk: risk
            .map(|risk| parse_enum::<RiskClass>(&risk, "risk class"))
            .transpose()?,
        detail: serde_json::from_str(&detail)
            .map_err(|e| StoreError::corrupt("audit detail", e))?,
    })
}

// ---- column helpers --------------------------------------------------------

fn uuid_at(row: &Row<'_>, index: usize, what: &str) -> Result<uuid::Uuid, StoreError> {
    let text: String = row.get(index)?;
    uuid::Uuid::parse_str(&text).map_err(|e| StoreError::corrupt(what.to_string(), e))
}

fn optional_uuid_at(
    row: &Row<'_>,
    index: usize,
    what: &str,
) -> Result<Option<uuid::Uuid>, StoreError> {
    let text: Option<String> = row.get(index)?;
    text.map(|text| {
        uuid::Uuid::parse_str(&text).map_err(|e| StoreError::corrupt(what.to_string(), e))
    })
    .transpose()
}

fn time_at(row: &Row<'_>, index: usize, what: &str) -> Result<DateTime<Utc>, StoreError> {
    let text: String = row.get(index)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|at| at.with_timezone(&Utc))
        .map_err(|e| StoreError::corrupt(what.to_string(), e))
}

fn enum_at<T: DeserializeOwned>(row: &Row<'_>, index: usize, what: &str) -> Result<T, StoreError> {
    let text: String = row.get(index)?;
    parse_enum(&text, what)
}

/// Fixed width and always UTC, so `ORDER BY` on the column is chronological
/// and a round trip changes nothing.
fn time_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// A path as text. Storing it any other way would mean the database could hold
/// a root that no `Project` can be built from.
fn path_text(path: &Path) -> Result<String, StoreError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::NonUtf8Path(path.to_path_buf()))
}

/// The spelling of an enum in the database is its serde spelling, so the
/// column and the wire never drift apart.
fn enum_text<T: Serialize>(value: &T, what: &str) -> Result<String, StoreError> {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => Ok(text),
        Ok(other) => Err(StoreError::Corrupt {
            what: what.to_string(),
            source: format!("expected a string, got {other}").into(),
        }),
        Err(e) => Err(StoreError::corrupt(what.to_string(), e)),
    }
}

fn parse_enum<T: DeserializeOwned>(text: &str, what: &str) -> Result<T, StoreError> {
    serde_json::from_value(serde_json::Value::String(text.to_owned()))
        .map_err(|e| StoreError::corrupt(what.to_string(), e))
}

fn plan_text(plan: Option<&Plan>) -> Result<Option<String>, StoreError> {
    plan.map(|plan| serde_json::to_string(plan).map_err(|e| StoreError::corrupt("plan", e)))
        .transpose()
}
