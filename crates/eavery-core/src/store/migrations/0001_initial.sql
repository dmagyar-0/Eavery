-- The tables from `docs/plan/03-architecture.md` §8.
--
-- Every table is STRICT: a column typed TEXT refuses an integer rather than
-- storing one and handing it back later as something the Rust side cannot
-- parse. Timestamps are RFC 3339 in UTC with nine fractional digits, always
-- the same width, so ORDER BY on them is chronological.

CREATE TABLE projects (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    root       TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    engine_id  TEXT
) STRICT;

CREATE TABLE sessions (
    id                TEXT PRIMARY KEY NOT NULL,
    project_id        TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    engine_id         TEXT NOT NULL,
    -- The ACP sessionId, for `session/load` after a restart (C9).
    engine_session_id TEXT,
    created_at        TEXT NOT NULL
) STRICT;

CREATE INDEX sessions_by_project ON sessions(project_id, created_at);

CREATE TABLE turns (
    id              TEXT PRIMARY KEY NOT NULL,
    session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    request         TEXT NOT NULL,
    phase           TEXT NOT NULL,
    plan_json       TEXT,
    pre_checkpoint  TEXT,
    post_checkpoint TEXT,
    started_at      TEXT NOT NULL
) STRICT;

CREATE INDEX turns_by_session ON turns(session_id, started_at);

-- `seq` is global and is what `core://event` carries, so a UI that misses an
-- event sees a gap in its own numbering and can re-fetch (§7).
CREATE TABLE events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id    TEXT,
    at         TEXT NOT NULL,
    json       TEXT NOT NULL
) STRICT;

CREATE INDEX events_by_session ON events(session_id, seq);

-- A cache for the UI. The Journal's commits are the truth; a row here that
-- git does not have is wrong, never the other way round
-- (`docs/plan/05-git-journal.md` §3).
CREATE TABLE checkpoints (
    id            TEXT PRIMARY KEY NOT NULL,
    project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    turn_id       TEXT,
    label         TEXT NOT NULL,
    kind          TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    files_changed INTEGER NOT NULL
) STRICT;

CREATE INDEX checkpoints_by_project ON checkpoints(project_id, created_at);

-- Append-only, and not by convention: the triggers below make an UPDATE or a
-- DELETE fail. `project_id` therefore carries no foreign key — a cascade from
-- `projects` would be a DELETE — so removing a Project leaves its decisions on
-- the record.
CREATE TABLE audit (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    at          TEXT NOT NULL,
    project_id  TEXT,
    turn_id     TEXT,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    risk        TEXT,
    detail_json TEXT NOT NULL
) STRICT;

CREATE INDEX audit_by_project ON audit(project_id, seq);

CREATE TRIGGER audit_is_append_only_update BEFORE UPDATE ON audit BEGIN
    SELECT RAISE(ABORT, 'the audit log is append-only');
END;

CREATE TRIGGER audit_is_append_only_delete BEFORE DELETE ON audit BEGIN
    SELECT RAISE(ABORT, 'the audit log is append-only');
END;

CREATE TABLE settings (
    key        TEXT PRIMARY KEY NOT NULL,
    value_json TEXT NOT NULL
) STRICT;
