//! SQLite persistence.
//!
//! One writer connection guarded by a mutex plus a small pool of read-only
//! connections, WAL mode, synchronous=NORMAL. WAL lets the readers run while the
//! writer commits, so a long `list_messages` scan no longer queues every write
//! behind it. All queries run on the blocking pool so the async runtime is never
//! blocked.
//!
//! Three invariants are enforced at [`SqliteStore::open`] instead of being
//! assumed:
//!
//! * **one process per data directory** — an exclusive `flock` on
//!   `<data_dir>/karucode.lock` is held for the lifetime of the store, so two
//!   servers can never migrate the same file or mark each other's live runs as
//!   interrupted;
//! * **the pragmas actually took effect** — `journal_mode=WAL` and
//!   `foreign_keys=ON` are read back and a failure is fatal, because a silently
//!   disabled `foreign_keys` turns `ON DELETE CASCADE` into orphaned rows;
//! * **the file is readable** — `PRAGMA quick_check` runs before any migration,
//!   so corruption is reported with a recovery command instead of a raw
//!   rusqlite string.

use async_trait::async_trait;
use kc_core::checkpoint::FileCheckpoint;
use kc_core::config::SessionModelSelection;
use kc_core::error::{Error, Result};
use kc_core::message::{ImageAttachment, ToolCall};
use kc_core::store::*;
use parking_lot::Mutex;
use rusqlite::{params, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Current schema version. Bump together with a new arm in [`SqliteStore::migrate`].
const SCHEMA_VERSION: i64 = 21;

/// Advisory lock file that makes a data directory single-instance.
const LOCK_FILE_NAME: &str = "karucode.lock";

/// Read-only connections used by the `list_*` / `get_*` paths. Two is enough to
/// keep a slow scan from starving the UI's polling reads without multiplying the
/// per-connection page cache.
const READ_POOL_SIZE: usize = 2;

/// A retention pass always keeps this many newest messages per session, so a
/// session that has been idle for longer than the retention window still has the
/// context its next run needs.
const RETENTION_KEEP_MESSAGES: usize = 100;

/// Schema v1. Every statement is `IF NOT EXISTS` so a partially applied
/// migration can be re-run safely; the whole batch runs in one transaction.
const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL DEFAULT '',
    workspace  TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    run_id       TEXT,
    agent_id     TEXT,
    role         TEXT NOT NULL,
    content      TEXT NOT NULL DEFAULT '',
    tool_calls   TEXT,
    tool_call_id TEXT,
    name         TEXT,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id, id);

CREATE TABLE IF NOT EXISTS runs (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    status      TEXT NOT NULL,
    model       TEXT NOT NULL DEFAULT '',
    prompt      TEXT NOT NULL DEFAULT '',
    error       TEXT,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_runs_session_started ON runs(session_id, started_at);

CREATE TABLE IF NOT EXISTS agents (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_run_id   TEXT NOT NULL DEFAULT '',
    parent_agent_id TEXT,
    name            TEXT NOT NULL DEFAULT '',
    role            TEXT NOT NULL DEFAULT '',
    task            TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL,
    model           TEXT NOT NULL DEFAULT '',
    iterations      INTEGER NOT NULL DEFAULT 0,
    summary         TEXT NOT NULL DEFAULT '',
    error           TEXT,
    owned_paths     TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    finished_at     INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agents_session_id ON agents(session_id);

CREATE TABLE IF NOT EXISTS approvals (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    run_id      TEXT NOT NULL,
    agent_id    TEXT,
    kind        TEXT NOT NULL,
    title       TEXT NOT NULL DEFAULT '',
    detail      TEXT NOT NULL DEFAULT '',
    payload     TEXT,
    status      TEXT NOT NULL,
    decision    TEXT,
    created_at  INTEGER NOT NULL,
    resolved_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals(status);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS claims (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    agent_id   TEXT NOT NULL,
    path       TEXT NOT NULL,
    mode       TEXT NOT NULL DEFAULT 'write',
    created_at INTEGER NOT NULL,
    UNIQUE(agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_claims_session_id ON claims(session_id);
"#;

/// Schema v2: projects. A project is a directory the agent may work in; every
/// session belongs to one (nullable for sessions created before v2, which the
/// server attaches at startup). The `workspace` column on sessions stays: it is
/// the path the session actually ran in, even if the project is later renamed.
///
/// The `ALTER TABLE sessions ADD COLUMN project_id` is not in the batch: SQLite
/// has no `ADD COLUMN IF NOT EXISTS`, so it goes through
/// [`add_column_if_missing`] like every other added column, and the batch here
/// runs *after* it so `idx_sessions_project_id` has a column to index. See
/// [`SqliteStore::migrate`] for why that matters.
const MIGRATION_2: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    path       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_path ON projects(path);
CREATE INDEX IF NOT EXISTS idx_sessions_project_id ON sessions(project_id);
"#;

/// Schema v3: per-session model selection. NULL follows the install-wide
/// default; otherwise a JSON [`SessionModelSelection`] (`{provider, model,
/// reasoning_effort?}`) pins this session to a profile + model.
///
/// The column itself is in [`ADDED_COLUMNS`], so this batch is empty - SQLite
/// has no `ADD COLUMN IF NOT EXISTS` and the step has to be replayable. See
/// [`SqliteStore::migrate`].
const MIGRATION_3: &str = "";

/// Schema v4: multi-instance safety and the indexes the hot queries need.
///
/// `instance_id` records which process created a run/agent. Crash recovery
/// (`mark_interrupted_runs` / `mark_interrupted_agents`) is scoped to the
/// current instance, so a second server can never mark another server's live
/// rows as interrupted. Rows written before v4 keep the empty string and are
/// therefore never touched by recovery — they are already terminal.
///
/// The indexes cover the four queries that were scanning:
/// `messages(agent_id, id)` for `list_agent_messages`,
/// `approvals(run_id)` for `expire_pending_approvals`,
/// `approvals(session_id, status)` for `list_pending_approvals(Some(_))` and
/// `sessions(updated_at)` for the `ORDER BY s.updated_at DESC` in
/// `list_sessions`.
const MIGRATION_4: &str = r#"
CREATE INDEX IF NOT EXISTS idx_messages_agent_id ON messages(agent_id, id);
CREATE INDEX IF NOT EXISTS idx_approvals_run_id ON approvals(run_id);
CREATE INDEX IF NOT EXISTS idx_approvals_session_status ON approvals(session_id, status);
CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at);
"#;

/// Schema v5: images carried by a message.
///
/// A `tool` message cannot hold pixels on the OpenAI-compatible wire format, so
/// `read_file` on an image stores the base64 payload beside the text and
/// `kc-provider` re-emits it as a follow-up `user` message with an `image_url`
/// content part. `'[]'` for rows written before v5 keeps every existing
/// transcript readable.
const MIGRATION_5: &str = "";

/// Schema v6: the browser terminal.
///
/// A row is a *named shell*: its title, the directory it was started in and its
/// place in the tab strip. It is not the process - a PTY is a child of the
/// server and cannot outlive it - but keeping the row means the tab strip, the
/// titles and the working directory are identical in every browser and after
/// every restart, which is what makes the terminal tab feel global rather than
/// per-page. `position` is a manual sort key; ties break on `created_at` so an
/// install that never reorders still lists oldest-first.
const MIGRATION_6: &str = r#"
CREATE TABLE IF NOT EXISTS terminals (
    id           TEXT PRIMARY KEY,
    title        TEXT NOT NULL DEFAULT '',
    cwd          TEXT NOT NULL DEFAULT '',
    project_id   TEXT,
    position     INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_terminals_position ON terminals(position, created_at);
"#;

/// Schema v7: rewind.
///
/// `messages.rewound_at` is the mark a rewind leaves. Rows are never deleted -
/// `repair_history`, retention and every client read the transcript as a
/// monotonic log, and deleting from the middle of it would renumber the tail a
/// second browser is currently rendering. `NULL` means "visible", which is the
/// value every existing row already has, so an upgraded database behaves exactly
/// as it did before.
///
/// `checkpoints` is the file journal: one row per mutating file operation,
/// holding the bytes the file had *before* the tool ran. `content_id` points at
/// the deduplicated payload in `checkpoint_blobs`, because a run that edits one
/// file five times stores four identical before-images and a large session can
/// hold hundreds of megabytes of them.
///
/// `(session_id, message_id)` is the access path every rewind uses: "everything
/// that happened after this message". `checkpoint_blobs` is swept by both
/// retention and the rewind itself, since a deleted journal row must not leave
/// its payload behind.
const MIGRATION_7: &str = r#"
CREATE TABLE IF NOT EXISTS checkpoint_blobs (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    sha  TEXT NOT NULL,
    data BLOB NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_checkpoint_blobs_sha ON checkpoint_blobs(sha);

CREATE TABLE IF NOT EXISTS checkpoints (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    run_id       TEXT NOT NULL DEFAULT '',
    agent_id     TEXT,
    call_id      TEXT NOT NULL DEFAULT '',
    tool         TEXT NOT NULL DEFAULT '',
    path         TEXT NOT NULL,
    existed      INTEGER NOT NULL DEFAULT 0,
    content_id   INTEGER REFERENCES checkpoint_blobs(id),
    capture_failed INTEGER NOT NULL DEFAULT 0,
    message_id   INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_checkpoints_region ON checkpoints(session_id, message_id);
"#;

/// Schema v8: per-call token accounting.
///
/// One row per provider call, written when the runtime receives the endpoint's
/// `usage` block. The `usage` *event* is live-only, so before this table the
/// answer to "what has this session cost" existed only in the browser tab that
/// happened to be open. `agent_id` is `NULL` for a run's own model calls and set
/// for a subagent's, which is what lets the session information view show the
/// main agent and each subagent separately without a second table.
///
/// No foreign keys beyond the session. A usage row is a *measurement*, not a
/// parent of anything: cascading from `runs` or `agents` would silently rewrite
/// history when retention prunes an old run, and the numbers stay meaningful
/// after the transcript rows they describe are gone.
const MIGRATION_8: &str = r#"
CREATE TABLE IF NOT EXISTS usage_calls (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    run_id        TEXT NOT NULL DEFAULT '',
    agent_id      TEXT,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER,
    created_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_usage_calls_session ON usage_calls(session_id, id);
"#;

/// Schema v9: reporting on a live subagent.
///
/// A background agent is a job the parent can watch, message and stop, so the
/// row carries what a human would ask about it: whether it was detached, what it
/// last said it was doing, and how far along it claims to be. `note` and
/// `progress` are written by `report_status`, which is advisory - they describe
/// the agent's own claim, not a measurement - and `background = 0` keeps every
/// existing row meaning "a spawn that blocked its caller", which is exactly what
/// it was.
const MIGRATION_9: &str = "";

/// Schema v10: the index the retention pass needs.
///
/// Pruning runs and agents asks "does any message still reference this row?",
/// which without an index on `messages(run_id)` is a correlated full scan of
/// `messages` per candidate row - 1.2 s for 149 runs over 33k messages, all of
/// it inside the transaction that holds the single writer. With the index the
/// subquery becomes a covering-index lookup per row.
const MIGRATION_10: &str = r#"
CREATE INDEX IF NOT EXISTS idx_messages_run_id ON messages(run_id);
"#;

/// Schema v11: preserve model reasoning across turns.
///
/// Some OpenAI-compatible endpoints stream analysis separately as
/// `reasoning_content`. Keeping that text beside the assistant answer lets later
/// requests send the complete turn back to the model. The column is nullable;
/// rows written before v11 and messages from endpoints that do not emit
/// reasoning read back as `None`.
const MIGRATION_11: &str = "";

const MIGRATION_19: &str = r#"
CREATE TABLE IF NOT EXISTS control_chat (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), session_id TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS control_dispatches (request_id TEXT PRIMARY KEY, session_id TEXT NOT NULL UNIQUE, project_id TEXT NOT NULL, title TEXT NOT NULL, workspace TEXT NOT NULL, prompt TEXT NOT NULL);
"#;

/// Schema v20: control sessions are a set, not a singleton.
///
/// The marker moves onto `sessions` (the column is in [`ADDED_COLUMNS`]) so any
/// number of control sessions can exist; the old singleton row is carried over
/// and the table dropped, because its `CHECK(singleton = 1)` is exactly what made
/// a second control chat impossible.
const MIGRATION_20: &str = r#"
UPDATE sessions SET control = 1 WHERE id IN (SELECT session_id FROM control_chat);
DROP TABLE IF EXISTS control_chat;
CREATE INDEX IF NOT EXISTS idx_sessions_control ON sessions(control);
"#;

/// Schema v21: orphan-blob sweeps look up checkpoints by their shared blob.
/// Without this index each spill sweep correlated a full checkpoint scan.
const MIGRATION_21: &str = r#"
CREATE INDEX IF NOT EXISTS idx_checkpoints_content_id ON checkpoints(content_id);
"#;

const MIGRATION_12: &str = r#"
CREATE TABLE IF NOT EXISTS missions (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
 title TEXT NOT NULL, description TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_missions_project ON missions(project_id);
CREATE TABLE IF NOT EXISTS mission_sessions (
 mission_id TEXT NOT NULL REFERENCES missions(id) ON DELETE CASCADE,
 request_id TEXT NOT NULL, session_id TEXT NOT NULL UNIQUE,
 title TEXT NOT NULL, prompt TEXT NOT NULL, created_at INTEGER NOT NULL,
 PRIMARY KEY (mission_id, request_id)
);
"#;

// Legacy mission sessions always belonged to the mission's home project. Keep
// this immutable request identity even if the session or target project is deleted.
const MIGRATION_13: &str = r#"
UPDATE mission_sessions SET project_id = (SELECT project_id FROM missions WHERE id = mission_id)
WHERE project_id IS NULL;
"#;

const MIGRATION_14: &str = r#"
CREATE TABLE IF NOT EXISTS mission_schedules (mission_id TEXT PRIMARY KEY REFERENCES missions(id) ON DELETE CASCADE, concurrency INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS mission_tasks (
 id TEXT PRIMARY KEY, mission_id TEXT NOT NULL REFERENCES missions(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL, title TEXT NOT NULL, prompt TEXT NOT NULL, dependencies TEXT NOT NULL,
 state TEXT NOT NULL, session_id TEXT, run_id TEXT, error TEXT, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_mission_tasks_mission ON mission_tasks(mission_id);
"#;

// Records contain only explicitly published text, never a file snapshot or a
// claim that verification was independently performed.
const MIGRATION_15: &str = r#"
CREATE TABLE IF NOT EXISTS mission_records (
 id TEXT PRIMARY KEY, mission_id TEXT NOT NULL REFERENCES missions(id) ON DELETE CASCADE,
 request_id TEXT NOT NULL, kind TEXT NOT NULL, title TEXT NOT NULL, content TEXT NOT NULL,
 task_id TEXT, session_id TEXT, artifact_id TEXT, created_at INTEGER NOT NULL,
 UNIQUE(mission_id, request_id)
);
CREATE INDEX IF NOT EXISTS idx_mission_records_mission ON mission_records(mission_id, kind);
"#;

// Durable request ledger and history of replaced attempts.
const MIGRATION_16: &str = r#"
CREATE TABLE IF NOT EXISTS mission_task_retries (
 mission_id TEXT NOT NULL REFERENCES missions(id) ON DELETE CASCADE,
 request_id TEXT NOT NULL, task_id TEXT NOT NULL, project_id TEXT,
 previous_project_id TEXT NOT NULL, previous_state TEXT NOT NULL,
 previous_session_id TEXT, previous_run_id TEXT, previous_error TEXT,
 PRIMARY KEY(mission_id, request_id)
);
"#;

// Independent of missions and sessions so deleting either never resets a budget.
const MIGRATION_18: &str = r#"
CREATE TABLE IF NOT EXISTS mission_attempt_budgets (
 mission_id TEXT PRIMARY KEY, max_attempts INTEGER, attempts INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS mission_attempt_sessions (
 session_id TEXT PRIMARY KEY, mission_id TEXT NOT NULL
);
INSERT OR IGNORE INTO mission_attempt_budgets (mission_id) SELECT id FROM missions;
INSERT OR IGNORE INTO mission_attempt_sessions SELECT session_id, mission_id FROM mission_sessions;
INSERT OR IGNORE INTO mission_attempt_sessions SELECT session_id, mission_id FROM mission_tasks WHERE session_id IS NOT NULL;
INSERT OR IGNORE INTO mission_attempt_sessions SELECT previous_session_id, mission_id FROM mission_task_retries WHERE previous_session_id IS NOT NULL;
CREATE TRIGGER IF NOT EXISTS mission_attempt_session_insert AFTER INSERT ON mission_sessions
BEGIN
 SELECT CASE WHEN EXISTS(SELECT 1 FROM mission_attempt_sessions WHERE session_id=NEW.session_id AND mission_id != NEW.mission_id) THEN RAISE(ABORT, 'session belongs to another mission') END;
 INSERT OR IGNORE INTO mission_attempt_sessions (session_id, mission_id) VALUES (NEW.session_id, NEW.mission_id);
END;
"#;

/// Every `ADD COLUMN` in the schema, as `(version, table, column, declaration)`.
///
/// SQLite has no `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` (it is a syntax
/// error), so a replayed migration batch that contains one aborts with
/// "duplicate column name". That is not a corner case: `sqlite3 db .dump` does
/// not carry `user_version`, so the ordinary backup idiom - dump, then restore
/// into a fresh file - produces a database whose *schema is current* and whose
/// version reads 0. Migrations then replay from v1 and used to die on the first
/// `ADD COLUMN`, leaving a file that could never be opened again.
///
/// So every step's columns live here and go through
/// [`add_column_if_missing`], which reads `PRAGMA table_info` first. `table` and
/// `column` are compile-time literals, never caller input.
const ADDED_COLUMNS: &[(i64, &str, &str, &str)] = &[
    (2, "sessions", "project_id", "TEXT"),
    (3, "sessions", "model", "TEXT"),
    (4, "runs", "instance_id", "TEXT NOT NULL DEFAULT ''"),
    (4, "agents", "instance_id", "TEXT NOT NULL DEFAULT ''"),
    (5, "messages", "images", "TEXT NOT NULL DEFAULT '[]'"),
    (7, "messages", "rewound_at", "INTEGER"),
    (9, "agents", "background", "INTEGER NOT NULL DEFAULT 0"),
    (9, "agents", "note", "TEXT NOT NULL DEFAULT ''"),
    (9, "agents", "progress", "REAL"),
    (11, "messages", "reasoning_content", "TEXT"),
    (13, "mission_sessions", "project_id", "TEXT"),
    (
        17,
        "mission_tasks",
        "artifact_ids",
        "TEXT NOT NULL DEFAULT '[]'",
    ),
    (
        15,
        "mission_schedules",
        "state",
        "TEXT NOT NULL DEFAULT 'active'",
    ),
    (20, "sessions", "control", "INTEGER NOT NULL DEFAULT 0"),
];

pub struct SqliteStore {
    /// The single writer connection. Every mutation takes this mutex.
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// Read-only connections for the `list_*` / `get_*` paths. WAL lets them
    /// read a consistent snapshot while the writer commits, so a long scan no
    /// longer blocks every write. Empty when the pool could not be opened (the
    /// reads then fall back to the writer connection).
    readers: Arc<Vec<Mutex<rusqlite::Connection>>>,
    /// Round-robin cursor into `readers`.
    next_reader: AtomicUsize,
    path: PathBuf,
    /// Identity of this process in `runs.instance_id` / `agents.instance_id`.
    instance_id: String,
    /// Held for the lifetime of the store: dropping it releases the advisory
    /// lock that keeps a data directory single-instance.
    _lock: Option<std::fs::File>,
    /// Unix ms of the last successful integrity check, for the health endpoint.
    last_integrity_check: AtomicI64,
}

/// Force `path` to `mode` on Unix. The settings table can hold an API key, so
/// the database file and its directory must not be group/world readable.
fn restrict_permissions(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| Error::io(path, e))?
            .permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms).map_err(|e| Error::io(path, e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Unix ms before which a row counts as "older than `days`".
fn retention_cutoff(days: u64) -> i64 {
    let ms = days.saturating_mul(24 * 60 * 60 * 1000);
    now_ms().saturating_sub(i64::try_from(ms).unwrap_or(i64::MAX))
}

/// Size of a file in bytes, or 0 when it does not exist yet.
fn file_size(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(meta.len()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(Error::io(path, e)),
    }
}

fn db_err(context: &str, e: rusqlite::Error) -> Error {
    Error::Store(format!("{context}: {e}"))
}

/// `ALTER TABLE <table> ADD COLUMN <column> <decl>`, but only when the column
/// is not there yet. Returns whether the table was changed.
///
/// This is the substitute for the `ADD COLUMN IF NOT EXISTS` SQLite does not
/// have, and it is what makes [`SqliteStore::migrate`] replayable: a restored
/// `.dump` has the current schema with `user_version = 0`, so every step runs
/// again over tables that already carry every column.
///
/// `PRAGMA table_info` cannot be parameterised, so the table name is
/// interpolated. The only strings that reach it are the literals in
/// [`ADDED_COLUMNS`] and [`SCHEMA_TABLES`], which a test checks against SQLite's
/// own table list - never user input.
fn add_column_if_missing(
    conn: &rusqlite::Connection,
    table: &'static str,
    column: &'static str,
    decl: &'static str,
) -> Result<bool> {
    let existing: Vec<String> = {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| db_err("table_info prepare", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>("name"))
            .map_err(|e| db_err("table_info query", e))?;
        // The statement is dropped before the ALTER so nothing is holding a
        // schema snapshot open while the table is rewritten.
        rows.collect::<rusqlite::Result<Vec<String>>>()
            .map_err(|e| db_err("table_info rows", e))?
    };
    if existing.iter().any(|name| name == column) {
        return Ok(false);
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))
        .map_err(|e| db_err(&format!("add {table}.{column}"), e))?;
    Ok(true)
}

/// Every table the schema creates, in the order [`SqliteStore::migrate`]'s
/// batches run. Only used by the test that keeps [`add_column_if_missing`]'s
/// interpolated table names honest (a typo would otherwise look fine, and
/// silently skip a column on a fresh database).
#[cfg(test)]
const SCHEMA_TABLES: &[&str] = &[
    "sessions",
    "messages",
    "runs",
    "agents",
    "approvals",
    "settings",
    "claims",
    "projects",
    "terminals",
    "checkpoint_blobs",
    "checkpoints",
    "usage_calls",
];

fn limit_i64(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

/// A SQLite `INTEGER` token count back into the `u32` the wire type uses.
///
/// Saturating rather than panicking: a provider that reports a count above
/// `u32::MAX` is telling us the call was enormous, and refusing to show its
/// cost would be worse than capping the number.
fn u32_of(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

/// A SQLite `INTEGER` sum back into a `u64`. `SUM` over the table can exceed
/// `i64` only after ~9·10¹⁸ tokens, so saturating is honest, not a limit.
fn u64_of(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}

/// Lowercase hex SHA-256, used as the content key of the rewind journal.
fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Decode a JSON array column, falling back to the default on NULL/garbage.
fn json_vec<T: serde::de::DeserializeOwned + Default>(raw: Option<String>, column: &str) -> Vec<T> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(column, error = %e, "malformed json column; using default");
                Vec::new()
            }
        },
        _ => Vec::new(),
    }
}

fn json_value(raw: Option<String>, column: &str) -> serde_json::Value {
    match raw {
        Some(raw) if !raw.trim().is_empty() => match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(column, error = %e, "malformed json column; using null");
                serde_json::Value::Null
            }
        },
        _ => serde_json::Value::Null,
    }
}

/// Decode a nullable JSON object column, falling back to `None` on NULL/garbage.
/// A corrupt per-session selection degrades to the install-wide default instead
/// of failing the whole row.
fn json_opt<T: serde::de::DeserializeOwned>(raw: Option<String>, column: &str) -> Option<T> {
    match raw {
        Some(raw) if !raw.trim().is_empty() => match serde_json::from_str(&raw) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(column, error = %e, "malformed json column; using default");
                None
            }
        },
        _ => None,
    }
}

/// Delete every `checkpoint_blobs` row no `checkpoints` row points at, and
/// return how many went.
///
/// The journal is content-addressed, so one blob can back many checkpoint rows
/// in many sessions, and the only link runs `checkpoints.content_id ->
/// checkpoint_blobs.id`. `NOT EXISTS` therefore is the correct shape: asking
/// "does anything still need these bytes" is safe, deleting by `session_id`
/// would take a payload another session is still using.
///
/// Only meaningful *after* the caller has deleted the journal rows it no longer
/// wants, which is why it is called from both `delete_session` and the
/// retention pass rather than offered as a standalone store method.
fn sweep_orphaned_blobs(conn: &rusqlite::Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM checkpoint_blobs
          WHERE NOT EXISTS (
                SELECT 1 FROM checkpoints c WHERE c.content_id = checkpoint_blobs.id
          )",
        [],
    )
}

fn map_session(row: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        title: row.get("title")?,
        workspace: row.get("workspace")?,
        project_id: row.get("project_id")?,
        model: json_opt(row.get("model")?, "sessions.model"),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn map_project(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        path: row.get("path")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        session_count: row.get("session_count")?,
    })
}

fn map_terminal(row: &Row<'_>) -> rusqlite::Result<TerminalSession> {
    Ok(TerminalSession {
        id: row.get("id")?,
        title: row.get("title")?,
        cwd: row.get("cwd")?,
        project_id: row.get("project_id")?,
        position: row.get("position")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_used_at: row.get("last_used_at")?,
    })
}

fn map_session_summary(row: &Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: row.get("id")?,
        title: row.get("title")?,
        workspace: row.get("workspace")?,
        project_id: row.get("project_id")?,
        model: json_opt(row.get("model")?, "sessions.model"),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        message_count: row.get("message_count")?,
        last_run_status: row.get("last_run_status")?,
        running: row.get::<_, i64>("running")? != 0,
    })
}

/// What a session listing covers.
#[derive(Clone, Copy)]
enum SessionScope<'a> {
    All,
    Project(&'a str),
    /// Control-plane chats: their own set, never part of a project's list.
    Control,
}

fn summarize_sessions(
    conn: &rusqlite::Connection,
    scope: SessionScope<'_>,
) -> Result<Vec<SessionSummary>> {
    let base =
        "SELECT s.id, s.title, s.workspace, s.project_id, s.model, s.created_at, s.updated_at,
                       (SELECT COUNT(*) FROM messages m
                         WHERE m.session_id = s.id AND m.rewound_at IS NULL) AS message_count,
                       (SELECT r.status FROM runs r
                         WHERE r.session_id = s.id
                         ORDER BY r.started_at DESC, r.id DESC LIMIT 1) AS last_run_status,
                       EXISTS(SELECT 1 FROM runs r
                               WHERE r.session_id = s.id AND r.status = 'running') AS running
                  FROM sessions s";
    let sql = match scope {
        SessionScope::Project(_) => {
            format!("{base} WHERE s.project_id = ?1 ORDER BY s.updated_at DESC, s.id DESC")
        }
        SessionScope::All => format!("{base} ORDER BY s.updated_at DESC, s.id DESC"),
        SessionScope::Control => {
            format!("{base} WHERE s.control = 1 ORDER BY s.updated_at DESC, s.id DESC")
        }
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| db_err("list_sessions prepare", e))?;
    let rows = match scope {
        SessionScope::Project(id) => stmt.query_map(params![id], map_session_summary),
        SessionScope::All | SessionScope::Control => stmt.query_map([], map_session_summary),
    }
    .map_err(|e| db_err("list_sessions query", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| db_err("list_sessions rows", e))
}

fn map_stored_message(row: &Row<'_>) -> rusqlite::Result<StoredMessage> {
    let tool_calls = json_vec::<ToolCall>(row.get("tool_calls")?, "messages.tool_calls");
    let images = json_vec::<ImageAttachment>(row.get("images")?, "messages.images");
    Ok(StoredMessage {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        run_id: row.get("run_id")?,
        agent_id: row.get("agent_id")?,
        role: row.get("role")?,
        content: row.get("content")?,
        tool_calls,
        images,
        tool_call_id: row.get("tool_call_id")?,
        name: row.get("name")?,
        reasoning_content: row.get("reasoning_content")?,
        rewound_at: row.get("rewound_at")?,
        created_at: row.get("created_at")?,
    })
}

fn map_run(row: &Row<'_>) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        status: row.get("status")?,
        model: row.get("model")?,
        prompt: row.get("prompt")?,
        error: row.get("error")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
    })
}

fn map_agent(row: &Row<'_>) -> rusqlite::Result<AgentRecord> {
    let owned_paths = json_vec::<String>(row.get("owned_paths")?, "agents.owned_paths");
    Ok(AgentRecord {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        parent_run_id: row.get("parent_run_id")?,
        parent_agent_id: row.get("parent_agent_id")?,
        name: row.get("name")?,
        role: row.get("role")?,
        task: row.get("task")?,
        status: row.get("status")?,
        model: row.get("model")?,
        iterations: row.get::<_, i64>("iterations")?.clamp(0, u32::MAX as i64) as u32,
        summary: row.get("summary")?,
        error: row.get("error")?,
        owned_paths,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        finished_at: row.get("finished_at")?,
        background: row.get::<_, i64>("background").unwrap_or(0) != 0,
        note: row.get::<_, String>("note").unwrap_or_default(),
        progress: row.get("progress").ok(),
    })
}

fn map_agent_summary(row: &Row<'_>) -> rusqlite::Result<AgentSummary> {
    Ok(AgentSummary {
        agent: map_agent(row)?,
        session_title: row.get("session_title")?,
        workspace: row.get("workspace")?,
        project_id: row.get("project_id")?,
    })
}

fn map_approval(row: &Row<'_>) -> rusqlite::Result<ApprovalRecord> {
    let payload = json_value(row.get("payload")?, "approvals.payload");
    Ok(ApprovalRecord {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        run_id: row.get("run_id")?,
        agent_id: row.get("agent_id")?,
        kind: row.get("kind")?,
        title: row.get("title")?,
        detail: row.get("detail")?,
        payload,
        status: row.get("status")?,
        decision: row.get("decision")?,
        created_at: row.get("created_at")?,
        resolved_at: row.get("resolved_at")?,
    })
}

fn map_claim(row: &Row<'_>) -> rusqlite::Result<ClaimRecord> {
    Ok(ClaimRecord {
        path: row.get("path")?,
        session_id: row.get("session_id")?,
        agent_id: row.get("agent_id")?,
        mode: row.get("mode")?,
        created_at: row.get("created_at")?,
    })
}

/// Canonical form used for overlap checks: no leading `./`, no trailing `/`.
fn normalize_claim_path(raw: &str) -> String {
    let mut p = raw.trim();
    while let Some(rest) = p.strip_prefix("./") {
        p = rest;
    }
    let mut p = p.trim_end_matches('/');
    if p == "." {
        p = "";
    }
    p.to_string()
}

/// Two claims overlap when they are identical or one is an ancestor directory of
/// the other (prefix ending at a `/` boundary, never at an arbitrary character).
fn paths_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let a_under_b = a
        .strip_prefix(b)
        .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1);
    let b_under_a = b
        .strip_prefix(a)
        .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1);
    a_under_b || b_under_a
}

/// Take the exclusive advisory lock that makes a data directory single-instance.
///
/// `flock` is released automatically when the returned `File` is dropped or the
/// process dies, so a crashed server never leaves a stale lock behind — which is
/// why this is a lock file and not a pid file. The pid is written into it purely
/// so an operator can see who holds it.
fn acquire_instance_lock(data_dir: &Path) -> Result<std::fs::File> {
    let lock_path = data_dir.join(LOCK_FILE_NAME);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::io(&lock_path, e))?;
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::io::AsRawFd;
        // SAFETY: `file` owns a valid descriptor for the duration of the call.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                let holder = std::fs::read_to_string(&lock_path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "unknown pid".to_string());
                return Err(Error::Config(format!(
                    "another karucode instance is using this data directory ({}): held by {holder}. \
                     Stop it first, or point KARUCODE_DATA_DIR at a different directory.",
                    data_dir.display()
                )));
            }
            return Err(Error::io(&lock_path, err));
        }
        // Best effort: the lock is what matters, the pid is only diagnostics.
        let _ = file.set_len(0);
        let _ = write!(&file, "pid {}", std::process::id());
    }
    #[cfg(not(unix))]
    {
        let _ = &lock_path;
    }
    Ok(file)
}

/// Apply the connection pragmas and *verify* them.
///
/// These used to be `.ok()`, which meant a filesystem without WAL support
/// silently ran in rollback-journal mode, and a failed `foreign_keys` silently
/// disabled `ON DELETE CASCADE` — leaving orphaned messages, runs, agents and
/// approvals behind forever. The two pragmas that change correctness are fatal;
/// `synchronous` is only a durability trade-off, so it warns.
fn configure_connection(conn: &rusqlite::Connection, path: &Path) -> Result<()> {
    let shown = path.display();
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| Error::Store(format!("set busy_timeout on {shown}: {e}")))?;

    // `auto_vacuum` must be set *before* the database switches to WAL: once the
    // file is in WAL mode SQLite ignores the pragma, and on a non-empty database
    // it only takes effect after a full VACUUM. So: set it while the file is
    // still empty, and tell the operator when an older file still has it off.
    let auto_vacuum: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
        .map_err(|e| db_err("read auto_vacuum", e))?;
    if auto_vacuum == 0 {
        let objects: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0))
            .map_err(|e| db_err("count sqlite_master", e))?;
        if objects == 0 {
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
                .map_err(|e| Error::Store(format!("enable auto_vacuum on {shown}: {e}")))?;
        } else {
            tracing::warn!(
                path = %shown,
                "database was created without auto_vacuum=INCREMENTAL: incremental_vacuum() \
                 will not reclaim space until a one-time `VACUUM` runs (SqliteStore::vacuum)"
            );
        }
    }

    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|e| Error::Store(format!("enable WAL on {shown}: {e}")))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::Store(format!(
            "{shown}: journal_mode is {mode:?}, not \"wal\". WAL is required for the \
             reader/writer concurrency this store relies on and is unavailable on some \
             network filesystems; move the data directory to a local disk."
        )));
    }

    if let Err(e) = conn.pragma_update(None, "synchronous", "NORMAL") {
        tracing::warn!(error = %e, "could not set synchronous=NORMAL; keeping the default");
    }

    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| Error::Store(format!("enable foreign_keys on {shown}: {e}")))?;
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(|e| db_err("read foreign_keys", e))?;
    if foreign_keys != 1 {
        return Err(Error::Store(format!(
            "{shown}: foreign_keys is still off after enabling it; ON DELETE CASCADE would \
             silently leave orphaned messages, runs, agents and approvals behind"
        )));
    }
    Ok(())
}

/// Actionable error for a database that failed `PRAGMA quick_check` or
/// `integrity_check`. A raw rusqlite string tells the operator nothing about
/// what to do next.
fn corruption_error(path: &Path, problems: &[String]) -> Error {
    let shown = path.display();
    let detail = problems.join("; ");
    Error::Store(format!(
        "database {shown} failed its integrity check: {detail}. Likely causes: the process \
         was killed mid-write, the disk filled up, or the file was copied while in use. \
         Recover with `sqlite3 {shown} \".recover\" | sqlite3 {shown}.recovered`, then move \
         {shown}.recovered over {shown} (keep a copy of the original first)."
    ))
}

/// Open the read-only connections used by the `list_*` / `get_*` paths.
async fn open_read_connections(path: &Path) -> Result<Vec<Mutex<rusqlite::Connection>>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<Vec<Mutex<rusqlite::Connection>>> {
        let mut out = Vec::with_capacity(READ_POOL_SIZE);
        for _ in 0..READ_POOL_SIZE {
            let conn = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|e| Error::Store(format!("open read-only {}: {e}", path.display())))?;
            conn.busy_timeout(Duration::from_secs(5))
                .map_err(|e| db_err("read connection busy_timeout", e))?;
            out.push(Mutex::new(conn));
        }
        Ok(out)
    })
    .await
    .map_err(|e| Error::Store(format!("join: {e}")))?
}

impl SqliteStore {
    /// Open (creating if needed) the database at `path`.
    ///
    /// Takes the exclusive advisory lock on `<data_dir>/karucode.lock` first, so
    /// a second server pointed at the same data directory fails here with a
    /// clear message instead of racing migrations and crash recovery.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let data_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        if !data_dir.as_os_str().is_empty() {
            let existed = data_dir.exists();
            std::fs::create_dir_all(&data_dir).map_err(|e| Error::io(&data_dir, e))?;
            if !existed {
                // Settings may hold an API key: never leave the directory
                // readable by other local users.
                restrict_permissions(&data_dir, 0o700)?;
            }
        }
        let lock = acquire_instance_lock(&data_dir)?;
        let instance_id = uuid::Uuid::new_v4().to_string();

        let db_path = path.clone();
        let conn = tokio::task::spawn_blocking(move || -> Result<rusqlite::Connection> {
            let conn = rusqlite::Connection::open(&db_path)
                .map_err(|e| Error::Store(format!("open {}: {e}", db_path.display())))?;
            // Tighten the file mode before WAL/journal files are created so they
            // inherit it (SQLite copies the database file's permissions).
            restrict_permissions(&db_path, 0o600)?;
            configure_connection(&conn, &db_path)?;
            Ok(conn)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))??;

        let mut store = Self {
            conn: Arc::new(Mutex::new(conn)),
            readers: Arc::new(Vec::new()),
            next_reader: AtomicUsize::new(0),
            path,
            instance_id,
            _lock: Some(lock),
            last_integrity_check: AtomicI64::new(0),
        };
        // Before any migration: a corrupt file must be reported, not written to.
        store.check_integrity_on_open().await?;
        store.migrate().await?;
        store.open_read_pool().await?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Identity this process stamps on the runs and agents it creates. Crash
    /// recovery only touches rows carrying this id.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Run a blocking closure with exclusive access to the writer connection.
    ///
    /// The mutex deliberately does not poison ([`parking_lot::Mutex`]): `f` is
    /// caller-supplied code that can panic, and with `std::sync::Mutex` a single
    /// panic would poison the writer and make every later call fail with
    /// "database mutex poisoned" for the lifetime of the process - a store that
    /// never recovers, unlike the runtime, which catches panics per tool. The
    /// connection is left in whatever state rusqlite guarantees (any open
    /// transaction is rolled back when its guard unwinds), so the next caller
    /// gets a usable connection.
    pub(crate) async fn with<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock();
            f(&mut guard)
        })
        .await
        .map_err(|e| Error::Store(format!("blocking task failed: {e}")))?
    }

    /// Run a blocking closure on a read-only connection.
    ///
    /// WAL gives every reader a consistent snapshot without blocking the
    /// writer, so the `list_*` / `get_*` paths use this instead of [`Self::with`].
    /// Falls back to the writer connection when the pool is empty (a store built
    /// without [`Self::open_read_pool`], e.g. in a unit test).
    pub(crate) async fn read<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    {
        if self.readers.is_empty() {
            return self.with(move |conn| f(conn)).await;
        }
        let readers = self.readers.clone();
        let index = self.next_reader.fetch_add(1, Ordering::Relaxed) % readers.len();
        tokio::task::spawn_blocking(move || {
            let guard = readers[index].lock();
            f(&guard)
        })
        .await
        .map_err(|e| Error::Store(format!("blocking task failed: {e}")))?
    }

    /// Open the read-only pool. Failure is not fatal: the store keeps working
    /// with the writer connection, only without reader/writer concurrency.
    async fn open_read_pool(&mut self) -> Result<()> {
        match open_read_connections(&self.path).await {
            Ok(readers) => {
                self.readers = Arc::new(readers);
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "read-only connection pool unavailable; reads will use the writer connection"
                );
                Ok(())
            }
        }
    }

    /// `PRAGMA quick_check` before the first migration. Cheap (it skips the
    /// index/row cross-checks) and enough to catch a truncated or overwritten
    /// file before we write to it.
    async fn check_integrity_on_open(&self) -> Result<()> {
        let path = self.path.clone();
        let result = self
            .with(move |conn| {
                let mut stmt = conn
                    .prepare("PRAGMA quick_check")
                    .map_err(|e| db_err("quick_check prepare", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| db_err("quick_check query", e))?;
                let mut problems = Vec::new();
                for row in rows {
                    let line = row.map_err(|e| db_err("quick_check row", e))?;
                    if !line.eq_ignore_ascii_case("ok") {
                        problems.push(line);
                    }
                }
                Ok(problems)
            })
            .await?;
        if result.is_empty() {
            self.last_integrity_check.store(now_ms(), Ordering::Relaxed);
            return Ok(());
        }
        Err(corruption_error(&path, &result))
    }

    /// Forward-only, replayable migrations driven by `PRAGMA user_version`.
    ///
    /// Each step runs in its own transaction: the DDL, the step's `ADD COLUMN`s
    /// and the version bump are committed together, so a crash mid-migration
    /// leaves the previous version intact and the step runs again next boot.
    ///
    /// Every statement is idempotent, which is a requirement rather than a
    /// nicety. `sqlite3 db .dump` does not carry `user_version`, so the ordinary
    /// backup idiom (dump, restore into a fresh file) yields a database with the
    /// *current schema* and version 0. Migrations then replay from v1 and must
    /// be able to run over tables that already have everything: `CREATE TABLE IF
    /// NOT EXISTS` / `CREATE INDEX IF NOT EXISTS` in the batches, and
    /// [`add_column_if_missing`] for the columns, because SQLite has no
    /// `ADD COLUMN IF NOT EXISTS`. Without that, a restore was permanently
    /// unbootable with "duplicate column name".
    ///
    /// A database written by a *newer* build is still refused rather than
    /// touched.
    async fn migrate(&self) -> Result<()> {
        self.with(|conn| {
            let current: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(|e| db_err("read user_version", e))?;
            if current > SCHEMA_VERSION {
                // Database written by a newer build: refuse rather than corrupt it.
                return Err(Error::Store(format!(
                    "database schema v{current} is newer than supported v{SCHEMA_VERSION}"
                )));
            }
            if current == SCHEMA_VERSION {
                return Ok(());
            }
            for version in (current + 1)..=SCHEMA_VERSION {
                let tx = conn
                    .transaction()
                    .map_err(|e| db_err("begin migration", e))?;
                let batch = match version {
                    1 => MIGRATION_1,
                    2 => MIGRATION_2,
                    3 => MIGRATION_3,
                    4 => MIGRATION_4,
                    5 => MIGRATION_5,
                    6 => MIGRATION_6,
                    7 => MIGRATION_7,
                    8 => MIGRATION_8,
                    9 => MIGRATION_9,
                    10 => MIGRATION_10,
                    11 => MIGRATION_11,
                    12 => MIGRATION_12,
                    13 => MIGRATION_13,
                    14 => MIGRATION_14,
                    15 => MIGRATION_15,
                    16 => MIGRATION_16,
                    17 => "",
                    18 => MIGRATION_18,
                    19 => MIGRATION_19,
                    20 => MIGRATION_20,
                    21 => MIGRATION_21,
                    other => {
                        return Err(Error::Store(format!("missing migration v{other}")));
                    }
                };
                // Columns first: a step's batch may index one of them (v2's
                // `idx_sessions_project_id`), and both are idempotent, so the
                // order only matters for the index that depends on the column.
                for (_, table, column, decl) in ADDED_COLUMNS
                    .iter()
                    .filter(|(introduced, ..)| *introduced == version)
                {
                    add_column_if_missing(&tx, table, column, decl)?;
                }
                tx.execute_batch(batch)
                    .map_err(|e| db_err(&format!("migration v{version}"), e))?;
                tx.pragma_update(None, "user_version", version)
                    .map_err(|e| db_err("set user_version", e))?;
                tx.commit().map_err(|e| db_err("commit migration", e))?;
                tracing::info!(version, "applied database migration");
            }
            Ok(())
        })
        .await
    }
}

fn map_mission(row: &Row<'_>) -> rusqlite::Result<Mission> {
    Ok(Mission {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        description: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn map_mission_record(row: &Row<'_>) -> rusqlite::Result<MissionRecord> {
    Ok(MissionRecord {
        id: row.get(0)?,
        mission_id: row.get(1)?,
        request_id: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        content: row.get(5)?,
        task_id: row.get(6)?,
        session_id: row.get(7)?,
        artifact_id: row.get(8)?,
        created_at: row.get(9)?,
    })
}

#[async_trait]
impl Store for SqliteStore {
    async fn publish_mission_record(
        &self,
        mut record: MissionRecord,
    ) -> Result<(MissionRecord, bool)> {
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("record transaction", e))?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM missions WHERE id=?1)", [&record.mission_id], |r| r.get(0)).map_err(|e| db_err("mission lookup", e))?;
            if !exists { return Err(Error::NotFound("mission not found".into())); }
            let old = tx.query_row("SELECT id,mission_id,request_id,kind,title,content,task_id,session_id,artifact_id,created_at FROM mission_records WHERE mission_id=?1 AND request_id=?2", params![record.mission_id,record.request_id], map_mission_record).optional().map_err(|e| db_err("record lookup",e))?;
            if let Some(old) = old {
                if old.kind != record.kind || old.title != record.title || old.content != record.content || old.task_id != record.task_id || old.session_id != record.session_id || old.artifact_id != record.artifact_id {
                    return Err(Error::BadRequest("request_id already used for a different record".into()));
                }
                return Ok((old, false));
            }
            if let Some(ref task) = record.task_id {
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mission_tasks WHERE mission_id=?1 AND id=?2 AND (?3 IS NULL OR session_id=?3 OR EXISTS(SELECT 1 FROM mission_task_retries WHERE mission_id=?1 AND task_id=?2 AND previous_session_id=?3)))", params![record.mission_id,task,record.session_id], |r| r.get(0)).map_err(|e| db_err("record task lookup",e))?;
                if !valid { return Err(Error::BadRequest("task must belong to the mission and match the supplied session".into())); }
            }
            if let Some(ref session) = record.session_id {
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mission_sessions WHERE mission_id=?1 AND session_id=?2 UNION ALL SELECT 1 FROM mission_tasks WHERE mission_id=?1 AND session_id=?2 UNION ALL SELECT 1 FROM mission_task_retries WHERE mission_id=?1 AND previous_session_id=?2)", params![record.mission_id,session], |r| r.get(0)).map_err(|e| db_err("record session lookup",e))?;
                if !valid { return Err(Error::BadRequest("session must belong to the mission".into())); }
            }
            if let Some(ref artifact) = record.artifact_id {
                let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mission_records WHERE mission_id=?1 AND id=?2 AND kind='artifact')", params![record.mission_id,artifact], |r| r.get(0)).map_err(|e| db_err("artifact lookup",e))?;
                if record.kind != "evidence" || !valid { return Err(Error::BadRequest("artifact_id must reference an artifact in this mission and is only valid on evidence".into())); }
            }
            if record.kind != "artifact" && record.kind != "evidence" { return Err(Error::BadRequest("invalid record kind".into())); }
            record.created_at = now_ms();
            tx.execute("INSERT INTO mission_records VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![record.id,record.mission_id,record.request_id,record.kind,record.title,record.content,record.task_id,record.session_id,record.artifact_id,record.created_at]).map_err(|e| db_err("publish record",e))?;
            tx.commit().map_err(|e| db_err("commit record",e))?;
            Ok((record, true))
        }).await
    }
    async fn list_mission_records(
        &self,
        mission_id: &str,
        kind: &str,
    ) -> Result<Vec<MissionRecord>> {
        let mission_id = mission_id.to_owned();
        let kind = kind.to_owned();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT id,mission_id,request_id,kind,title,content,task_id,session_id,artifact_id,created_at FROM mission_records WHERE mission_id=?1 AND kind=?2 ORDER BY rowid").map_err(|e| db_err("list records",e))?;
            let rows = stmt.query_map(params![mission_id,kind], map_mission_record).map_err(|e| db_err("list records",e))?;
            rows.collect::<std::result::Result<Vec<_>,_>>().map_err(|e| db_err("list records",e))
        }).await
    }
    async fn get_mission_artifacts(
        &self,
        mission_id: &str,
        ids: &[String],
    ) -> Result<Vec<MissionRecord>> {
        if ids.len() > 16 {
            return Err(Error::BadRequest(
                "at most 16 artifacts may be selected".into(),
            ));
        }
        let mission_id = mission_id.to_owned();
        let ids = ids.to_vec();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT id,mission_id,request_id,kind,title,content,task_id,session_id,artifact_id,created_at FROM mission_records WHERE id=?1 AND mission_id=?2 AND kind='artifact'")
                .map_err(|e| db_err("selected artifact lookup", e))?;
            let mut selected = Vec::with_capacity(ids.len());
            let mut seen = std::collections::HashSet::new();
            let mut total = 0usize;
            for id in &ids {
                if !seen.insert(id) {
                    return Err(Error::BadRequest("artifact IDs must be unique".into()));
                }
                let record = stmt.query_row(params![id, mission_id], map_mission_record)
                    .optional().map_err(|e| db_err("selected artifact lookup", e))?
                    .ok_or_else(|| Error::Store("selected mission artifact no longer exists".into()))?;
                total += record.title.len() + record.content.len();
                if total > 256 * 1024 {
                    return Err(Error::Store("selected artifact content exceeds 256 KiB".into()));
                }
                selected.push(record);
            }
            Ok(selected)
        }).await
    }
    async fn create_mission_task(&self, task: MissionTask) -> Result<MissionTask> {
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("task transaction", e))?;
            let mission: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM missions WHERE id=?1)",
                    [&task.mission_id],
                    |r| r.get(0),
                )
                .map_err(|e| db_err("mission lookup", e))?;
            if !mission {
                return Err(Error::NotFound("mission not found".into()));
            }
            let project: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
                    [&task.project_id],
                    |r| r.get(0),
                )
                .map_err(|e| db_err("project lookup", e))?;
            if !project {
                return Err(Error::NotFound("project not found".into()));
            }
            let started: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM mission_schedules WHERE mission_id=?1)",
                    [&task.mission_id],
                    |r| r.get(0),
                )
                .map_err(|e| db_err("schedule lookup", e))?;
            if started {
                return Err(Error::BadRequest("mission already started".into()));
            }
            let mut seen = std::collections::HashSet::new();
            for dep in &task.dependencies {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM mission_tasks WHERE id=?1 AND mission_id=?2)",
                        params![dep, task.mission_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| db_err("dependency lookup", e))?;
                if dep == &task.id || !exists || !seen.insert(dep) {
                    return Err(Error::BadRequest(
                        "dependencies must be unique existing tasks in this mission".into(),
                    ));
                }
            }
            let mut seen_artifacts = std::collections::HashSet::new();
            if task.artifact_ids.len() > 16 {
                return Err(Error::BadRequest("at most 16 artifacts may be selected".into()));
            }
            let mut total = 0usize;
            for artifact in &task.artifact_ids {
                if !seen_artifacts.insert(artifact) {
                    return Err(Error::BadRequest("artifact IDs must be unique".into()));
                }
                let size: Option<i64> = tx.query_row(
                    "SELECT length(CAST(content AS BLOB)) + length(CAST(title AS BLOB)) FROM mission_records WHERE id=?1 AND mission_id=?2 AND kind='artifact'",
                    params![artifact, task.mission_id], |r| r.get(0)
                ).optional().map_err(|e| db_err("artifact lookup", e))?;
                total += size.ok_or_else(|| Error::BadRequest("artifact must exist in this mission".into()))? as usize;
            }
            if total > 256 * 1024 {
                return Err(Error::BadRequest("selected artifact content exceeds 256 KiB".into()));
            }
            let artifact_ids = serde_json::to_string(&task.artifact_ids).map_err(|e| Error::Store(e.to_string()))?;
            let deps = serde_json::to_string(&task.dependencies)
                .map_err(|e| Error::Store(e.to_string()))?;
            tx.execute(
                "INSERT INTO mission_tasks (id,mission_id,project_id,title,prompt,dependencies,state,session_id,run_id,error,created_at,artifact_ids) VALUES (?1,?2,?3,?4,?5,?6,'draft',NULL,NULL,NULL,?7,?8)",
                params![
                    task.id,
                    task.mission_id,
                    task.project_id,
                    task.title,
                    task.prompt,
                    deps,
                    now_ms(),
                    artifact_ids
                ],
            )
            .map_err(|e| db_err("create task", e))?;
            tx.commit().map_err(|e| db_err("commit task", e))?;
            Ok(task)
        })
        .await
    }
    async fn revise_mission_task(
        &self,
        mission_id: &str,
        task_id: &str,
        project_id: Option<String>,
        prompt: Option<String>,
        dependencies: Option<Vec<String>>,
    ) -> Result<MissionTask> {
        let (mission_id, task_id) = (mission_id.to_owned(), task_id.to_owned());
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("revise task", e))?;
            let started: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mission_schedules WHERE mission_id=?1)", [&mission_id], |r| r.get(0)).map_err(|e| db_err("schedule lookup", e))?;
            if started { return Err(Error::BadRequest("mission already started".into())); }
            let mut tasks = {
                let mut stmt = tx.prepare("SELECT id,mission_id,project_id,title,prompt,dependencies,state,session_id,run_id,error,artifact_ids FROM mission_tasks WHERE mission_id=?1").map_err(|e| db_err("list tasks", e))?;
                let rows = stmt.query_map([&mission_id], |r| {
                    let deps: String = r.get(5)?;
                    let artifacts: String = r.get(10)?;
                    Ok(MissionTask { id:r.get(0)?, mission_id:r.get(1)?, project_id:r.get(2)?, title:r.get(3)?, prompt:r.get(4)?, dependencies:serde_json::from_str(&deps).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5,rusqlite::types::Type::Text,Box::new(e)))?, state:r.get(6)?, session_id:r.get(7)?, run_id:r.get(8)?, error:r.get(9)?, artifact_ids:serde_json::from_str(&artifacts).map_err(|e| rusqlite::Error::FromSqlConversionFailure(10,rusqlite::types::Type::Text,Box::new(e)))? })
                }).map_err(|e| db_err("list tasks", e))?;
                rows.collect::<std::result::Result<Vec<_>,_>>().map_err(|e| db_err("list tasks", e))?
            };
            let index = tasks.iter().position(|t| t.id == task_id).ok_or_else(|| Error::NotFound("task not found in mission".into()))?;
            if tasks[index].state != "draft" { return Err(Error::BadRequest("task is not a draft".into())); }
            if let Some(project) = project_id {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)", [&project], |r| r.get(0)).map_err(|e| db_err("project lookup",e))?;
                if !exists { return Err(Error::NotFound("project not found".into())); }
                tasks[index].project_id = project;
            }
            if let Some(prompt) = prompt { tasks[index].prompt = prompt; }
            if let Some(deps) = dependencies {
                if deps.len() > 256 { return Err(Error::BadRequest("too many dependencies".into())); }
                let mut seen = std::collections::HashSet::new();
                for dep in &deps {
                    if dep == &task_id || !seen.insert(dep) || !tasks.iter().any(|t| &t.id == dep) {
                        return Err(Error::BadRequest("dependencies must be unique existing tasks in this mission".into()));
                    }
                }
                // Following dependencies from each new edge must never reach this task.
                let mut stack = deps.clone();
                let mut visited = std::collections::HashSet::new();
                while let Some(id) = stack.pop() {
                    if id == task_id { return Err(Error::BadRequest("dependencies would create a cycle".into())); }
                    if visited.insert(id.clone()) {
                        if let Some(task) = tasks.iter().find(|t| t.id == id) { stack.extend(task.dependencies.iter().cloned()); }
                    }
                }
                tasks[index].dependencies = deps;
            }
            let task = tasks.swap_remove(index);
            let deps = serde_json::to_string(&task.dependencies).map_err(|e| Error::Store(e.to_string()))?;
            tx.execute("UPDATE mission_tasks SET project_id=?1,prompt=?2,dependencies=?3 WHERE id=?4 AND mission_id=?5", params![task.project_id,task.prompt,deps,task.id,mission_id]).map_err(|e| db_err("revise task",e))?;
            tx.commit().map_err(|e| db_err("commit revision",e))?;
            Ok(task)
        }).await
    }
    async fn replan_mission_task(
        &self,
        mission_id: &str,
        task_id: &str,
        expected: MissionTaskPlan,
        project_id: Option<String>,
        prompt: Option<String>,
        dependencies: Option<Vec<String>>,
    ) -> Result<MissionTask> {
        let (mission_id, task_id) = (mission_id.to_owned(), task_id.to_owned());
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("revise task", e))?;
            let schedule: Option<String> = tx.query_row("SELECT state FROM mission_schedules WHERE mission_id=?1", [&mission_id], |r| r.get(0)).optional().map_err(|e| db_err("schedule lookup", e))?;
            if !matches!(schedule.as_deref(), Some("active" | "paused")) { return Err(Error::BadRequest("mission must be started and not cancelled".into())); }
            let mut tasks = {
                let mut stmt = tx.prepare("SELECT id,mission_id,project_id,title,prompt,dependencies,state,session_id,run_id,error,artifact_ids FROM mission_tasks WHERE mission_id=?1").map_err(|e| db_err("list tasks", e))?;
                let rows = stmt.query_map([&mission_id], |r| {
                    let deps: String = r.get(5)?;
                    let artifacts: String = r.get(10)?;
                    Ok(MissionTask { id:r.get(0)?, mission_id:r.get(1)?, project_id:r.get(2)?, title:r.get(3)?, prompt:r.get(4)?, dependencies:serde_json::from_str(&deps).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5,rusqlite::types::Type::Text,Box::new(e)))?, state:r.get(6)?, session_id:r.get(7)?, run_id:r.get(8)?, error:r.get(9)?, artifact_ids:serde_json::from_str(&artifacts).map_err(|e| rusqlite::Error::FromSqlConversionFailure(10,rusqlite::types::Type::Text,Box::new(e)))? })
                }).map_err(|e| db_err("list tasks", e))?;
                rows.collect::<std::result::Result<Vec<_>,_>>().map_err(|e| db_err("list tasks", e))?
            };
            let index = tasks.iter().position(|t| t.id == task_id).ok_or_else(|| Error::NotFound("task not found in mission".into()))?;
            let task = &tasks[index];
            let retried: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM mission_task_retries WHERE mission_id=?1 AND task_id=?2)", params![mission_id,task_id], |r| r.get(0)).map_err(|e| db_err("retry lookup",e))?;
            if !matches!(task.state.as_str(), "pending" | "blocked") || task.session_id.is_some() || task.run_id.is_some() || retried { return Err(Error::BadRequest("only never-claimed pending or blocked tasks can be replanned".into())); }
            if task.project_id != expected.project_id || task.prompt != expected.prompt || task.dependencies != expected.dependencies { return Err(Error::BadRequest("task plan changed; refresh and review it again".into())); }
            if let Some(project) = project_id {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)", [&project], |r| r.get(0)).map_err(|e| db_err("project lookup",e))?;
                if !exists { return Err(Error::NotFound("project not found".into())); }
                tasks[index].project_id = project;
            }
            if let Some(prompt) = prompt { tasks[index].prompt = prompt; }
            if let Some(deps) = dependencies {
                if deps.len() > 256 { return Err(Error::BadRequest("too many dependencies".into())); }
                let mut seen = std::collections::HashSet::new();
                for dep in &deps {
                    if dep == &task_id || !seen.insert(dep) || !tasks.iter().any(|t| &t.id == dep) {
                        return Err(Error::BadRequest("dependencies must be unique existing tasks in this mission".into()));
                    }
                }
                // Following dependencies from each new edge must never reach this task.
                let mut stack = deps.clone();
                let mut visited = std::collections::HashSet::new();
                while let Some(id) = stack.pop() {
                    if id == task_id { return Err(Error::BadRequest("dependencies would create a cycle".into())); }
                    if visited.insert(id.clone()) {
                        if let Some(task) = tasks.iter().find(|t| t.id == id) { stack.extend(task.dependencies.iter().cloned()); }
                    }
                }
                tasks[index].dependencies = deps;
            }
            // Reconsider the edited blocked task and its blocked descendants; the
            // scheduler will re-block any whose dependencies still failed.
            if tasks[index].state == "blocked" { tasks[index].state = "pending".into(); tasks[index].error = None; }
            let task = tasks.swap_remove(index);
            let deps = serde_json::to_string(&task.dependencies).map_err(|e| Error::Store(e.to_string()))?;
            tx.execute("UPDATE mission_tasks SET project_id=?1,prompt=?2,dependencies=?3,state=?6,error=?7 WHERE id=?4 AND mission_id=?5", params![task.project_id,task.prompt,deps,task.id,mission_id,task.state,task.error]).map_err(|e| db_err("revise task",e))?;
            tx.execute("UPDATE mission_tasks SET state='pending',error=NULL WHERE mission_id=?1 AND state='blocked'", [&mission_id]).map_err(|e| db_err("reconsider blocked tasks",e))?;
            tx.commit().map_err(|e| db_err("commit revision",e))?;
            Ok(task)
        }).await
    }
    async fn list_mission_tasks(&self, mission_id: &str) -> Result<Vec<MissionTask>> {
        let mission_id = mission_id.to_owned();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT id,mission_id,project_id,title,prompt,dependencies,state,session_id,run_id,error,artifact_ids FROM mission_tasks WHERE mission_id=?1 ORDER BY rowid").map_err(|e| db_err("list tasks",e))?;
            let rows = stmt.query_map([mission_id], |r| { let deps: String = r.get(5)?; Ok(MissionTask { id:r.get(0)?, mission_id:r.get(1)?, project_id:r.get(2)?, title:r.get(3)?, prompt:r.get(4)?, dependencies:serde_json::from_str(&deps).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5,rusqlite::types::Type::Text,Box::new(e)))?, artifact_ids: serde_json::from_str(&r.get::<_, String>(10)?).map_err(|e| rusqlite::Error::FromSqlConversionFailure(10,rusqlite::types::Type::Text,Box::new(e)))?, state:r.get(6)?, session_id:r.get(7)?, run_id:r.get(8)?, error:r.get(9)? }) }).map_err(|e| db_err("list tasks",e))?;
            rows.collect::<std::result::Result<Vec<_>,_>>().map_err(|e| db_err("list tasks",e))
        }).await
    }
    async fn start_mission_tasks(&self, mission_id: &str, limit: u32) -> Result<()> {
        let id = mission_id.to_owned();
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("start mission", e))?;
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM missions WHERE id=?1)",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(|e| db_err("mission lookup", e))?;
            if !exists {
                return Err(Error::NotFound("mission not found".into()));
            }
            if !(1..=8).contains(&limit) {
                return Err(Error::BadRequest("max_concurrency must be 1..=8".into()));
            }
            tx.execute(
                "INSERT INTO mission_schedules (mission_id,concurrency) VALUES (?1,?2) ON CONFLICT(mission_id) DO NOTHING",
                params![id, limit],
            )
            .map_err(|e| db_err("start mission", e))?;
            tx.execute(
                "UPDATE mission_tasks SET state='pending' WHERE mission_id=?1 AND state='draft'",
                [&id],
            )
            .map_err(|e| db_err("approve tasks", e))?;
            tx.commit().map_err(|e| db_err("commit start", e))?;
            Ok(())
        })
        .await
    }
    async fn mission_schedule_state(&self, mission_id: &str) -> Result<Option<String>> {
        let id = mission_id.to_owned();
        self.with(move |conn| {
            conn.query_row(
                "SELECT state FROM mission_schedules WHERE mission_id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| db_err("schedule state", e))
        })
        .await
    }
    async fn control_mission_tasks(&self, mission_id: &str, action: &str) -> Result<()> {
        let (id, action) = (mission_id.to_owned(), action.to_owned());
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("control mission", e))?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM missions WHERE id=?1)", [&id], |r| r.get(0)).map_err(|e| db_err("mission lookup", e))?;
            if !exists { return Err(Error::NotFound("mission not found".into())); }
            let current: Option<String> = tx.query_row("SELECT state FROM mission_schedules WHERE mission_id=?1", [&id], |r| r.get(0)).optional().map_err(|e| db_err("schedule lookup", e))?;
            let next = match (action.as_str(), current.as_deref()) {
                ("pause", Some("active" | "paused")) => "paused",
                ("resume", Some("paused" | "active")) => "active",
                ("cancel", Some("active" | "paused" | "cancelled")) => "cancelled",
                _ => return Err(Error::BadRequest("invalid mission control transition".into())),
            };
            tx.execute("UPDATE mission_schedules SET state=?2 WHERE mission_id=?1", params![id, next]).map_err(|e| db_err("control schedule", e))?;
            if next == "cancelled" {
                tx.execute("UPDATE mission_tasks SET state='cancelled', error='mission cancelled before claim' WHERE mission_id=?1 AND state IN ('draft','pending')", [&id]).map_err(|e| db_err("cancel tasks", e))?;
            }
            tx.commit().map_err(|e| db_err("commit mission control", e))?;
            Ok(())
        }).await
    }
    async fn retry_mission_task(
        &self,
        mission_id: &str,
        task_id: &str,
        request_id: &str,
        project_id: Option<&str>,
    ) -> Result<bool> {
        let (mission, task, request, project) = (
            mission_id.to_owned(),
            task_id.to_owned(),
            request_id.to_owned(),
            project_id.map(str::to_owned),
        );
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("retry task", e))?;
            let existing: Option<(String, Option<String>)> = tx.query_row(
                "SELECT task_id,project_id FROM mission_task_retries WHERE mission_id=?1 AND request_id=?2",
                params![mission, request], |r| Ok((r.get(0)?,r.get(1)?)))
                .optional().map_err(|e| db_err("retry lookup", e))?;
            if let Some((old_task, old_project)) = existing {
                if old_task != task || old_project != project { return Err(Error::BadRequest("request_id already used for a different retry".into())); }
                return Ok(false);
            }
            let schedule: Option<String> = tx.query_row("SELECT state FROM mission_schedules WHERE mission_id=?1", [&mission], |r| r.get(0)).optional().map_err(|e| db_err("retry schedule",e))?;
            if !matches!(schedule.as_deref(), Some("active" | "paused")) { return Err(Error::BadRequest("mission must be started and not cancelled".into())); }
            let previous: Option<(String,String,Option<String>,Option<String>,Option<String>)> = tx.query_row(
                "SELECT project_id,state,session_id,run_id,error FROM mission_tasks WHERE mission_id=?1 AND id=?2", params![mission,task],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e| db_err("retry task lookup",e))?;
            let (old_project, old_state, session, run, error) = previous.ok_or_else(|| Error::NotFound("task not found".into()))?;
            if !matches!(old_state.as_str(), "failed" | "interrupted" | "cancelled") { return Err(Error::BadRequest("only failed, interrupted or cancelled tasks can be retried".into())); }
            let target = project.as_deref().unwrap_or(&old_project);
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",[target],|r|r.get(0)).map_err(|e|db_err("retry project",e))?;
            if !exists { return Err(Error::NotFound("project not found".into())); }
            tx.execute("INSERT INTO mission_task_retries VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![mission,request,task,project,old_project,old_state,session,run,error]).map_err(|e| db_err("reserve retry",e))?;
            tx.execute("UPDATE mission_tasks SET project_id=?3,state='pending',session_id=NULL,run_id=NULL,error=NULL WHERE mission_id=?1 AND id=?2",params![mission,task,target]).map_err(|e|db_err("retry task",e))?;
            // Previously blocked descendants are reconsidered; the scheduler will
            // re-block those still depending on any other failed task.
            tx.execute("UPDATE mission_tasks SET state='pending',error=NULL WHERE mission_id=?1 AND state='blocked'",[&mission]).map_err(|e|db_err("reconsider blocked tasks",e))?;
            tx.commit().map_err(|e|db_err("commit retry",e))?;
            Ok(true)
        }).await
    }
    async fn claim_mission_task(
        &self,
        mission_id: &str,
        session_id: &str,
    ) -> Result<Option<MissionTask>> {
        let id = mission_id.to_owned();
        let sid = session_id.to_owned();
        self.with(move |conn| {
            let tx=conn.transaction().map_err(|e| db_err("claim task",e))?;
            let limit: Option<i64>=tx.query_row("SELECT concurrency FROM mission_schedules WHERE mission_id=?1 AND state='active'",[&id],|r|r.get(0)).optional().map_err(|e|db_err("schedule",e))?;
            let Some(limit)=limit else { return Ok(None) };
            // Propagate terminal dependency failures to a fixed point.
            loop {
                let n=tx.execute("UPDATE mission_tasks SET state='blocked', error='dependency did not complete' WHERE mission_id=?1 AND state='pending' AND EXISTS (SELECT 1 FROM json_each(mission_tasks.dependencies) d JOIN mission_tasks parent ON parent.id=d.value WHERE parent.state IN ('failed','blocked','cancelled','interrupted'))",[&id]).map_err(|e|db_err("block tasks",e))?;
                if n==0 { break; }
            }
            let running:i64=tx.query_row("SELECT COUNT(*) FROM mission_tasks WHERE mission_id=?1 AND state='running'",[&id],|r|r.get(0)).map_err(|e|db_err("count tasks",e))?;
            let task_id:Option<String>=if running < limit { tx.query_row("SELECT id FROM mission_tasks WHERE mission_id=?1 AND state='pending' AND NOT EXISTS (SELECT 1 FROM json_each(mission_tasks.dependencies) d JOIN mission_tasks parent ON parent.id=d.value WHERE parent.state != 'completed') ORDER BY rowid LIMIT 1",[&id],|r|r.get(0)).optional().map_err(|e|db_err("ready task",e))? } else { None };
            let task = if let Some(task_id) = task_id {
                tx.execute("INSERT INTO mission_attempt_sessions (session_id,mission_id) VALUES (?1,?2)",params![sid,id]).map_err(|e|db_err("reserve task attribution",e))?;
                tx.execute("UPDATE mission_tasks SET state='running', session_id=?2 WHERE id=?1 AND state='pending'",params![task_id,sid]).map_err(|e|db_err("claim task",e))?;
                Some(tx.query_row("SELECT id,mission_id,project_id,title,prompt,dependencies,state,session_id,run_id,error,artifact_ids FROM mission_tasks WHERE id=?1", [&task_id], |r| {
                    let deps: String = r.get(5)?;
                    Ok(MissionTask { id:r.get(0)?, mission_id:r.get(1)?, project_id:r.get(2)?, title:r.get(3)?, prompt:r.get(4)?, dependencies:serde_json::from_str(&deps).map_err(|e| rusqlite::Error::FromSqlConversionFailure(5,rusqlite::types::Type::Text,Box::new(e)))?, artifact_ids: serde_json::from_str(&r.get::<_, String>(10)?).map_err(|e| rusqlite::Error::FromSqlConversionFailure(10,rusqlite::types::Type::Text,Box::new(e)))?, state:r.get(6)?, session_id:r.get(7)?, run_id:r.get(8)?, error:r.get(9)? })
                }).map_err(|e|db_err("read claimed task",e))?)
            } else { None };
            tx.commit().map_err(|e|db_err("commit claim",e))?; Ok(task)
        }).await
    }
    async fn finish_mission_task(
        &self,
        id: &str,
        state: &str,
        run_id: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        let (id, state, run_id, error) = (
            id.to_owned(),
            state.to_owned(),
            run_id.map(str::to_owned),
            error.map(str::to_owned),
        );
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("finish task transaction", e))?;
            if state != "running" {
                // Capture only persisted facts for this exact attempt, atomically with
                // the terminal transition. A missing run is not a successful run.
                let task: Option<(String, Option<String>)> = tx.query_row(
                    "SELECT mission_id,session_id FROM mission_tasks WHERE id=?1 AND state='running'", [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional().map_err(|e| db_err("capture task", e))?;
                if let Some((mission, Some(session))) = task {
                    let run: Option<(String, Option<String>, i64, Option<i64>)> = match &run_id {
                        Some(run_id) => tx.query_row(
                            "SELECT model,error,started_at,finished_at FROM runs WHERE id=?1 AND session_id=?2",
                            params![run_id, session], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
                        ).optional().map_err(|e| db_err("capture run", e))?,
                        None => None,
                    };
                    let counts: (i64, i64, i64) = match &run_id {
                        Some(run_id) => tx.query_row(
                            "SELECT COUNT(*), COALESCE(SUM(role='assistant'),0), COALESCE(SUM(role='tool'),0) FROM messages WHERE session_id=?1 AND run_id=?2 AND rewound_at IS NULL",
                            params![session,run_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
                        ).map_err(|e| db_err("capture messages", e))?,
                        None => (0,0,0),
                    };
                    let content = serde_json::json!({
                        "description": "Runtime observations, not a verification verdict. Message counts do not establish command success.",
                        "task_state": state, "task_error": error, "run_id": run_id,
                        "persisted_run": run.map(|(model, error, started_at, finished_at)| serde_json::json!({"model":model,"error":error,"started_at":started_at,"finished_at":finished_at})),
                        "visible_message_counts": {"total":counts.0,"assistant":counts.1,"tool":counts.2}
                    }).to_string();
                    let request = format!("_karucode_auto_evidence_{}_{}", id, session);
                    tx.execute("INSERT OR IGNORE INTO mission_records (id,mission_id,request_id,kind,title,content,task_id,session_id,artifact_id,created_at) VALUES (?1,?2,?3,'evidence','Automatic task run observations',?4,?5,?6,NULL,?7)", params![request,mission,request,content,id,session,now_ms()]).map_err(|e| db_err("capture evidence", e))?;
                }
            }
            tx.execute("UPDATE mission_tasks SET state=?2,run_id=COALESCE(?3,run_id),error=?4 WHERE id=?1 AND state='running'",params![id,state,run_id,error]).map_err(|e|db_err("finish task",e))?;
            tx.commit().map_err(|e| db_err("commit task finish", e))?;
            Ok(())
        }).await
    }
    async fn recover_mission_tasks(&self) -> Result<Vec<String>> {
        self.with(move |conn| {
            conn.execute("UPDATE mission_tasks SET state='interrupted',error='server stopped during dispatch or execution' WHERE state='running'",[]).map_err(|e|db_err("recover tasks",e))?;
            let mut stmt=conn.prepare("SELECT mission_id FROM mission_schedules").map_err(|e|db_err("recover schedules",e))?;
            let rows=stmt.query_map([],|r|r.get(0)).map_err(|e|db_err("recover schedules",e))?;
            rows.collect::<std::result::Result<Vec<String>,_>>().map_err(|e|db_err("recover schedules",e))
        }).await
    }
    async fn admit_provider_attempt(&self, session_id: &str) -> Result<()> {
        let session_id = session_id.to_owned();
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("attempt transaction", e))?;
            let mission: Option<String> = tx.query_row("SELECT mission_id FROM mission_attempt_sessions WHERE session_id=?1", [&session_id], |r| r.get(0)).optional().map_err(|e| db_err("attempt attribution", e))?;
            if let Some(mission) = mission {
                let n = tx.execute("UPDATE mission_attempt_budgets SET attempts=attempts+1 WHERE mission_id=?1 AND (max_attempts IS NULL OR attempts < max_attempts)", [&mission]).map_err(|e| db_err("attempt admission", e))?;
                if n != 1 { return Err(Error::Store(format!("mission {mission} provider attempt budget exhausted or unavailable"))); }
            }
            tx.commit().map_err(|e| db_err("commit attempt admission", e))?;
            Ok(())
        }).await
    }
    async fn mission_attempt_budget(&self, id: &str) -> Result<Option<(Option<u32>, u32)>> {
        let id = id.to_owned();
        self.read(move |conn| {
            conn.query_row(
                "SELECT max_attempts,attempts FROM mission_attempt_budgets WHERE mission_id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| db_err("read mission budget", e))
        })
        .await
    }

    async fn create_mission(
        &self,
        id: &str,
        project_id: &str,
        title: &str,
        description: &str,
        max_provider_attempts: Option<u32>,
    ) -> Result<Mission> {
        let (id, project_id, title, description) = (
            id.to_owned(),
            project_id.to_owned(),
            title.to_owned(),
            description.to_owned(),
        );
        self.with(move |conn| {
            let now = now_ms();
            let tx = conn
                .transaction()
                .map_err(|e| db_err("create mission transaction", e))?;
            tx.execute(
                "INSERT INTO missions VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![id, project_id, title, description, now],
            )
            .map_err(|e| db_err("create mission", e))?;
            tx.execute(
                "INSERT INTO mission_attempt_budgets (mission_id,max_attempts) VALUES (?1,?2)",
                params![id, max_provider_attempts],
            )
            .map_err(|e| db_err("create mission budget", e))?;
            tx.commit().map_err(|e| db_err("commit mission", e))?;
            Ok(Mission {
                id,
                project_id,
                title,
                description,
                created_at: now,
                updated_at: now,
            })
        })
        .await
    }

    async fn list_missions(&self, project_id: &str) -> Result<Vec<Mission>> {
        let project_id = project_id.to_owned();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT id, project_id, title, description, created_at, updated_at FROM missions WHERE project_id = ?1 ORDER BY created_at DESC, id").map_err(|e| db_err("list missions", e))?;
            let rows = stmt.query_map(params![project_id], map_mission).map_err(|e| db_err("list missions", e))?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(|e| db_err("list missions", e))
        }).await
    }

    async fn list_mission_page(
        &self,
        project_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Mission>, bool)> {
        let project_id = project_id.to_owned();
        self.read(move |conn| {
            let query_limit = limit_i64(limit).saturating_add(1);
            let mut stmt = conn.prepare("SELECT id, project_id, title, description, created_at, updated_at FROM missions WHERE project_id = ?1 ORDER BY updated_at DESC, id DESC LIMIT ?2 OFFSET ?3").map_err(|e| db_err("list mission page", e))?;
            let mut missions = stmt.query_map(params![project_id, query_limit, limit_i64(offset)], map_mission).map_err(|e| db_err("list mission page", e))?
                .collect::<std::result::Result<Vec<_>, _>>().map_err(|e| db_err("list mission page", e))?;
            let has_more = missions.len() > limit;
            missions.truncate(limit);
            Ok((missions, has_more))
        }).await
    }

    async fn get_mission(&self, id: &str) -> Result<Option<Mission>> {
        let id = id.to_owned();
        self.read(move |conn| conn.query_row("SELECT id, project_id, title, description, created_at, updated_at FROM missions WHERE id = ?1", params![id], map_mission).optional().map_err(|e| db_err("get mission", e))).await
    }

    async fn coordinator_mission(&self, session_id: &str) -> Result<Option<Mission>> {
        let session_id = session_id.to_owned();
        self.read(move |conn| conn.query_row("SELECT m.id, m.project_id, m.title, m.description, m.created_at, m.updated_at FROM missions m JOIN mission_sessions s ON s.mission_id=m.id WHERE s.session_id=?1 AND s.request_id='_karucode_coordinator'", params![session_id], map_mission).optional().map_err(|e| db_err("find coordinator mission", e))).await
    }

    async fn delete_mission(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.with(move |conn| {
            let n = conn
                .execute("DELETE FROM missions WHERE id = ?1", params![id])
                .map_err(|e| db_err("delete mission", e))?;
            if n == 0 {
                return Err(Error::NotFound(format!("mission {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn create_mission_session(
        &self,
        mission_id: &str,
        request_id: &str,
        session_id: &str,
        project_id: &str,
        title: &str,
        workspace: &str,
        prompt: &str,
    ) -> Result<bool> {
        let (mission_id, request_id, session_id, project_id, title, workspace, prompt) = (
            mission_id.to_owned(),
            request_id.to_owned(),
            session_id.to_owned(),
            project_id.to_owned(),
            title.to_owned(),
            workspace.to_owned(),
            prompt.to_owned(),
        );
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("mission session transaction", e))?;
            let existing: Option<(String, String, String)> = tx.query_row("SELECT title, prompt, project_id FROM mission_sessions WHERE mission_id = ?1 AND request_id = ?2", params![mission_id, request_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(|e| db_err("mission request lookup", e))?;
            if let Some((old_title, old_prompt, old_project)) = existing {
                if old_title != title || old_prompt != prompt || old_project != project_id { return Err(Error::BadRequest("request_id already used with different input".into())); }
                return Ok(false);
            }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM missions WHERE id = ?1)", params![mission_id], |r| r.get(0)).map_err(|e| db_err("mission lookup", e))?;
            if !exists { return Err(Error::NotFound(format!("mission {mission_id}"))); }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)", params![project_id], |r| r.get(0)).map_err(|e| db_err("project lookup", e))?;
            if !exists { return Err(Error::NotFound(format!("project {project_id}"))); }
            let now = now_ms();
            tx.execute("INSERT INTO sessions (id,title,workspace,project_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)", params![session_id,title,workspace,project_id,now]).map_err(|e| db_err("create mission session", e))?;
            tx.execute("INSERT INTO mission_sessions (mission_id,request_id,session_id,title,prompt,created_at,project_id) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![mission_id,request_id,session_id,title,prompt,now,project_id]).map_err(|e| db_err("link mission session", e))?;
            tx.execute("UPDATE missions SET updated_at = ?2 WHERE id = ?1", params![mission_id,now]).map_err(|e| db_err("touch mission", e))?;
            tx.commit().map_err(|e| db_err("commit mission session", e))?;
            Ok(true)
        }).await
    }

    async fn list_mission_sessions(&self, mission_id: &str) -> Result<Vec<MissionSession>> {
        let mission_id = mission_id.to_owned();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT session_id, request_id, prompt, created_at, project_id FROM mission_sessions WHERE mission_id = ?1 ORDER BY created_at, session_id").map_err(|e| db_err("list mission sessions", e))?;
            let rows = stmt.query_map(params![mission_id], |r| Ok(MissionSession { session_id:r.get(0)?, request_id:r.get(1)?, prompt:r.get(2)?, created_at:r.get(3)?, project_id:r.get(4)? })).map_err(|e| db_err("list mission sessions", e))?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(|e| db_err("list mission sessions", e))
        }).await
    }

    async fn create_control_session(
        &self,
        id: &str,
        title: &str,
        workspace: &str,
    ) -> Result<Session> {
        let (id, title, workspace) = (id.to_owned(), title.to_owned(), workspace.to_owned());
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO sessions (id,title,workspace,project_id,created_at,updated_at,control) VALUES (?1,?2,?3,NULL,?4,?4,1)",
                params![id, title, workspace, now],
            )
            .map_err(|e| db_err("create control session", e))?;
            conn.query_row(
                "SELECT id,title,workspace,project_id,model,created_at,updated_at FROM sessions WHERE id=?1",
                params![id],
                map_session,
            )
            .map_err(|e| db_err("read control session", e))
        })
        .await
    }
    async fn list_control_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.read(|conn| summarize_sessions(conn, SessionScope::Control))
            .await
    }
    async fn detach_control_session(&self, id: &str, workspace: &str) -> Result<()> {
        let (id, workspace) = (id.to_owned(), workspace.to_owned());
        self.with(move |conn| {
            conn.execute(
                "UPDATE sessions SET project_id=NULL, workspace=?2, updated_at=?3 WHERE id=?1 AND control=1",
                params![id, workspace, now_ms()],
            )
            .map_err(|e| db_err("detach control session", e))?;
            Ok(())
        })
        .await
    }
    async fn is_control_session(&self, id: &str) -> Result<bool> {
        let id = id.to_owned();
        self.read(move |conn| {
            conn.query_row(
                "SELECT COALESCE((SELECT control FROM sessions WHERE id=?1), 0)",
                params![id],
                |r| r.get::<_, i64>(0).map(|value| value == 1),
            )
            .map_err(|e| db_err("control chat lookup", e))
        })
        .await
    }
    async fn reserve_control_dispatch(
        &self,
        request_id: &str,
        session_id: &str,
        project_id: &str,
        title: &str,
        workspace: &str,
        prompt: &str,
    ) -> Result<(String, bool)> {
        let (request_id, session_id, project_id, title, workspace, prompt) = (
            request_id.to_owned(),
            session_id.to_owned(),
            project_id.to_owned(),
            title.to_owned(),
            workspace.to_owned(),
            prompt.to_owned(),
        );
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("control dispatch transaction",e))?;
            let old: Option<(String,String,String,String,String)> = tx.query_row("SELECT session_id,project_id,title,workspace,prompt FROM control_dispatches WHERE request_id=?1",params![request_id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional().map_err(|e| db_err("control dispatch lookup",e))?;
            if let Some((id,p,t,w,body)) = old {
                if p != project_id || t != title || w != workspace || body != prompt { return Err(Error::BadRequest("request_id already used with different input".into())); }
                return Ok((id,false));
            }
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1 AND path=?2)",params![project_id,workspace],|r| r.get(0)).map_err(|e| db_err("control project lookup",e))?;
            if !exists { return Err(Error::BadRequest("target project changed or no longer exists".into())); }
            let now = now_ms();
            tx.execute("INSERT INTO sessions (id,title,workspace,project_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)",params![session_id,title,workspace,project_id,now]).map_err(|e| db_err("create control dispatch session",e))?;
            tx.execute("INSERT INTO control_dispatches VALUES (?1,?2,?3,?4,?5,?6)",params![request_id,session_id,project_id,title,workspace,prompt]).map_err(|e| db_err("reserve control dispatch",e))?;
            tx.commit().map_err(|e| db_err("commit control dispatch",e))?;
            Ok((session_id,true))
        }).await
    }

    async fn create_session(
        &self,
        id: &str,
        title: &str,
        workspace: &str,
        project_id: Option<&str>,
    ) -> Result<Session> {
        let id = id.to_string();
        let title = title.to_string();
        let workspace = workspace.to_string();
        let project_id = project_id.map(str::to_string);
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, project_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO NOTHING",
                params![id, title, workspace, project_id, now, now],
            )
            .map_err(|e| db_err("create_session", e))?;
            conn.query_row(
                "SELECT id, title, workspace, project_id, model, created_at, updated_at
                   FROM sessions WHERE id = ?1",
                params![id],
                map_session,
            )
            .map_err(|e| db_err("create_session read back", e))
        })
        .await
    }

    async fn recovery_sessions(
        &self,
        cutoff: i64,
        limit: usize,
        include_agents: bool,
    ) -> Result<(usize, Vec<String>)> {
        self.read(move |conn| {
            let total: usize = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                .map_err(|e| db_err("recovery session count", e))?;
            let mut stmt = conn.prepare(
                "SELECT id FROM (SELECT s.id, s.updated_at,
                    (EXISTS(SELECT 1 FROM runs r WHERE r.session_id = s.id AND r.status = 'running')
                     OR (?3 AND EXISTS(SELECT 1 FROM agents a WHERE a.session_id = s.id AND a.status IN ('running', 'paused')))) AS interrupted
                 FROM sessions s)
                 WHERE interrupted OR updated_at >= ?1
                 ORDER BY interrupted DESC, updated_at DESC, id DESC LIMIT ?2"
            ).map_err(|e| db_err("recovery sessions prepare", e))?;
            let ids = stmt.query_map(params![cutoff, i64::try_from(limit).unwrap_or(i64::MAX), include_agents], |r| r.get(0))
                .map_err(|e| db_err("recovery sessions query", e))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("recovery sessions rows", e))?;
            Ok((total, ids))
        }).await
    }

    async fn running_agent_ids(&self) -> Result<Vec<(String, String)>> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT session_id, id FROM agents WHERE status IN ('running', 'paused') ORDER BY session_id, id")
                .map_err(|e| db_err("running agents prepare", e))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| db_err("running agents query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("running agents rows", e))
        })
        .await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.read(|conn| summarize_sessions(conn, SessionScope::All))
            .await
    }

    async fn list_sessions_for_project(&self, project_id: &str) -> Result<Vec<SessionSummary>> {
        let project_id = project_id.to_string();
        self.read(move |conn| summarize_sessions(conn, SessionScope::Project(&project_id)))
            .await
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let id = id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT id, title, workspace, project_id, model, created_at, updated_at FROM sessions WHERE id = ?1")
                .map_err(|e| db_err("get_session prepare", e))?;
            let mut rows = stmt.query_map(params![id], map_session).map_err(|e| db_err("get_session query", e))?;
            match rows.next() {
                Some(row) => row.map(Some).map_err(|e| db_err("get_session row", e)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        let id = id.to_string();
        let title = title.to_string();
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, title, now],
                )
                .map_err(|e| db_err("rename_session", e))?;
            if n == 0 {
                return Err(Error::NotFound(format!("session {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn rename_session_if_title(&self, id: &str, expected: &str, title: &str) -> Result<bool> {
        let id = id.to_string();
        let expected = expected.to_string();
        let title = title.to_string();
        self.with(move |conn| {
            let updated = conn
                .execute(
                    "UPDATE sessions SET title = ?3, updated_at = ?4 WHERE id = ?1 AND title = ?2",
                    params![id, expected, title, now_ms()],
                )
                .map_err(|e| db_err("rename_session_if_title", e))?;
            Ok(updated == 1)
        })
        .await
    }

    async fn set_session_model(
        &self,
        id: &str,
        model: Option<&SessionModelSelection>,
    ) -> Result<()> {
        let id = id.to_string();
        let json = match model {
            Some(selection) => Some(
                serde_json::to_string(selection)
                    .map_err(|e| Error::Store(format!("session model selection: {e}")))?,
            ),
            None => None,
        };
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE sessions SET model = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, json, now],
                )
                .map_err(|e| db_err("set_session_model", e))?;
            if n == 0 {
                return Err(Error::NotFound(format!("session {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("delete_session begin", e))?;
            // Messages/runs/agents/approvals/claims go with it (ON DELETE CASCADE).
            tx.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map_err(|e| db_err("delete_session", e))?;
            // `checkpoint_blobs` is not cascaded: it is content-addressed, shared
            // by every checkpoint holding the same bytes, and its only link
            // points the other way (`checkpoints.content_id`). Without this sweep
            // deleting a session left every before-image of every file the
            // session touched on disk - 12.7 MB of private file contents for a
            // session the user believed they had deleted.
            sweep_orphaned_blobs(&tx).map_err(|e| db_err("delete_session blobs", e))?;
            tx.commit()
                .map_err(|e| db_err("delete_session commit", e))?;
            Ok(())
        })
        .await
    }

    async fn touch_session(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )
            .map_err(|e| db_err("touch_session", e))?;
            Ok(())
        })
        .await
    }

    async fn assign_session_project(&self, session_id: &str, project_id: &str) -> Result<()> {
        let session_id = session_id.to_string();
        let project_id = project_id.to_string();
        self.with(move |conn| {
            let now = now_ms();
            let updated = conn
                .execute(
                    "UPDATE sessions
                        SET project_id = ?2,
                            workspace = (SELECT path FROM projects WHERE id = ?2),
                            updated_at = ?3
                      WHERE id = ?1
                        AND EXISTS (SELECT 1 FROM projects WHERE id = ?2)",
                    params![session_id, project_id, now],
                )
                .map_err(|e| db_err("assign_session_project", e))?;
            if updated == 0 {
                return Err(Error::NotFound(format!(
                    "session {session_id} or project {project_id}"
                )));
            }
            Ok(())
        })
        .await
    }

    async fn create_project(&self, id: &str, name: &str, path: &str) -> Result<Project> {
        let id = id.to_string();
        let name = name.to_string();
        let path = path.to_string();
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, name, path, now, now],
            )
            .map_err(|e| db_err("create_project", e))?;
            conn.query_row(
                "SELECT p.id, p.name, p.path, p.created_at, p.updated_at,
                        (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id) AS session_count
                   FROM projects p WHERE p.id = ?1",
                params![id],
                map_project,
            )
            .map_err(|e| db_err("create_project read back", e))
        })
        .await
    }

    async fn list_projects(&self) -> Result<Vec<Project>> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT p.id, p.name, p.path, p.created_at, p.updated_at,
                            (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id) AS session_count
                       FROM projects p
                      ORDER BY p.created_at ASC, p.id ASC",
                )
                .map_err(|e| db_err("list_projects prepare", e))?;
            let rows = stmt
                .query_map([], map_project)
                .map_err(|e| db_err("list_projects query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_projects rows", e))
        })
        .await
    }

    async fn get_project(&self, id: &str) -> Result<Option<Project>> {
        let id = id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT p.id, p.name, p.path, p.created_at, p.updated_at,
                            (SELECT COUNT(*) FROM sessions s WHERE s.project_id = p.id) AS session_count
                       FROM projects p WHERE p.id = ?1",
                )
                .map_err(|e| db_err("get_project prepare", e))?;
            let mut rows = stmt
                .query_map(params![id], map_project)
                .map_err(|e| db_err("get_project query", e))?;
            match rows.next() {
                Some(row) => row.map(Some).map_err(|e| db_err("get_project row", e)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn rename_project(&self, id: &str, name: &str) -> Result<()> {
        let id = id.to_string();
        let name = name.to_string();
        self.with(move |conn| {
            let now = now_ms();
            let updated = conn
                .execute(
                    "UPDATE projects SET name = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, name, now],
                )
                .map_err(|e| db_err("rename_project", e))?;
            if updated == 0 {
                return Err(Error::NotFound(format!("project {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn delete_project(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.with(move |conn| {
            // Sessions keep their history but lose the (now dangling) project
            // reference; the route deletes them first when the caller asked for
            // a cascade.
            conn.execute(
                "UPDATE sessions SET project_id = NULL WHERE project_id = ?1",
                params![id],
            )
            .map_err(|e| db_err("delete_project detach sessions", e))?;
            conn.execute("DELETE FROM projects WHERE id = ?1", params![id])
                .map_err(|e| db_err("delete_project", e))?;
            Ok(())
        })
        .await
    }

    async fn append_message(&self, message: &NewMessage) -> Result<i64> {
        let message = message.clone();
        self.with(move |conn| {
            let now = now_ms();
            let tool_calls = if message.tool_calls.is_empty() {
                None
            } else {
                Some(
                    serde_json::to_string(&message.tool_calls)
                        .map_err(|e| Error::Store(format!("encode tool_calls: {e}")))?,
                )
            };
            // Always a JSON array, never NULL: the column is NOT NULL so a row
            // written before v5 (`'[]'`) and a new one read back identically.
            let images = serde_json::to_string(&message.images)
                .map_err(|e| Error::Store(format!("encode images: {e}")))?;
            let tx = conn.transaction().map_err(|e| db_err("append_message begin", e))?;
            tx.execute(
                "INSERT INTO messages
                    (session_id, run_id, agent_id, role, content, tool_calls, tool_call_id, name, images, reasoning_content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    message.session_id,
                    message.run_id,
                    message.agent_id,
                    message.role,
                    message.content,
                    tool_calls,
                    message.tool_call_id,
                    message.name,
                    images,
                    message.reasoning_content,
                    now
                ],
            )
            .map_err(|e| db_err("append_message", e))?;
            let id = tx.last_insert_rowid();
            // A new message is session activity: keep list_sessions ordering honest
            // even if the caller never calls touch_session.
            tx.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![message.session_id, now],
            )
            .map_err(|e| db_err("append_message touch session", e))?;
            tx.commit().map_err(|e| db_err("append_message commit", e))?;
            Ok(id)
        })
        .await
    }

    async fn list_messages(&self, session_id: &str, limit: usize) -> Result<Vec<StoredMessage>> {
        let session_id = session_id.to_string();
        let limit = limit_i64(limit);
        self.read(move |conn| {
            // Newest N by id, then re-sorted ascending: the caller gets the tail
            // of the conversation in chronological order.
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM (
                         SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                                tool_call_id, name, images, reasoning_content, rewound_at,
                                created_at
                           FROM messages
                          WHERE session_id = ?1 AND rewound_at IS NULL
                          ORDER BY id DESC LIMIT ?2
                     ) ORDER BY id ASC",
                )
                .map_err(|e| db_err("list_messages prepare", e))?;
            let rows = stmt
                .query_map(params![session_id, limit], map_stored_message)
                .map_err(|e| db_err("list_messages query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_messages rows", e))
        })
        .await
    }

    async fn list_message_page(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
        before_id: Option<i64>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<MessagePage> {
        let session_id = session_id.to_string();
        let agent_id = agent_id.map(str::to_owned);
        self.read(move |conn| {
            // Read sizes first so an excluded oversized row is never materialized.
            // Both queries use the same read transaction for a consistent snapshot.
            let tx = conn.unchecked_transaction().map_err(|e| db_err("message page begin", e))?;
            let mut stmt = tx.prepare(
                "SELECT id, length(CAST(session_id AS BLOB)) + COALESCE(length(CAST(run_id AS BLOB)),0)
                    + COALESCE(length(CAST(agent_id AS BLOB)),0) + length(CAST(role AS BLOB))
                    + length(CAST(content AS BLOB)) + COALESCE(length(CAST(tool_calls AS BLOB)),0)
                    + COALESCE(length(CAST(tool_call_id AS BLOB)),0) + COALESCE(length(CAST(name AS BLOB)),0)
                    + COALESCE(length(CAST(images AS BLOB)),0) + COALESCE(length(CAST(reasoning_content AS BLOB)),0)
                   FROM messages WHERE session_id = ?1 AND rewound_at IS NULL
                    AND (?2 IS NULL OR agent_id = ?2) AND (?3 IS NULL OR id < ?3)
                   ORDER BY id DESC LIMIT ?4"
            ).map_err(|e| db_err("message page sizes prepare", e))?;
            let mut rows = stmt.query(params![session_id, agent_id, before_id, limit_i64(limit.saturating_add(1))])
                .map_err(|e| db_err("message page sizes query", e))?;
            let mut ids = Vec::new();
            let mut bytes = 0usize;
            let mut has_more = false;
            while let Some(row) = rows.next().map_err(|e| db_err("message page sizes rows", e))? {
                let id: i64 = row.get(0).map_err(|e| db_err("message page id", e))?;
                let size: i64 = row.get(1).map_err(|e| db_err("message page size", e))?;
                let size = usize::try_from(size).unwrap_or(usize::MAX);
                if ids.len() >= limit || (!ids.is_empty() && bytes.saturating_add(size) > max_bytes) {
                    has_more = true;
                    break;
                }
                bytes = bytes.saturating_add(size);
                ids.push(id);
            }
            drop(rows);
            drop(stmt);
            let mut messages = Vec::with_capacity(ids.len());
            let mut stmt = tx.prepare(
                "SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                        tool_call_id, name, images, reasoning_content, rewound_at, created_at
                   FROM messages WHERE id = ?1"
            ).map_err(|e| db_err("message page rows prepare", e))?;
            for id in ids.into_iter().rev() {
                messages.push(stmt.query_row(params![id], map_stored_message)
                    .map_err(|e| db_err("message page row", e))?);
            }
            Ok(MessagePage { messages, has_more })
        }).await
    }

    async fn list_history_page(
        &self,
        session_id: &str,
        before_id: Option<i64>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<StoredMessage>> {
        let session_id = session_id.to_owned();
        self.read(move |conn| {
            // Calculate payload lengths in SQLite, before copying any row into Rust.
            // BLOB length counts UTF-8 bytes rather than Unicode code points.
            let mut stmt = conn.prepare(
                "WITH bounded AS (
                    SELECT id, session_id, run_id, agent_id, role,
                           CASE WHEN role = 'tool' AND length(CAST(content AS BLOB)) > ?4 / 2
                                THEN '[tool result omitted: exceeds history byte budget]' ELSE content END AS content,
                           tool_calls, tool_call_id, name, images, reasoning_content, rewound_at, created_at
                    FROM messages WHERE session_id = ?1 AND rewound_at IS NULL
                     AND agent_id IS NULL AND role != 'system' AND (?2 IS NULL OR id < ?2)
                    ORDER BY id DESC LIMIT ?3
                 )
                 SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                        tool_call_id, name, images, reasoning_content, rewound_at, created_at,
                        COALESCE(length(CAST(content AS BLOB)), 0) + COALESCE(length(CAST(tool_calls AS BLOB)), 0) +
                        COALESCE(length(CAST(images AS BLOB)), 0) + COALESCE(length(CAST(reasoning_content AS BLOB)), 0) +
                        COALESCE(length(CAST(tool_call_id AS BLOB)), 0) + COALESCE(length(CAST(name AS BLOB)), 0)
                   FROM bounded ORDER BY id DESC"
            ).map_err(|e| db_err("list_history_page prepare", e))?;
            let mut rows = stmt.query(params![session_id, before_id, limit_i64(limit), limit_i64(max_bytes)])
                .map_err(|e| db_err("list_history_page query", e))?;
            let mut out = Vec::new();
            let mut bytes = 0usize;
            while let Some(row) = rows.next().map_err(|e| db_err("list_history_page rows", e))? {
                let size: i64 = row.get(13).map_err(|e| db_err("list_history_page size", e))?;
                let size = usize::try_from(size).unwrap_or(usize::MAX);
                if !out.is_empty() && bytes.saturating_add(size) > max_bytes { break; }
                out.push(map_stored_message(row).map_err(|e| db_err("list_history_page row", e))?);
                bytes = bytes.saturating_add(size);
            }
            Ok(out)
        }).await
    }

    async fn rewind_tool_names(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
        max_bytes: usize,
    ) -> Result<Vec<String>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT length(CAST(tool_calls AS BLOB)), tool_calls FROM messages WHERE id IN (SELECT id FROM messages WHERE session_id = ?1 AND id > ?2 AND rewound_at IS NULL ORDER BY id ASC LIMIT ?3) ORDER BY id ASC")
                .map_err(|e| db_err("rewind_tool_names prepare", e))?;
            let mut rows = stmt.query(params![session_id, after_id, limit_i64(limit)]).map_err(|e| db_err("rewind_tool_names query", e))?;
            let mut remaining = max_bytes;
            let mut names = Vec::new();
            while let Some(row) = rows.next().map_err(|e| db_err("rewind_tool_names row", e))? {
                let size: Option<i64> = row.get(0).map_err(|e| db_err("rewind_tool_names length", e))?;
                let size = size.unwrap_or(0).max(0) as usize;
                if size > remaining { return Err(Error::BadRequest("rewind tool scan exceeds byte limit".into())); }
                remaining -= size;
                let json: Option<String> = row.get(1).map_err(|e| db_err("rewind_tool_names calls", e))?;
                for call in json_vec::<ToolCall>(json, "messages.tool_calls") { names.push(call.name); }
            }
            Ok(names)
        }).await
    }

    async fn list_messages_after(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let session_id = session_id.to_string();
        let limit = limit_i64(limit);
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                            tool_call_id, name, images, reasoning_content, rewound_at,
                            created_at
                       FROM messages
                      WHERE session_id = ?1 AND id > ?2 AND rewound_at IS NULL
                      ORDER BY id ASC LIMIT ?3",
                )
                .map_err(|e| db_err("list_messages_after prepare", e))?;
            let rows = stmt
                .query_map(params![session_id, after_id, limit], map_stored_message)
                .map_err(|e| db_err("list_messages_after query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_messages_after rows", e))
        })
        .await
    }

    async fn create_run(
        &self,
        id: &str,
        session_id: &str,
        model: &str,
        prompt: &str,
    ) -> Result<()> {
        let id = id.to_string();
        let session_id = session_id.to_string();
        let model = model.to_string();
        let prompt = prompt.to_string();
        let instance_id = self.instance_id.clone();
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO runs
                    (id, session_id, status, model, prompt, error, started_at, finished_at, instance_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL, ?7)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    id,
                    session_id,
                    RunStatus::Running.as_str(),
                    model,
                    prompt,
                    now,
                    instance_id
                ],
            )
            .map_err(|e| db_err("create_run", e))?;
            Ok(())
        })
        .await
    }

    async fn finish_run(&self, run_id: &str, status: RunStatus, error: Option<&str>) -> Result<()> {
        let run_id = run_id.to_string();
        let status = status.as_str().to_string();
        let error = error.map(|s| s.to_string());
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE runs SET status = ?2, error = ?3, finished_at = ?4 WHERE id = ?1",
                    params![run_id, status, error, now],
                )
                .map_err(|e| db_err("finish_run", e))?;
            if n == 0 {
                // Shutdown paths may race a deleted session; do not turn that into
                // a user-visible failure.
                tracing::warn!(run_id, "finish_run: unknown run id");
            }
            Ok(())
        })
        .await
    }

    async fn mark_interrupted_runs(&self) -> Result<usize> {
        // Scoped to *other* instances, not this one. The exclusive data-dir lock
        // means no other process can be live, so any `running` row stamped with a
        // different `instance_id` belongs to a previous incarnation that died —
        // exactly what recovery is for. Filtering on `instance_id = ?` instead
        // would match nothing after a restart (the id is regenerated per process)
        // and silently disable crash recovery. Excluding our own id keeps a live
        // run in this process safe if recovery is ever called mid-flight.
        let instance_id = self.instance_id.clone();
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE runs SET status = ?1, finished_at = ?2
                      WHERE status = 'running' AND instance_id != ?3",
                    params![RunStatus::Interrupted.as_str(), now, instance_id],
                )
                .map_err(|e| db_err("mark_interrupted_runs", e))?;
            Ok(n)
        })
        .await
    }

    async fn list_runs(&self, session_id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        let session_id = session_id.to_string();
        let limit = limit_i64(limit);
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, status, model, prompt, error, started_at, finished_at
                       FROM runs WHERE session_id = ?1
                      ORDER BY started_at DESC, id DESC LIMIT ?2",
                )
                .map_err(|e| db_err("list_runs prepare", e))?;
            let rows = stmt
                .query_map(params![session_id, limit], map_run)
                .map_err(|e| db_err("list_runs query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_runs rows", e))
        })
        .await
    }

    // ---- rewind ----------------------------------------------------------

    async fn rewind_point(&self, session_id: &str, message_id: i64) -> Result<RewindPoint> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let (row_session, role, content): (String, String, String) = conn
                .query_row(
                    "SELECT session_id, role, content FROM messages WHERE id = ?1",
                    params![message_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        Error::NotFound(format!("no message with id {message_id}"))
                    }
                    other => db_err("rewind_point lookup", other),
                })?;
            if row_session != session_id {
                // A message id from another session must not be a way to rewind
                // this one: ids are global, so this is a real mistake, not a
                // 404-worthy guess.
                return Err(Error::NotFound(format!(
                    "message {message_id} does not belong to session {session_id}"
                )));
            }

            // Normalise to the user turn. An assistant message, a tool result or
            // a steering note belongs to the prompt that started it, and rewinding
            // to "some point inside a turn" would leave the model a transcript
            // starting with a tool result.
            let (user_id, prompt) = match role.as_str() {
                "user" => (message_id, content),
                _ => conn
                    .query_row(
                        "SELECT id, content FROM messages
                          WHERE session_id = ?1 AND id < ?2 AND role = 'user'
                          ORDER BY id DESC LIMIT 1",
                        params![session_id, message_id],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => Error::BadRequest(format!(
                            "message {message_id} has no user message before it; there is nothing to rewind to"
                        )),
                        other => db_err("rewind_point user lookup", other),
                    })?,
            };

            // The next message after the chosen prompt is where the rewind cuts.
            // `keep_through` is that id minus one, expressed as the id of the
            // prompt itself: it *survives*, because the user wants to rewrite it
            // or have it retried. `before_prompt` is the id of the message in
            // front of it, for the two modes that replace the prompt.
            let before_prompt: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM messages
                      WHERE session_id = ?1 AND id < ?2",
                    params![session_id, user_id],
                    |row| row.get(0),
                )
                .map_err(|e| db_err("rewind_point before", e))?;
            let messages_removed: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM messages
                      WHERE session_id = ?1 AND id > ?2 AND rewound_at IS NULL",
                    params![session_id, user_id],
                    |row| row.get(0),
                )
                .map_err(|e| db_err("rewind_point count", e))?;

            Ok(RewindPoint {
                session_id: session_id.clone(),
                user_message_id: user_id,
                prompt,
                keep_through: user_id,
                before_prompt,
                messages_removed: messages_removed.max(0) as usize,
            })
        })
        .await
    }

    async fn mark_messages_rewound(&self, session_id: &str, keep_through: i64) -> Result<usize> {
        let session_id = session_id.to_string();
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("mark_messages_rewound begin", e))?;
            let now = now_ms();
            // Only rows that are still visible are counted, so re-running the
            // same rewind reports 0 instead of pretending it hid the transcript
            // a second time.
            let hidden = tx
                .execute(
                    "UPDATE messages SET rewound_at = ?3
                      WHERE session_id = ?1 AND id > ?2 AND rewound_at IS NULL",
                    params![session_id, keep_through, now],
                )
                .map_err(|e| db_err("mark_messages_rewound", e))?;
            tx.commit()
                .map_err(|e| db_err("mark_messages_rewound commit", e))?;
            Ok(hidden)
        })
        .await
    }

    async fn record_checkpoint(&self, checkpoint: &FileCheckpoint) -> Result<i64> {
        let checkpoint = checkpoint.clone();
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("record_checkpoint begin", e))?;
            // Deduplicate the payload: an edit that changes one line of a 40 KB
            // file stores that file once per edit without this.
            let content_id: Option<i64> =
                if checkpoint.capture_failed || checkpoint.content.is_empty() {
                    None
                } else {
                    let sha = hex_digest(checkpoint.content.as_bytes());
                    tx.execute(
                        "INSERT OR IGNORE INTO checkpoint_blobs (sha, data) VALUES (?1, ?2)",
                        params![sha, checkpoint.content.as_bytes()],
                    )
                    .map_err(|e| db_err("record_checkpoint blob", e))?;
                    Some(
                        tx.query_row(
                            "SELECT id FROM checkpoint_blobs WHERE sha = ?1",
                            params![sha],
                            |row| row.get(0),
                        )
                        .map_err(|e| db_err("record_checkpoint blob id", e))?,
                    )
                };
            // An empty before-image is ambiguous: it can mean "the file was
            // empty" or "there is no payload". A blob row is the marker, so an
            // empty file is stored as a zero-byte blob rather than as NULL.
            let content_id = match content_id {
                Some(id) => Some(id),
                None if checkpoint.existed && !checkpoint.capture_failed => {
                    let sha = hex_digest(b"");
                    tx.execute(
                        "INSERT OR IGNORE INTO checkpoint_blobs (sha, data) VALUES (?1, x'')",
                        params![sha],
                    )
                    .map_err(|e| db_err("record_checkpoint empty blob", e))?;
                    Some(
                        tx.query_row(
                            "SELECT id FROM checkpoint_blobs WHERE sha = ?1",
                            params![sha],
                            |row| row.get(0),
                        )
                        .map_err(|e| db_err("record_checkpoint empty blob id", e))?,
                    )
                }
                None => None,
            };

            tx.execute(
                "INSERT INTO checkpoints
                    (session_id, run_id, agent_id, call_id, tool, path, existed,
                     content_id, capture_failed, message_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    checkpoint.session_id,
                    checkpoint.run_id,
                    checkpoint.agent_id,
                    checkpoint.call_id,
                    checkpoint.tool,
                    checkpoint.path,
                    i64::from(checkpoint.existed),
                    content_id,
                    i64::from(checkpoint.capture_failed),
                    checkpoint.message_id,
                    now_ms()
                ],
            )
            .map_err(|e| db_err("record_checkpoint", e))?;
            let id = tx.last_insert_rowid();
            tx.commit()
                .map_err(|e| db_err("record_checkpoint commit", e))?;
            Ok(id)
        })
        .await
    }

    async fn list_checkpoints(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<FileCheckpoint>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.id, c.session_id, c.run_id, c.agent_id, c.call_id, c.tool,
                            c.path, c.existed, c.capture_failed, c.message_id, c.created_at,
                            CAST(b.data AS TEXT) AS content
                       FROM checkpoints c
                       LEFT JOIN checkpoint_blobs b ON b.id = c.content_id
                      WHERE c.session_id = ?1 AND c.message_id > ?2
                      ORDER BY c.id ASC",
                )
                .map_err(|e| db_err("list_checkpoints prepare", e))?;
            let rows = stmt
                .query_map(params![session_id, after_id], |row| {
                    Ok(FileCheckpoint {
                        id: row.get("id")?,
                        session_id: row.get("session_id")?,
                        run_id: row.get("run_id")?,
                        agent_id: row.get("agent_id")?,
                        call_id: row.get("call_id")?,
                        tool: row.get("tool")?,
                        path: row.get("path")?,
                        existed: row.get::<_, i64>("existed")? != 0,
                        content: row.get::<_, Option<String>>("content")?.unwrap_or_default(),
                        capture_failed: row.get::<_, i64>("capture_failed")? != 0,
                        message_id: row.get("message_id")?,
                        created_at: row.get("created_at")?,
                    })
                })
                .map_err(|e| db_err("list_checkpoints query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_checkpoints rows", e))
        })
        .await
    }

    async fn list_checkpoint_metadata(
        &self,
        session_id: &str,
        after_id: i64,
    ) -> Result<Vec<FileCheckpoint>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, run_id, agent_id, call_id, tool, path, existed,
                        capture_failed, message_id, created_at FROM checkpoints
                 WHERE session_id = ?1 AND message_id > ?2 ORDER BY id ASC",
                )
                .map_err(|e| db_err("list_checkpoint_metadata prepare", e))?;
            let rows = stmt
                .query_map(params![session_id, after_id], |row| {
                    Ok(FileCheckpoint {
                        id: row.get("id")?,
                        session_id: row.get("session_id")?,
                        run_id: row.get("run_id")?,
                        agent_id: row.get("agent_id")?,
                        call_id: row.get("call_id")?,
                        tool: row.get("tool")?,
                        path: row.get("path")?,
                        existed: row.get::<_, i64>("existed")? != 0,
                        content: String::new(),
                        capture_failed: row.get::<_, i64>("capture_failed")? != 0,
                        message_id: row.get("message_id")?,
                        created_at: row.get("created_at")?,
                    })
                })
                .map_err(|e| db_err("list_checkpoint_metadata query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_checkpoint_metadata rows", e))
        })
        .await
    }

    async fn checkpoint_content_bounded(
        &self,
        id: i64,
        max_bytes: usize,
    ) -> Result<Option<String>> {
        self.read(move |conn| {
            let mut stmt = conn.prepare("SELECT length(b.data), CAST(b.data AS TEXT) FROM checkpoints c JOIN checkpoint_blobs b ON b.id = c.content_id WHERE c.id = ?1")
                .map_err(|e| db_err("checkpoint_content_bounded prepare", e))?;
            let mut rows = stmt.query(params![id]).map_err(|e| db_err("checkpoint_content_bounded query", e))?;
            let Some(row) = rows.next().map_err(|e| db_err("checkpoint_content_bounded row", e))? else { return Ok(None); };
            let size: i64 = row.get(0).map_err(|e| db_err("checkpoint_content_bounded length", e))?;
            if size < 0 || size as u64 > max_bytes as u64 { return Err(Error::BadRequest("rewind preview before-images exceed byte limit".into())); }
            row.get(1).map(Some).map_err(|e| db_err("checkpoint_content_bounded content", e))
        }).await
    }

    async fn checkpoint_content(&self, id: i64) -> Result<Option<String>> {
        self.read(move |conn| {
            conn.query_row(
                "SELECT CAST(b.data AS TEXT) FROM checkpoints c JOIN checkpoint_blobs b ON b.id = c.content_id WHERE c.id = ?1",
                params![id], |row| row.get(0),
            ).optional().map_err(|e| db_err("checkpoint_content", e))
        }).await
    }

    async fn delete_checkpoint(&self, id: i64) -> Result<()> {
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("delete_checkpoint begin", e))?;
            // Remember the payload before dropping the row so an unreferenced
            // blob can go with it; a journal whose blobs outlive it grows
            // without bound on a fresh database.
            let content_id: Option<i64> = tx
                .query_row(
                    "SELECT content_id FROM checkpoints WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap_or(None);
            tx.execute("DELETE FROM checkpoints WHERE id = ?1", params![id])
                .map_err(|e| db_err("delete_checkpoint", e))?;
            if let Some(content_id) = content_id {
                tx.execute(
                    "DELETE FROM checkpoint_blobs
                      WHERE id = ?1
                        AND NOT EXISTS (SELECT 1 FROM checkpoints WHERE content_id = ?1)",
                    params![content_id],
                )
                .map_err(|e| db_err("delete_checkpoint blob", e))?;
            }
            tx.commit()
                .map_err(|e| db_err("delete_checkpoint commit", e))?;
            Ok(())
        })
        .await
    }

    async fn set_checkpoint_message_id(&self, id: i64, message_id: i64) -> Result<()> {
        self.with(move |conn| {
            conn.execute(
                "UPDATE checkpoints SET message_id = ?2 WHERE id = ?1 AND message_id = 0",
                params![id, message_id],
            )
            .map_err(|e| db_err("set_checkpoint_message_id", e))?;
            Ok(())
        })
        .await
    }

    async fn bind_checkpoints(&self, ids: &[i64], message_id: i64) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        if message_id <= 0 {
            return Err(Error::BadRequest(
                "a checkpoint must bind to a positive message id".into(),
            ));
        }
        let ids = ids.to_vec();
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("bind_checkpoints begin", e))?;
            let placeholders = (2..=ids.len() + 1)
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "UPDATE checkpoints SET message_id = ?1 WHERE message_id = 0 AND id IN ({placeholders})"
            );
            let mut values = Vec::with_capacity(ids.len() + 1);
            values.push(message_id);
            values.extend(ids);
            let bound = tx
                .execute(&sql, rusqlite::params_from_iter(values))
                .map_err(|e| db_err("bind_checkpoints", e))?;
            tx.commit()
                .map_err(|e| db_err("bind_checkpoints commit", e))?;
            Ok(bound)
        })
        .await
    }

    async fn last_rewind_at(&self, session_id: &str) -> Result<Option<i64>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            conn.query_row(
                "SELECT MAX(rewound_at) FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|e| db_err("last_rewind_at", e))
        })
        .await
    }

    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<()> {
        let agent = agent.clone();
        let instance_id = self.instance_id.clone();
        self.with(move |conn| {
            let now = now_ms();
            let owned_paths = serde_json::to_string(&agent.owned_paths)
                .map_err(|e| Error::Store(format!("encode owned_paths: {e}")))?;
            let updated_at = if agent.updated_at > 0 {
                agent.updated_at
            } else {
                now
            };
            let created_at = if agent.created_at > 0 {
                agent.created_at
            } else {
                now
            };
            conn.execute(
                "INSERT INTO agents
                    (id, session_id, parent_run_id, parent_agent_id, name, role, task, status,
                     model, iterations, summary, error, owned_paths, created_at, updated_at,
                     finished_at, instance_id, background, note, progress)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                        ?18, ?19, ?20)
                 ON CONFLICT(id) DO UPDATE SET
                     session_id      = excluded.session_id,
                     parent_run_id   = excluded.parent_run_id,
                     parent_agent_id = excluded.parent_agent_id,
                     name            = excluded.name,
                     role            = excluded.role,
                     task            = excluded.task,
                     status          = excluded.status,
                     model           = excluded.model,
                     iterations      = excluded.iterations,
                     summary         = excluded.summary,
                     error           = excluded.error,
                     owned_paths     = excluded.owned_paths,
                     updated_at      = excluded.updated_at,
                     finished_at     = excluded.finished_at,
                     instance_id     = excluded.instance_id,
                     background      = excluded.background,
                     note            = excluded.note,
                     progress        = excluded.progress",
                params![
                    agent.id,
                    agent.session_id,
                    agent.parent_run_id,
                    agent.parent_agent_id,
                    agent.name,
                    agent.role,
                    agent.task,
                    agent.status,
                    agent.model,
                    i64::from(agent.iterations),
                    agent.summary,
                    agent.error,
                    owned_paths,
                    created_at,
                    updated_at,
                    agent.finished_at,
                    instance_id,
                    i64::from(agent.background),
                    agent.note,
                    agent.progress.map(f64::from)
                ],
            )
            .map_err(|e| db_err("upsert_agent", e))?;
            Ok(())
        })
        .await
    }

    async fn transition_agent_status(
        &self,
        id: &str,
        expected: &[&str],
        next: &str,
        updated_at: i64,
    ) -> Result<bool> {
        if expected.is_empty() {
            return Ok(false);
        }
        let id = id.to_string();
        let expected = expected
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>();
        let next = next.to_string();
        self.with(move |conn| {
            let placeholders = (4..=expected.len() + 3)
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "UPDATE agents SET status = ?2, updated_at = ?3 WHERE id = ?1 AND status IN ({placeholders})"
            );
            let mut values: Vec<&dyn rusqlite::ToSql> = vec![&id, &next, &updated_at];
            values.extend(expected.iter().map(|status| status as &dyn rusqlite::ToSql));
            let updated = conn
                .execute(&sql, values.as_slice())
                .map_err(|e| db_err("transition_agent_status", e))?;
            Ok(updated == 1)
        })
        .await
    }

    async fn list_agents(&self, session_id: &str) -> Result<Vec<AgentRecord>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, parent_run_id, parent_agent_id, name, role, task,
                            status, model, iterations, summary, error, owned_paths, created_at,
                            updated_at, finished_at, background, note, progress
                       FROM agents WHERE session_id = ?1
                      ORDER BY created_at ASC, id ASC",
                )
                .map_err(|e| db_err("list_agents prepare", e))?;
            let rows = stmt
                .query_map(params![session_id], map_agent)
                .map_err(|e| db_err("list_agents query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_agents rows", e))
        })
        .await
    }

    async fn list_agents_all(&self, project_id: Option<&str>) -> Result<Vec<AgentSummary>> {
        let project_id = project_id.map(str::to_string);
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT a.id, a.session_id, a.parent_run_id, a.parent_agent_id, a.name,
                            a.role, a.task, a.status, a.model, a.iterations, a.summary, a.error,
                            a.owned_paths, a.created_at, a.updated_at, a.finished_at,
                            a.background, a.note, a.progress,
                            s.title AS session_title, s.workspace AS workspace,
                            s.project_id AS project_id
                       FROM agents a JOIN sessions s ON s.id = a.session_id
                      WHERE (?1 IS NULL OR s.project_id = ?1)
                      ORDER BY a.created_at DESC, a.id DESC",
                )
                .map_err(|e| db_err("list_agents_all prepare", e))?;
            let rows = stmt
                .query_map(params![project_id], map_agent_summary)
                .map_err(|e| db_err("list_agents_all query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_agents_all rows", e))
        })
        .await
    }

    async fn list_agents_filtered(
        &self,
        project_id: Option<&str>,
        session_id: Option<&str>,
        statuses: &[String],
    ) -> Result<Vec<AgentSummary>> {
        let project_id = project_id.map(str::to_string);
        let session_id = session_id.map(str::to_string);
        let statuses = statuses.to_vec();
        self.read(move |conn| {
            let mut values = Vec::<String>::new();
            let mut predicates = Vec::<String>::new();
            if let Some(project_id) = project_id {
                values.push(project_id);
                predicates.push(format!("s.project_id = ?{}", values.len()));
            }
            if let Some(session_id) = session_id {
                values.push(session_id);
                predicates.push(format!("a.session_id = ?{}", values.len()));
            }
            if !statuses.is_empty() {
                let placeholders = statuses
                    .iter()
                    .map(|status| {
                        values.push(status.clone());
                        format!("?{}", values.len())
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                predicates.push(format!("a.status IN ({placeholders})"));
            }
            let predicate = if predicates.is_empty() {
                String::new()
            } else {
                format!("WHERE {}", predicates.join(" AND "))
            };
            let sql = format!(
                "SELECT a.id, a.session_id, a.parent_run_id, a.parent_agent_id, a.name,
                        a.role, a.task, a.status, a.model, a.iterations, a.summary, a.error,
                        a.owned_paths, a.created_at, a.updated_at, a.finished_at,
                        a.background, a.note, a.progress,
                        s.title AS session_title, s.workspace AS workspace,
                        s.project_id AS project_id
                   FROM agents a JOIN sessions s ON s.id = a.session_id
                   {predicate}
                  ORDER BY a.created_at DESC, a.id DESC"
            );
            let params = values
                .iter()
                .map(|value| value as &dyn rusqlite::ToSql)
                .collect::<Vec<_>>();
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| db_err("list_agents_filtered prepare", e))?;
            let rows = stmt
                .query_map(params.as_slice(), map_agent_summary)
                .map_err(|e| db_err("list_agents_filtered query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_agents_filtered rows", e))
        })
        .await
    }

    async fn get_agent(&self, id: &str) -> Result<Option<AgentSummary>> {
        let id = id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT a.id, a.session_id, a.parent_run_id, a.parent_agent_id, a.name,
                            a.role, a.task, a.status, a.model, a.iterations, a.summary, a.error,
                            a.owned_paths, a.created_at, a.updated_at, a.finished_at,
                            a.background, a.note, a.progress,
                            s.title AS session_title, s.workspace AS workspace,
                            s.project_id AS project_id
                       FROM agents a JOIN sessions s ON s.id = a.session_id
                      WHERE a.id = ?1",
                )
                .map_err(|e| db_err("get_agent prepare", e))?;
            let mut rows = stmt
                .query_map(params![id], map_agent_summary)
                .map_err(|e| db_err("get_agent query", e))?;
            match rows.next() {
                Some(row) => Ok(Some(row.map_err(|e| db_err("get_agent row", e))?)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_agent_messages(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let agent_id = agent_id.to_string();
        let limit = limit_i64(limit);
        self.read(move |conn| {
            // Same tail-then-sort shape as `list_messages`, filtered to one agent.
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM (
                         SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                                tool_call_id, name, images, reasoning_content, rewound_at,
                                created_at
                           FROM messages WHERE agent_id = ?1
                          ORDER BY id DESC LIMIT ?2
                     ) ORDER BY id ASC",
                )
                .map_err(|e| db_err("list_agent_messages prepare", e))?;
            let rows = stmt
                .query_map(params![agent_id, limit], map_stored_message)
                .map_err(|e| db_err("list_agent_messages query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_agent_messages rows", e))
        })
        .await
    }

    async fn mark_interrupted_agents(&self) -> Result<usize> {
        // Scoped to *other* instances, like `mark_interrupted_runs`: the
        // data-dir lock guarantees no peer is live, so a `running` agent from a
        // different instance is a leftover from a process that died.
        let instance_id = self.instance_id.clone();
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE agents SET status = ?1, updated_at = ?2, finished_at = ?2
                      WHERE status IN ('running', 'paused') AND instance_id != ?3",
                    params![AgentStatus::Interrupted.as_str(), now, instance_id],
                )
                .map_err(|e| db_err("mark_interrupted_agents", e))?;
            Ok(n)
        })
        .await
    }

    async fn record_usage(&self, record: &UsageRecord) -> Result<()> {
        let record = record.clone();
        self.with(move |conn| {
            conn.execute(
                "INSERT INTO usage_calls
                    (session_id, run_id, agent_id, input_tokens, output_tokens, cached_tokens, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    record.session_id,
                    record.run_id,
                    record.agent_id,
                    i64::from(record.input_tokens),
                    i64::from(record.output_tokens),
                    record.cached_tokens.map(i64::from),
                    now_ms(),
                ],
            )
            .map_err(|e| db_err("record_usage", e))?;
            Ok(())
        })
        .await
    }

    async fn usage_totals(
        &self,
        session_id: &str,
    ) -> Result<BTreeMap<Option<String>, UsageTotals>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT agent_id,
                            COUNT(*)                                   AS calls,
                            COALESCE(SUM(input_tokens), 0)             AS input_tokens,
                            COALESCE(SUM(output_tokens), 0)            AS output_tokens,
                            COALESCE(SUM(cached_tokens), 0)            AS cached_tokens,
                            COALESCE(SUM(CASE WHEN cached_tokens IS NULL THEN 0 ELSE 1 END), 0)
                                                                       AS cached_known_calls
                       FROM usage_calls
                      WHERE session_id = ?1
                      GROUP BY agent_id",
                )
                .map_err(|e| db_err("usage_totals prepare", e))?;
            let rows = stmt
                .query_map(params![session_id], |row| {
                    let agent_id: Option<String> = row.get("agent_id")?;
                    Ok((
                        agent_id,
                        UsageTotals {
                            calls: u32_of(row.get::<_, i64>("calls")?),
                            input_tokens: u64_of(row.get::<_, i64>("input_tokens")?),
                            output_tokens: u64_of(row.get::<_, i64>("output_tokens")?),
                            cached_tokens: u64_of(row.get::<_, i64>("cached_tokens")?),
                            cached_known_calls: u32_of(row.get::<_, i64>("cached_known_calls")?),
                            last_input_tokens: 0,
                        },
                    ))
                })
                .map_err(|e| db_err("usage_totals query", e))?;
            let mut out: BTreeMap<Option<String>, UsageTotals> = BTreeMap::new();
            for row in rows {
                let (agent_id, totals) = row.map_err(|e| db_err("usage_totals rows", e))?;
                out.insert(agent_id, totals);
            }

            // The newest call's prompt size is how full the window is, which a
            // group sum cannot answer. One `MAX(id)` per agent, `IS` so a NULL
            // agent id matches NULL instead of being filtered out.
            let mut last = conn
                .prepare(
                    "SELECT agent_id, input_tokens FROM usage_calls
                      WHERE session_id = ?1
                        AND id IN (SELECT MAX(id) FROM usage_calls
                                    WHERE session_id = ?1 GROUP BY agent_id)",
                )
                .map_err(|e| db_err("usage_totals last prepare", e))?;
            let rows = last
                .query_map(params![session_id], |row| {
                    Ok((
                        row.get::<_, Option<String>>("agent_id")?,
                        row.get::<_, i64>("input_tokens")?,
                    ))
                })
                .map_err(|e| db_err("usage_totals last query", e))?;
            for row in rows {
                let (agent_id, input_tokens) =
                    row.map_err(|e| db_err("usage_totals last rows", e))?;
                if let Some(totals) = out.get_mut(&agent_id) {
                    totals.last_input_tokens = u32_of(input_tokens);
                }
            }
            Ok(out)
        })
        .await
    }

    async fn mission_usage(&self, mission_id: &str) -> Result<Option<UsageTotals>> {
        let mission_id = mission_id.to_owned();
        self.read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(u.input_tokens), 0),
                        COALESCE(SUM(u.output_tokens), 0), COALESCE(SUM(u.cached_tokens), 0),
                        COUNT(u.cached_tokens)
                   FROM usage_calls u JOIN mission_attempt_sessions s ON s.session_id = u.session_id
                  WHERE s.mission_id = ?1",
                [mission_id],
                |row| {
                    Ok(Some(UsageTotals {
                        calls: u32_of(row.get(0)?),
                        input_tokens: u64_of(row.get(1)?),
                        output_tokens: u64_of(row.get(2)?),
                        cached_tokens: u64_of(row.get(3)?),
                        cached_known_calls: u32_of(row.get(4)?),
                        last_input_tokens: 0,
                    }))
                },
            )
            .map_err(|e| db_err("mission_usage", e))
        })
        .await
    }

    async fn session_stats(&self, session_id: &str) -> Result<SessionStats> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stats = SessionStats::default();

            // Messages: one query for the whole table's counts by (agent-ness,
            // role), split in Rust. Two queries (main/subagent) would be two
            // snapshots and could disagree.
            let mut counts = conn
                .prepare(
                    "SELECT role, (agent_id IS NOT NULL) AS sub, COUNT(*) AS n
                       FROM messages
                      WHERE session_id = ?1 AND rewound_at IS NULL
                      GROUP BY role, sub",
                )
                .map_err(|e| db_err("session_stats messages prepare", e))?;
            let rows = counts
                .query_map(params![session_id], |row| {
                    Ok((
                        row.get::<_, String>("role")?,
                        row.get::<_, i64>("sub")? != 0,
                        row.get::<_, i64>("n")?,
                    ))
                })
                .map_err(|e| db_err("session_stats messages query", e))?;
            for row in rows {
                let (role, subagent, n) = row.map_err(|e| db_err("session_stats messages", e))?;
                let n = u64_of(n);
                stats.messages_total += n;
                if subagent {
                    stats.messages_subagent += n;
                } else {
                    stats.messages_main += n;
                    *stats.messages_main_by_role.entry(role.clone()).or_insert(0) += n;
                }
                *stats.messages_by_role.entry(role).or_insert(0) += n;
            }

            // Tool calls. `tool_calls` is a JSON array on an assistant row; JSON1
            // is compiled into the bundled SQLite. A malformed array (written by
            // an older build, or hand-edited) must not fail the whole summary, so
            // it contributes zero rather than erroring.
            //
            // The alias is `tool_name`, not `name`: `messages` has its own `name`
            // column, and a bare `GROUP BY name` binds to that column (NULL on
            // every assistant row) instead of the extracted field - which silently
            // collapses every tool into one bucket.
            let mut calls = conn
                .prepare(
                    "SELECT json_extract(call.value, '$.name') AS tool_name, COUNT(*) AS n
                       FROM messages m, json_each(m.tool_calls) AS call
                      WHERE m.session_id = ?1
                        AND m.rewound_at IS NULL
                        AND m.role = 'assistant'
                        AND json_valid(m.tool_calls)
                      GROUP BY tool_name",
                )
                .map_err(|e| db_err("session_stats tools prepare", e))?;
            let rows = calls
                .query_map(params![session_id], |row| {
                    Ok((
                        row.get::<_, Option<String>>("tool_name")?,
                        row.get::<_, i64>("n")?,
                    ))
                })
                .map_err(|e| db_err("session_stats tools query", e))?;
            for row in rows {
                let (name, n) = row.map_err(|e| db_err("session_stats tools", e))?;
                let n = u64_of(n);
                stats.tool_calls_total += n;
                // A call with no name is still a call; name it so the total and
                // the breakdown cannot disagree.
                let key = name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| "(unnamed)".to_string());
                *stats.tool_calls.entry(key).or_insert(0) += n;
            }

            // Runs. `working_ms` sums only finished runs: an in-flight run's
            // duration is still growing, and counting it would make the number
            // shrink when it ends.
            let mut runs = conn
                .prepare(
                    "SELECT status,
                            COUNT(*)                                              AS n,
                            COALESCE(SUM(MAX(0, COALESCE(finished_at, started_at) - started_at)
                                         * (finished_at IS NOT NULL)), 0)          AS ms
                       FROM runs
                      WHERE session_id = ?1
                      GROUP BY status",
                )
                .map_err(|e| db_err("session_stats runs prepare", e))?;
            let rows = runs
                .query_map(params![session_id], |row| {
                    Ok((
                        row.get::<_, String>("status")?,
                        row.get::<_, i64>("n")?,
                        row.get::<_, i64>("ms")?,
                    ))
                })
                .map_err(|e| db_err("session_stats runs query", e))?;
            for row in rows {
                let (status, n, ms) = row.map_err(|e| db_err("session_stats runs", e))?;
                stats.runs_total += u64_of(n);
                stats.working_ms += u64_of(ms);
                *stats.runs_by_status.entry(status).or_insert(0) += u64_of(n);
            }

            Ok(stats)
        })
        .await
    }

    async fn agent_message_counts(&self, session_id: &str) -> Result<BTreeMap<String, u64>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT agent_id, COUNT(*) AS n
                       FROM messages
                      WHERE session_id = ?1 AND agent_id IS NOT NULL AND rewound_at IS NULL
                      GROUP BY agent_id",
                )
                .map_err(|e| db_err("agent_message_counts prepare", e))?;
            let rows = stmt
                .query_map(params![session_id], |row| {
                    Ok((row.get::<_, String>("agent_id")?, row.get::<_, i64>("n")?))
                })
                .map_err(|e| db_err("agent_message_counts query", e))?;
            let mut out = BTreeMap::new();
            for row in rows {
                let (agent_id, n) = row.map_err(|e| db_err("agent_message_counts rows", e))?;
                out.insert(agent_id, u64_of(n));
            }
            Ok(out)
        })
        .await
    }

    async fn create_approval(&self, record: &ApprovalRecord) -> Result<()> {
        let record = record.clone();
        self.with(move |conn| {
            let created_at = if record.created_at > 0 {
                record.created_at
            } else {
                now_ms()
            };
            let payload = serde_json::to_string(&record.payload)
                .map_err(|e| Error::Store(format!("encode approval payload: {e}")))?;
            conn.execute(
                "INSERT INTO approvals
                    (id, session_id, run_id, agent_id, kind, title, detail, payload, status,
                     decision, created_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO NOTHING",
                params![
                    record.id,
                    record.session_id,
                    record.run_id,
                    record.agent_id,
                    record.kind,
                    record.title,
                    record.detail,
                    payload,
                    record.status,
                    record.decision,
                    created_at,
                    record.resolved_at
                ],
            )
            .map_err(|e| db_err("create_approval", e))?;
            Ok(())
        })
        .await
    }

    async fn resolve_approval(&self, id: &str, decision: &str) -> Result<()> {
        let id = id.to_string();
        let decision = decision.to_string();
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE approvals SET status = 'resolved', decision = ?2, resolved_at = ?3
                      WHERE id = ?1 AND status = 'pending'",
                    params![id, decision, now],
                )
                .map_err(|e| db_err("resolve_approval", e))?;
            if n == 0 {
                // Unknown, already resolved or expired: all map to 404 upstream.
                return Err(Error::NotFound(format!("pending approval {id}")));
            }
            Ok(())
        })
        .await
    }

    async fn expire_pending_approvals(&self, run_id: &str, reason: &str) -> Result<usize> {
        let run_id = run_id.to_string();
        let reason = reason.to_string();
        self.with(move |conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE approvals SET status = 'expired', decision = 'expired', resolved_at = ?2
                      WHERE run_id = ?1 AND status = 'pending'",
                    params![run_id, now],
                )
                .map_err(|e| db_err("expire_pending_approvals", e))?;
            if n > 0 {
                // `detail` is kept as the human-readable request; the machine
                // reason is only logged (it can repeat for many rows).
                tracing::debug!(run_id, expired = n, reason = %reason, "expired pending approvals");
            }
            Ok(n)
        })
        .await
    }

    async fn list_pending_approvals(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<ApprovalRecord>> {
        let session_id = session_id.map(|s| s.to_string());
        self.read(move |conn| {
            let sql = match session_id {
                Some(_) => {
                    "SELECT id, session_id, run_id, agent_id, kind, title, detail, payload, status,
                            decision, created_at, resolved_at
                       FROM approvals WHERE status = 'pending' AND session_id = ?1
                      ORDER BY created_at ASC, id ASC"
                }
                None => {
                    "SELECT id, session_id, run_id, agent_id, kind, title, detail, payload, status,
                            decision, created_at, resolved_at
                       FROM approvals WHERE status = 'pending'
                      ORDER BY created_at ASC, id ASC"
                }
            };
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| db_err("list_pending_approvals prepare", e))?;
            match session_id {
                Some(session_id) => {
                    let rows = stmt
                        .query_map(params![session_id], map_approval)
                        .map_err(|e| db_err("list_pending_approvals query", e))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|e| db_err("list_pending_approvals rows", e))
                }
                None => {
                    let rows = stmt
                        .query_map([], map_approval)
                        .map_err(|e| db_err("list_pending_approvals query", e))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|e| db_err("list_pending_approvals rows", e))
                }
            }
        })
        .await
    }

    async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT value FROM settings WHERE key = ?1")
                .map_err(|e| db_err("get_setting prepare", e))?;
            let mut rows = stmt
                .query_map(params![key], |row| row.get::<_, String>(0))
                .map_err(|e| db_err("get_setting query", e))?;
            match rows.next() {
                Some(row) => row.map(Some).map_err(|e| db_err("get_setting row", e)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let key = key.to_string();
        let value = value.to_string();
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now],
            )
            .map_err(|e| db_err("set_setting", e))?;
            Ok(())
        })
        .await
    }

    async fn set_settings(&self, settings: &[(String, String)]) -> Result<()> {
        if settings.is_empty() {
            return Ok(());
        }
        let settings = settings.to_vec();
        self.with(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| db_err("set_settings begin", e))?;
            let now = now_ms();
            for (key, value) in &settings {
                tx.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    params![key, value, now],
                )
                .map_err(|e| db_err("set_settings", e))?;
            }
            tx.commit()
                .map_err(|e| db_err("set_settings commit", e))?;
            Ok(())
        })
        .await
    }

    async fn all_settings(&self) -> Result<Vec<(String, String)>> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT key, value FROM settings ORDER BY key ASC")
                .map_err(|e| db_err("all_settings prepare", e))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| db_err("all_settings query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("all_settings rows", e))
        })
        .await
    }

    async fn create_terminal(&self, terminal: &TerminalSession) -> Result<TerminalSession> {
        let row = terminal.clone();
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO terminals
                    (id, title, cwd, project_id, position, created_at, updated_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    row.id,
                    row.title,
                    row.cwd,
                    row.project_id,
                    row.position,
                    now,
                    now,
                    now
                ],
            )
            .map_err(|e| db_err("create_terminal", e))?;
            conn.query_row(
                "SELECT id, title, cwd, project_id, position, created_at, updated_at, last_used_at
                   FROM terminals WHERE id = ?1",
                params![row.id],
                map_terminal,
            )
            .map_err(|e| db_err("create_terminal read back", e))
        })
        .await
    }

    async fn list_terminals(&self) -> Result<Vec<TerminalSession>> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, title, cwd, project_id, position, created_at, updated_at, last_used_at
                       FROM terminals
                      ORDER BY position ASC, created_at ASC, id ASC",
                )
                .map_err(|e| db_err("list_terminals prepare", e))?;
            let rows = stmt
                .query_map([], map_terminal)
                .map_err(|e| db_err("list_terminals query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_terminals rows", e))
        })
        .await
    }

    async fn get_terminal(&self, id: &str) -> Result<Option<TerminalSession>> {
        let id = id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, title, cwd, project_id, position, created_at, updated_at, last_used_at
                       FROM terminals WHERE id = ?1",
                )
                .map_err(|e| db_err("get_terminal prepare", e))?;
            let mut rows = stmt
                .query_map(params![id], map_terminal)
                .map_err(|e| db_err("get_terminal query", e))?;
            match rows.next() {
                Some(row) => row.map(Some).map_err(|e| db_err("get_terminal row", e)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn rename_terminal(&self, id: &str, title: Option<&str>) -> Result<()> {
        let id = id.to_string();
        // The default title is derived, not stored twice: reading the row back
        // gives `rename_terminal` everything it needs, and a moved directory then
        // relabels itself instead of keeping a stale name.
        let lookup = id.clone();
        let rows = self
            .read(move |conn| {
                conn.query_row(
                "SELECT id, title, cwd, project_id, position, created_at, updated_at, last_used_at
                   FROM terminals WHERE id = ?1",
                params![lookup],
                map_terminal,
            )
            .optional()
            .map_err(|e| db_err("rename_terminal read", e))
            })
            .await?;
        let Some(existing) = rows else {
            return Err(Error::NotFound(format!("no terminal with id `{id}`")));
        };
        let title = match title {
            Some(title) => title.to_string(),
            None => TerminalSession::default_title(&existing.cwd, existing.position),
        };
        let id = existing.id;
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "UPDATE terminals SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, title, now],
            )
            .map_err(|e| db_err("rename_terminal", e))?;
            Ok(())
        })
        .await
    }

    async fn touch_terminal(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "UPDATE terminals SET last_used_at = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, now, now],
            )
            .map_err(|e| db_err("touch_terminal", e))?;
            Ok(())
        })
        .await
    }

    async fn delete_terminal(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.with(move |conn| {
            conn.execute("DELETE FROM terminals WHERE id = ?1", params![id])
                .map_err(|e| db_err("delete_terminal", e))?;
            Ok(())
        })
        .await
    }

    async fn terminal_ids(&self) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut stmt = conn
                .prepare("SELECT id FROM terminals ORDER BY position ASC, created_at ASC")
                .map_err(|e| db_err("terminal_ids prepare", e))?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| db_err("terminal_ids query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("terminal_ids rows", e))
        })
        .await
    }

    async fn claim_paths(
        &self,
        session_id: &str,
        agent_id: &str,
        paths: &[String],
        mode: &str,
    ) -> Result<Vec<ClaimRecord>> {
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        let mode = mode.to_string();
        let paths: Vec<String> = paths
            .iter()
            .map(|p| normalize_claim_path(p))
            .filter(|p| !p.is_empty())
            .collect();
        self.with(move |conn| {
            let now = now_ms();
            let tx = conn.transaction().map_err(|e| db_err("claim_paths begin", e))?;
            {
                let mut stmt = tx
                    .prepare("INSERT OR REPLACE INTO claims (session_id, agent_id, path, mode, created_at) VALUES (?1, ?2, ?3, ?4, ?5)")
                    .map_err(|e| db_err("claim_paths prepare insert", e))?;
                for path in &paths {
                    stmt.execute(params![session_id, agent_id, path, mode, now])
                        .map_err(|e| db_err("claim_paths insert", e))?;
                }
            }
            // Advisory only: report other agents' overlapping claims, never block.
            let conflicts = {
                let mut stmt = tx
                    .prepare(
                        "SELECT path, session_id, agent_id, mode, created_at
                           FROM claims WHERE session_id = ?1 AND agent_id <> ?2
                          ORDER BY agent_id ASC, path ASC",
                    )
                    .map_err(|e| db_err("claim_paths prepare select", e))?;
                let rows = stmt
                    .query_map(params![session_id, agent_id], map_claim)
                    .map_err(|e| db_err("claim_paths query", e))?;
                let mut out = Vec::new();
                for row in rows {
                    let claim = row.map_err(|e| db_err("claim_paths row", e))?;
                    if paths.iter().any(|p| paths_overlap(p, &claim.path)) {
                        out.push(claim);
                    }
                }
                out
            };
            tx.commit().map_err(|e| db_err("claim_paths commit", e))?;
            Ok(conflicts)
        })
        .await
    }

    async fn list_claims(&self, session_id: &str) -> Result<Vec<ClaimRecord>> {
        let session_id = session_id.to_string();
        self.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT path, session_id, agent_id, mode, created_at
                       FROM claims WHERE session_id = ?1 ORDER BY agent_id ASC, path ASC",
                )
                .map_err(|e| db_err("list_claims prepare", e))?;
            let rows = stmt
                .query_map(params![session_id], map_claim)
                .map_err(|e| db_err("list_claims query", e))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| db_err("list_claims rows", e))
        })
        .await
    }

    async fn release_claims(&self, agent_id: &str) -> Result<()> {
        let agent_id = agent_id.to_string();
        self.with(move |conn| {
            conn.execute("DELETE FROM claims WHERE agent_id = ?1", params![agent_id])
                .map_err(|e| db_err("release_claims", e))?;
            Ok(())
        })
        .await
    }

    async fn prune_messages_older_than(&self, days: u64) -> Result<PruneReport> {
        if days == 0 {
            // 0 means "keep everything": a retention pass must never be a
            // surprise delete when the operator has not opted in.
            return Ok(PruneReport::default());
        }
        let cutoff = retention_cutoff(days);
        let keep = limit_i64(RETENTION_KEEP_MESSAGES);
        self.with(move |conn| {
            let tx = conn.transaction().map_err(|e| db_err("prune begin", e))?;
            // Rank every message per session by id (insertion order) and delete
            // the ones past the newest `keep` that are also older than the
            // cutoff. The ranking deliberately covers *all* messages, not just
            // the old ones: ranking only the old rows would keep the newest
            // `keep` old messages and delete the recent ones.
            let messages = tx
                .execute(
                    "DELETE FROM messages
                      WHERE created_at < ?1
                        AND id IN (
                            SELECT id FROM (
                                SELECT id,
                                       ROW_NUMBER() OVER (
                                           PARTITION BY session_id ORDER BY id DESC
                                       ) AS rn
                                  FROM messages
                            ) ranked
                            WHERE ranked.rn > ?2
                        )",
                    params![cutoff, keep],
                )
                .map_err(|e| db_err("prune messages", e))?;

            // A run/agent whose transcript is gone is dead weight. Only
            // terminal rows older than the cutoff are touched, so a run that is
            // still executing (or one that just started and has not written its
            // first message yet) is never deleted.
            let runs = tx
                .execute(
                    "DELETE FROM runs
                      WHERE status <> 'running'
                        AND started_at < ?1
                        AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.run_id = runs.id)",
                    params![cutoff],
                )
                .map_err(|e| db_err("prune runs", e))?;
            let agents = tx
                .execute(
                    "DELETE FROM agents
                      WHERE status NOT IN ('running', 'paused')
                        AND created_at < ?1
                        AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.agent_id = agents.id)",
                    params![cutoff],
                )
                .map_err(|e| db_err("prune agents", e))?;
            // `claims` only has a session FK, so an agent's path claims outlived
            // the agent row and every later subagent touching those paths was
            // warned about a dead agent - which trains the model and the operator
            // to ignore the one overlap signal `owned_paths` provides. The
            // subagent release path cannot be relied on here either: a claim
            // whose `release_claims` never ran (a crash, a killed task) is
            // exactly the kind that needs sweeping.
            let claims = tx
                .execute(
                    "DELETE FROM claims
                      WHERE NOT EXISTS (SELECT 1 FROM agents a WHERE a.id = claims.agent_id)",
                    [],
                )
                .map_err(|e| db_err("prune claims", e))?;
            // After the runs: an approval whose run was just pruned is orphaned.
            let approvals = tx
                .execute(
                    "DELETE FROM approvals
                      WHERE status <> 'pending'
                        AND created_at < ?1
                        AND NOT EXISTS (SELECT 1 FROM runs r WHERE r.id = approvals.run_id)",
                    params![cutoff],
                )
                .map_err(|e| db_err("prune approvals", e))?;
            // A rewound message is deleted by the retention window like any
            // other old row, so the journal rows that point at it must go too.
            // Sweeping here (rather than only when a rewind is undone) is what
            // keeps `checkpoints` from outliving the transcript it describes.
            //
            // An unbound row is preserved only while its run is still running.
            // Once its writer is gone there is no safe transcript position to
            // replay it at, so it is stale private state rather than an undo row.
            tx.execute(
                "DELETE FROM checkpoints
                  WHERE (message_id > 0
                    AND NOT EXISTS (SELECT 1 FROM messages m WHERE m.id = checkpoints.message_id))
                    OR (message_id = 0
                    AND NOT EXISTS (SELECT 1 FROM runs r
                                    WHERE r.id = checkpoints.run_id AND r.status = 'running'))",
                [],
            )
            .map_err(|e| db_err("prune checkpoints", e))?;
            sweep_orphaned_blobs(&tx).map_err(|e| db_err("prune checkpoint blobs", e))?;
            tx.commit().map_err(|e| db_err("prune commit", e))?;

            let report = PruneReport {
                messages,
                runs,
                agents,
                approvals,
            };
            if !report.is_empty() {
                tracing::info!(
                    days,
                    messages = report.messages,
                    runs = report.runs,
                    agents = report.agents,
                    approvals = report.approvals,
                    "pruned old rows"
                );
            }
            if claims > 0 {
                tracing::debug!(claims, "released path claims left behind by pruned agents");
            }
            Ok(report)
        })
        .await
    }

    fn retention_keep_messages(&self) -> usize {
        RETENTION_KEEP_MESSAGES
    }

    async fn vacuum(&self) -> Result<()> {
        // VACUUM cannot run inside a transaction, and `with` does not open one.
        // It rewrites the whole file, so it also converts an old database to
        // auto_vacuum=INCREMENTAL (see `configure_connection`).
        self.with(|conn| {
            conn.execute_batch("VACUUM")
                .map_err(|e| db_err("vacuum", e))
        })
        .await
    }

    async fn incremental_vacuum(&self) -> Result<()> {
        self.with(|conn| {
            conn.execute_batch("PRAGMA incremental_vacuum")
                .map_err(|e| db_err("incremental_vacuum", e))
        })
        .await
    }

    async fn db_size_bytes(&self) -> Result<u64> {
        // The database plus its WAL: the WAL is where recent writes live until
        // the next checkpoint, so reporting only the main file would understate
        // the footprint of a busy store.
        let mut total = file_size(&self.path)?;
        let wal = self.path.with_extension("db-wal");
        if let Ok(size) = std::fs::metadata(&wal) {
            total += size.len();
        }
        Ok(total)
    }

    async fn integrity_check(&self) -> Result<IntegrityReport> {
        let path = self.path.clone();
        let problems = self
            .with(|conn| {
                let mut stmt = conn
                    .prepare("PRAGMA integrity_check")
                    .map_err(|e| db_err("integrity_check prepare", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| db_err("integrity_check query", e))?;
                let mut problems = Vec::new();
                for row in rows {
                    let line = row.map_err(|e| db_err("integrity_check row", e))?;
                    if !line.eq_ignore_ascii_case("ok") {
                        problems.push(line);
                    }
                }
                Ok(problems)
            })
            .await?;
        let checked_at = now_ms();
        self.last_integrity_check
            .store(checked_at, Ordering::Relaxed);
        let ok = problems.is_empty();
        if !ok {
            // The report is still returned so a health endpoint can show
            // `ok: false`; the actionable recovery text goes to the log.
            tracing::error!("{}", corruption_error(&path, &problems));
        }
        Ok(IntegrityReport {
            ok,
            problems,
            checked_at,
        })
    }

    fn last_integrity_check(&self) -> Option<i64> {
        match self.last_integrity_check.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::store::{AgentRecord, ApprovalRecord, NewMessage, RunStatus};

    pub(super) struct TempDb {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempDb {
        pub(super) fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "karucode-store-test-{tag}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            let path = dir.join("karucode.db");
            Self { dir, path }
        }

        pub(super) async fn open(&self) -> SqliteStore {
            SqliteStore::open(&self.path).await.expect("open store")
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn new_message(session_id: &str, role: &str, content: &str) -> NewMessage {
        NewMessage {
            session_id: session_id.to_string(),
            run_id: None,
            agent_id: None,
            role: role.to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            images: Vec::new(),
            reasoning_content: None,
        }
    }

    fn agent(id: &str, session_id: &str, status: &str) -> AgentRecord {
        AgentRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            parent_run_id: "r_parent".to_string(),
            parent_agent_id: None,
            name: "reviewer".to_string(),
            role: "reviewer".to_string(),
            task: "review the diff".to_string(),
            status: status.to_string(),
            model: "test-model".to_string(),
            iterations: 2,
            summary: String::new(),
            error: None,
            owned_paths: vec!["crates/foo/**".to_string()],
            created_at: 0,
            updated_at: 0,
            finished_at: None,
            background: false,
            note: String::new(),
            progress: None,
        }
    }

    fn approval(id: &str, session_id: &str, run_id: &str) -> ApprovalRecord {
        ApprovalRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            agent_id: None,
            kind: "shell_command".to_string(),
            title: "Run cargo test".to_string(),
            detail: "cargo test".to_string(),
            payload: serde_json::json!({ "command": "cargo test" }),
            status: "pending".to_string(),
            decision: None,
            created_at: 0,
            resolved_at: None,
        }
    }

    /// Write schema versions `1..=version` into a database the way a real
    /// previous build would have: that step's `ADD COLUMN`s plus the batch.
    ///
    /// The columns are not in the batches any more (see [`ADDED_COLUMNS`]), so a
    /// fixture that only ran the batches would build a database no shipped
    /// version ever wrote. The order matches [`SqliteStore::migrate`]: columns
    /// first, because a batch may index one of them.
    fn write_schema_to(conn: &rusqlite::Connection, version: i64) {
        for step in 1..=version {
            let batch = match step {
                1 => MIGRATION_1,
                2 => MIGRATION_2,
                3 => MIGRATION_3,
                4 => MIGRATION_4,
                5 => MIGRATION_5,
                6 => MIGRATION_6,
                7 => MIGRATION_7,
                8 => MIGRATION_8,
                9 => MIGRATION_9,
                10 => MIGRATION_10,
                11 => MIGRATION_11,
                12 => MIGRATION_12,
                13 => MIGRATION_13,
                14 => MIGRATION_14,
                15 => MIGRATION_15,
                16 => MIGRATION_16,
                17 => "",
                18 => MIGRATION_18,
                19 => MIGRATION_19,
                20 => MIGRATION_20,
                21 => MIGRATION_21,
                other => panic!("no batch for v{other}"),
            };
            for (_, table, column, decl) in ADDED_COLUMNS
                .iter()
                .filter(|(introduced, ..)| *introduced == step)
            {
                add_column_if_missing(conn, table, column, decl).expect("schema column");
            }
            conn.execute_batch(batch).expect("schema batch");
        }
    }

    #[tokio::test]
    async fn mission_attempt_admission_is_atomic_and_survives_deletion() {
        let db = TempDb::new("mission-attempts");
        let store = Arc::new(db.open().await);
        store.create_project("p", "P", "/p").await.unwrap();
        store
            .create_mission("m", "p", "M", "", Some(5))
            .await
            .unwrap();
        for i in 0..10 {
            store
                .create_mission_session(
                    "m",
                    &format!("r{i}"),
                    &format!("s{i}"),
                    "p",
                    "T",
                    "/p",
                    "prompt",
                )
                .await
                .unwrap();
        }
        let mut tasks = Vec::new();
        for i in 0..10 {
            let store = Arc::clone(&store);
            tasks.push(tokio::spawn(async move {
                store.admit_provider_attempt(&format!("s{i}")).await.is_ok()
            }));
        }
        let mut admitted = 0;
        for task in tasks {
            admitted += usize::from(task.await.unwrap());
        }
        assert_eq!(admitted, 5);
        assert_eq!(
            store.mission_attempt_budget("m").await.unwrap(),
            Some((Some(5), 5))
        );
        store.delete_mission("m").await.unwrap();
        assert!(store.admit_provider_attempt("s0").await.is_err());
        assert!(store.admit_provider_attempt("ordinary").await.is_ok());
    }

    #[tokio::test]
    async fn control_sessions_are_a_set_with_independent_workspaces() {
        let db = TempDb::new("control-set");
        let store = Arc::new(db.open().await);
        store.create_project("q", "Q", "/q").await.unwrap();
        let first = store
            .create_control_session("a", "Control chat", "/control/a")
            .await
            .unwrap();
        let second = store
            .create_control_session("b", "Control chat 2", "/control/b")
            .await
            .unwrap();
        assert_eq!(first.project_id, None);
        assert_eq!(second.project_id, None);
        assert_eq!(first.workspace, "/control/a");
        assert_eq!(second.workspace, "/control/b");
        assert!(store.is_control_session("a").await.unwrap());
        assert!(store.is_control_session("b").await.unwrap());
        let listed = store.list_control_sessions().await.unwrap();
        assert_eq!(listed.len(), 2, "several control chats coexist: {listed:?}");
        assert!(listed.iter().all(|session| session.project_id.is_none()));
        // A project session is not a control session and is never listed with them.
        let ordinary = store
            .create_session("n", "Project work", "/q", Some("q"))
            .await
            .unwrap();
        assert!(!store.is_control_session(&ordinary.id).await.unwrap());
        assert_eq!(store.list_control_sessions().await.unwrap().len(), 2);
        // Deleting one control chat leaves the others alone, and the dispatch
        // ledger survives it.
        assert_eq!(
            store
                .reserve_control_dispatch("r", "dispatched", "q", "T", "/q", "body")
                .await
                .unwrap(),
            ("dispatched".into(), true)
        );
        store.delete_session("a").await.unwrap();
        assert!(!store.is_control_session("a").await.unwrap());
        assert!(store.is_control_session("b").await.unwrap());
        assert_eq!(store.list_control_sessions().await.unwrap().len(), 1);
        // A legacy project-bound chat is detached, not recreated.
        store
            .with(|conn| {
                conn.execute("UPDATE sessions SET project_id='q' WHERE id='b'", [])
                    .map_err(|e| db_err("legacy control chat", e))?;
                Ok(())
            })
            .await
            .unwrap();
        store
            .detach_control_session("b", "/control/moved")
            .await
            .unwrap();
        let detached = store.get_session("b").await.unwrap().unwrap();
        assert_eq!(detached.project_id, None);
        assert_eq!(detached.workspace, "/control/moved");
        assert!(store.is_control_session("b").await.unwrap());
        assert_eq!(
            store
                .reserve_control_dispatch("r", "duplicate", "q", "T", "/q", "body")
                .await
                .unwrap(),
            ("dispatched".into(), false)
        );
    }

    /// v19 held one control chat in a singleton table; v20 turns that row into a
    /// marked session and drops the table so more chats can exist.
    #[tokio::test]
    async fn migration_v20_turns_the_singleton_into_a_control_session() {
        let db = TempDb::new("control-migration");
        let store = Arc::new(db.open().await);
        store.create_project("p", "P", "/p").await.unwrap();
        let legacy = store
            .create_session("legacy", "Control chat", "/p", Some("p"))
            .await
            .unwrap();
        store
            .with(|conn| {
                conn.execute_batch(
                    "CREATE TABLE control_chat (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), session_id TEXT NOT NULL UNIQUE);
                     INSERT INTO control_chat VALUES (1, 'legacy');
                     PRAGMA user_version = 19;",
                )
                .map_err(|e| db_err("legacy control schema", e))?;
                Ok(())
            })
            .await
            .unwrap();
        drop(store);
        let store = db.open().await;
        assert!(store.is_control_session("legacy").await.unwrap());
        assert_eq!(
            store.list_control_sessions().await.unwrap()[0].id,
            legacy.id
        );
        let table: bool = store
            .read(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='control_chat')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| db_err("control chat table", e))
            })
            .await
            .unwrap();
        assert!(!table, "the singleton table is gone");
    }

    #[tokio::test]
    async fn mission_budget_survives_restart_project_deletion_and_missing_budget() {
        let db = TempDb::new("mission-budget-reopen");
        let store = db.open().await;
        store.create_project("p", "P", "/p").await.unwrap();
        store
            .create_mission("m", "p", "M", "", Some(2))
            .await
            .unwrap();
        store
            .create_mission_session("m", "r", "s", "p", "T", "/p", "prompt")
            .await
            .unwrap();
        store.admit_provider_attempt("s").await.unwrap();
        drop(store);
        let store = db.open().await;
        assert_eq!(
            store.mission_attempt_budget("m").await.unwrap(),
            Some((Some(2), 1))
        );
        store.delete_project("p").await.unwrap();
        store.admit_provider_attempt("s").await.unwrap();
        assert!(store.admit_provider_attempt("s").await.is_err());
        store
            .with(|conn| {
                conn.execute(
                    "DELETE FROM mission_attempt_budgets WHERE mission_id='m'",
                    [],
                )
                .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert!(store.admit_provider_attempt("s").await.is_err());
    }

    async fn count(store: &SqliteStore, sql: &str) -> i64 {
        let sql = sql.to_string();
        store
            .with(move |conn| {
                conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
                    .map_err(|e| db_err("count", e))
            })
            .await
            .expect("count")
    }

    /// `PRAGMA user_version` of an open store.
    async fn version_of(store: &SqliteStore) -> i64 {
        store
            .with(|conn| {
                conn.query_row("PRAGMA user_version", [], |row| row.get(0))
                    .map_err(|e| db_err("user_version", e))
            })
            .await
            .expect("user_version")
    }

    #[tokio::test]
    async fn mission_controls_persist_and_preserve_claimed_work() {
        let db = TempDb::new("mission-controls");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        for id in ["a", "b"] {
            store
                .create_mission_task(MissionTask {
                    id: id.into(),
                    mission_id: "m".into(),
                    project_id: "p".into(),
                    title: id.into(),
                    prompt: "work".into(),
                    dependencies: vec![],
                    artifact_ids: vec![],
                    state: "draft".into(),
                    session_id: None,
                    run_id: None,
                    error: None,
                })
                .await
                .unwrap();
        }
        assert!(store.control_mission_tasks("m", "pause").await.is_err());
        store.start_mission_tasks("m", 1).await.unwrap();
        store.claim_mission_task("m", "s1").await.unwrap().unwrap();
        store.control_mission_tasks("m", "pause").await.unwrap();
        assert_eq!(
            store.mission_schedule_state("m").await.unwrap().as_deref(),
            Some("paused")
        );
        store.start_mission_tasks("m", 1).await.unwrap();
        assert!(store
            .claim_mission_task("m", "paused-session")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            store.mission_schedule_state("m").await.unwrap().as_deref(),
            Some("paused")
        );
        drop(store);
        let store = db.open().await;
        assert_eq!(
            store.mission_schedule_state("m").await.unwrap().as_deref(),
            Some("paused")
        );
        store.control_mission_tasks("m", "resume").await.unwrap();
        store.control_mission_tasks("m", "cancel").await.unwrap();
        let tasks = store.list_mission_tasks("m").await.unwrap();
        assert_eq!(tasks[0].state, "running");
        assert_eq!(tasks[1].state, "cancelled");
        store
            .finish_mission_task("a", "completed", Some("r"), None)
            .await
            .unwrap();
        assert_eq!(
            store.list_mission_tasks("m").await.unwrap()[0].state,
            "completed"
        );
        let evidence = store.list_mission_records("m", "evidence").await.unwrap();
        assert_eq!(evidence.len(), 1);
        let observations: serde_json::Value = serde_json::from_str(&evidence[0].content).unwrap();
        assert_eq!(observations["task_state"], "completed");
        assert_eq!(observations["run_id"], "r");
        assert!(observations["persisted_run"].is_null());
        assert_eq!(observations["visible_message_counts"]["tool"], 0);
        store
            .finish_mission_task("a", "completed", Some("r"), None)
            .await
            .unwrap();
        assert_eq!(
            store
                .list_mission_records("m", "evidence")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(store.control_mission_tasks("m", "resume").await.is_err());
        assert!(store
            .claim_mission_task("m", "cancelled-session")
            .await
            .unwrap()
            .is_none());
        drop(store);
        let store = db.open().await;
        assert_eq!(
            store.mission_schedule_state("m").await.unwrap().as_deref(),
            Some("cancelled")
        );
    }

    #[tokio::test]
    async fn automatic_evidence_captures_exact_run_and_retry() {
        let db = TempDb::new("automatic-evidence");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store.create_mission("m", "p", "M", "", None).await.unwrap();
        store
            .create_mission_task(MissionTask {
                id: "t".into(),
                mission_id: "m".into(),
                project_id: "p".into(),
                title: "T".into(),
                prompt: "work".into(),
                dependencies: vec![],
                artifact_ids: vec![],
                state: "draft".into(),
                session_id: None,
                run_id: None,
                error: None,
            })
            .await
            .unwrap();
        store.start_mission_tasks("m", 1).await.unwrap();
        store.claim_mission_task("m", "s").await.unwrap().unwrap();
        store
            .create_mission_session("m", "session-key", "s", "p", "T", "/one", "work")
            .await
            .unwrap();
        store.create_run("r", "s", "model", "work").await.unwrap();
        let mut msg = new_message("s", "tool", "untrusted output");
        msg.run_id = Some("r".into());
        store.append_message(&msg).await.unwrap();
        msg.run_id = Some("other".into());
        store.append_message(&msg).await.unwrap();
        store
            .finish_run("r", RunStatus::Failed, Some("failed"))
            .await
            .unwrap();
        store
            .finish_mission_task("t", "failed", Some("r"), Some("failed"))
            .await
            .unwrap();
        let records = store.list_mission_records("m", "evidence").await.unwrap();
        let value: serde_json::Value = serde_json::from_str(&records[0].content).unwrap();
        assert_eq!(value["task_state"], "failed");
        assert_eq!(value["persisted_run"]["model"], "model");
        assert_eq!(value["persisted_run"]["error"], "failed");
        assert_eq!(value["visible_message_counts"]["tool"], 1);
        assert!(!records[0].content.contains("untrusted output"));
        store
            .retry_mission_task("m", "t", "retry", None)
            .await
            .unwrap();
        store.claim_mission_task("m", "s2").await.unwrap().unwrap();
        store
            .finish_mission_task("t", "interrupted", None, Some("not submitted"))
            .await
            .unwrap();
        let records = store.list_mission_records("m", "evidence").await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].session_id.as_deref(), Some("s2"));
        let retry: serde_json::Value = serde_json::from_str(&records[1].content).unwrap();
        assert!(retry["persisted_run"].is_null());
        assert_eq!(retry["task_state"], "interrupted");
    }

    #[tokio::test]
    async fn mission_records_survive_reopen_and_cascade() {
        let db = TempDb::new("mission-records");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        let record = MissionRecord {
            id: "a".into(),
            mission_id: "m".into(),
            request_id: "r".into(),
            kind: "artifact".into(),
            title: "Artifact".into(),
            content: "text".into(),
            task_id: None,
            session_id: None,
            artifact_id: None,
            created_at: 0,
        };
        let (created, is_new) = store.publish_mission_record(record.clone()).await.unwrap();
        assert!(is_new);
        drop(store);
        let store = db.open().await;
        let records = store.list_mission_records("m", "artifact").await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, created.id);
        assert!(!store.publish_mission_record(record).await.unwrap().1);
        store.delete_mission("m").await.unwrap();
        assert!(store
            .list_mission_records("m", "artifact")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn mission_retry_is_transactional_idempotent_and_reassigns() {
        let db = TempDb::new("mission-retry");
        let store = db.open().await;
        store.create_project("p", "P", "/p").await.unwrap();
        store.create_project("q", "Q", "/q").await.unwrap();
        store.create_mission("m", "p", "M", "", None).await.unwrap();
        for (id, dependencies) in [("a", vec![]), ("b", vec!["a".to_string()])] {
            store
                .create_mission_task(MissionTask {
                    id: id.into(),
                    mission_id: "m".into(),
                    project_id: "p".into(),
                    title: id.into(),
                    prompt: "work".into(),
                    dependencies,
                    artifact_ids: vec![],
                    state: "draft".into(),
                    session_id: None,
                    run_id: None,
                    error: None,
                })
                .await
                .unwrap();
        }
        store.start_mission_tasks("m", 1).await.unwrap();
        assert!(store
            .retry_mission_task("m", "a", "key", Some("q"))
            .await
            .is_err());
        store.claim_mission_task("m", "old").await.unwrap().unwrap();
        assert!(store
            .retry_mission_task("m", "a", "key", Some("q"))
            .await
            .is_err());
        store
            .finish_mission_task("a", "failed", Some("run"), Some("bad"))
            .await
            .unwrap();
        assert!(store
            .claim_mission_task("m", "unused")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            store.list_mission_tasks("m").await.unwrap()[1].state,
            "blocked"
        );
        store.control_mission_tasks("m", "pause").await.unwrap();
        assert!(store
            .retry_mission_task("m", "a", "key", Some("q"))
            .await
            .unwrap());
        assert!(!store
            .retry_mission_task("m", "a", "key", Some("q"))
            .await
            .unwrap());
        assert!(store
            .retry_mission_task("m", "a", "key", None)
            .await
            .is_err());
        assert!(store
            .claim_mission_task("m", "new")
            .await
            .unwrap()
            .is_none());
        let tasks = store.list_mission_tasks("m").await.unwrap();
        assert_eq!(
            (
                tasks[0].project_id.as_str(),
                tasks[0].state.as_str(),
                tasks[1].state.as_str()
            ),
            ("q", "pending", "pending")
        );
        store.control_mission_tasks("m", "resume").await.unwrap();
        let claimed = store.claim_mission_task("m", "new").await.unwrap().unwrap();
        assert_eq!(claimed.session_id.as_deref(), Some("new"));
        assert_eq!(claimed.run_id, None);
        store
            .finish_mission_task("a", "completed", Some("newrun"), None)
            .await
            .unwrap();
        assert!(!store
            .retry_mission_task("m", "a", "key", Some("q"))
            .await
            .unwrap());
        assert!(store
            .retry_mission_task("m", "a", "another", None)
            .await
            .is_err());
        assert_eq!(
            store
                .claim_mission_task("m", "child")
                .await
                .unwrap()
                .unwrap()
                .id,
            "b"
        );
    }

    #[tokio::test]
    async fn mission_record_provenance_accepts_historical_attempts_only_for_matching_task() {
        let db = TempDb::new("historical-provenance");
        let store = db.open().await;
        store.create_project("p", "P", "/p").await.unwrap();
        store.create_mission("m", "p", "M", "", None).await.unwrap();
        store
            .create_mission("other", "p", "Other", "", None)
            .await
            .unwrap();
        for id in ["a", "b"] {
            store
                .create_mission_task(MissionTask {
                    id: id.into(),
                    mission_id: "m".into(),
                    project_id: "p".into(),
                    title: id.into(),
                    prompt: "work".into(),
                    dependencies: vec![],
                    artifact_ids: vec![],
                    state: "draft".into(),
                    session_id: None,
                    run_id: None,
                    error: None,
                })
                .await
                .unwrap();
        }
        store.start_mission_tasks("m", 1).await.unwrap();
        store.claim_mission_task("m", "old").await.unwrap().unwrap();
        store
            .finish_mission_task("a", "failed", None, None)
            .await
            .unwrap();
        store
            .retry_mission_task("m", "a", "retry", None)
            .await
            .unwrap();
        store.claim_mission_task("m", "new").await.unwrap().unwrap();
        let record =
            |id: &str, mission: &str, task: Option<&str>, session: Option<&str>| MissionRecord {
                id: id.into(),
                mission_id: mission.into(),
                request_id: id.into(),
                kind: "artifact".into(),
                title: "T".into(),
                content: "C".into(),
                task_id: task.map(str::to_owned),
                session_id: session.map(str::to_owned),
                artifact_id: None,
                created_at: 0,
            };
        store
            .publish_mission_record(record("historical", "m", Some("a"), Some("old")))
            .await
            .unwrap();
        store
            .publish_mission_record(record("current", "m", Some("a"), Some("new")))
            .await
            .unwrap();
        store
            .publish_mission_record(record("session-only", "m", None, Some("old")))
            .await
            .unwrap();
        assert!(store
            .publish_mission_record(record("wrong-task", "m", Some("b"), Some("old")))
            .await
            .is_err());
        assert!(store
            .publish_mission_record(record("wrong-mission", "other", None, Some("old")))
            .await
            .is_err());
        assert!(store
            .publish_mission_record(record("unknown", "m", Some("a"), Some("unknown")))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn selected_artifact_lookup_is_ordered_scoped_and_bounded() {
        let db = TempDb::new("selected-artifact-lookup");
        let store = db.open().await;
        store.create_project("p", "P", "/p").await.unwrap();
        store.create_mission("m", "p", "M", "", None).await.unwrap();
        store
            .create_mission("other", "p", "Other", "", None)
            .await
            .unwrap();
        for (id, mission, kind, content) in [
            ("a", "m", "artifact", "a".into()),
            ("b", "m", "artifact", "b".into()),
            ("foreign", "other", "artifact", "x".into()),
            ("evidence", "m", "evidence", "x".into()),
            ("large", "m", "artifact", "x".repeat(256 * 1024)),
        ] {
            store
                .publish_mission_record(MissionRecord {
                    id: id.into(),
                    mission_id: mission.into(),
                    request_id: id.into(),
                    kind: kind.into(),
                    title: "T".into(),
                    content,
                    task_id: None,
                    session_id: None,
                    artifact_id: None,
                    created_at: 0,
                })
                .await
                .unwrap();
        }
        let ids = |values: &[&str]| values.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let selected = store
            .get_mission_artifacts("m", &ids(&["b", "a"]))
            .await
            .unwrap();
        assert_eq!(
            selected.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert!(store
            .get_mission_artifacts("m", &[])
            .await
            .unwrap()
            .is_empty());
        for bad in [
            vec!["foreign"],
            vec!["evidence"],
            vec!["missing"],
            vec!["a", "a"],
            vec!["large"],
            vec!["a"; 17],
        ] {
            assert!(store.get_mission_artifacts("m", &ids(&bad)).await.is_err());
        }
    }

    #[tokio::test]
    async fn mission_task_artifact_selection_is_scoped_bounded_and_durable() {
        let db = TempDb::new("artifact-handoff");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        store
            .create_mission("other", "p", "Other", "", None)
            .await
            .unwrap();
        let record = |id: &str, mission: &str, kind: &str, content: String| MissionRecord {
            id: id.into(),
            mission_id: mission.into(),
            request_id: id.into(),
            kind: kind.into(),
            title: "source".into(),
            content,
            task_id: None,
            session_id: None,
            artifact_id: None,
            created_at: 0,
        };
        for r in [
            record("a", "m", "artifact", "data".into()),
            record("foreign", "other", "artifact", "data".into()),
            record("evidence", "m", "evidence", "claim".into()),
            record("large", "m", "artifact", "x".repeat(256 * 1024)),
        ] {
            store.publish_mission_record(r).await.unwrap();
        }
        let task = |ids: Vec<&str>| MissionTask {
            id: "task".into(),
            mission_id: "m".into(),
            project_id: "p".into(),
            title: "Task".into(),
            prompt: "work".into(),
            dependencies: vec![],
            artifact_ids: ids.into_iter().map(str::to_owned).collect(),
            state: "draft".into(),
            session_id: None,
            run_id: None,
            error: None,
        };
        for ids in [
            vec!["foreign"],
            vec!["evidence"],
            vec!["missing"],
            vec!["a", "a"],
            vec!["large"],
            vec!["a"; 17],
        ] {
            assert!(store.create_mission_task(task(ids)).await.is_err());
        }
        store.create_mission_task(task(vec!["a"])).await.unwrap();
        drop(store);
        let store = db.open().await;
        assert_eq!(
            store.list_mission_tasks("m").await.unwrap()[0].artifact_ids,
            vec!["a"]
        );
        store.start_mission_tasks("m", 1).await.unwrap();
        assert_eq!(
            store
                .claim_mission_task("m", "sid")
                .await
                .unwrap()
                .unwrap()
                .artifact_ids,
            vec!["a"]
        );
    }

    #[tokio::test]
    async fn mission_replan_guards_claims_stale_plans_and_cycles() {
        let db = TempDb::new("mission-replan");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        for (id, deps) in [("a", vec![]), ("b", vec!["a".into()])] {
            store
                .create_mission_task(MissionTask {
                    id: id.into(),
                    mission_id: "m".into(),
                    project_id: "p".into(),
                    title: id.into(),
                    prompt: "old".into(),
                    dependencies: deps,
                    artifact_ids: vec![],
                    state: "draft".into(),
                    session_id: None,
                    run_id: None,
                    error: None,
                })
                .await
                .unwrap();
        }
        let expected = || MissionTaskPlan {
            project_id: "p".into(),
            prompt: "old".into(),
            dependencies: vec![],
        };
        assert!(store
            .replan_mission_task("m", "a", expected(), None, Some("new".into()), None)
            .await
            .is_err());
        store.start_mission_tasks("m", 1).await.unwrap();
        assert!(store
            .replan_mission_task("m", "a", expected(), None, None, Some(vec!["b".into()]))
            .await
            .is_err());
        let (edit, claim) = tokio::join!(
            store.replan_mission_task("m", "a", expected(), None, Some("new".into()), None),
            store.claim_mission_task("m", "session")
        );
        let claimed = claim.unwrap().unwrap();
        assert_eq!(claimed.prompt, if edit.is_ok() { "new" } else { "old" });
        assert!(store
            .replan_mission_task("m", "a", expected(), None, Some("late".into()), None)
            .await
            .is_err());
        let b_expected = MissionTaskPlan {
            project_id: "p".into(),
            prompt: "old".into(),
            dependencies: vec!["a".into()],
        };
        store
            .replan_mission_task(
                "m",
                "b",
                b_expected.clone(),
                None,
                Some("updated".into()),
                None,
            )
            .await
            .unwrap();
        assert!(store
            .replan_mission_task("m", "b", b_expected, None, Some("stale".into()), None)
            .await
            .is_err());
        assert_eq!(
            store.list_mission_tasks("m").await.unwrap()[1].prompt,
            "updated"
        );
    }

    #[tokio::test]
    async fn mission_revision_persists_and_serializes_with_start() {
        let db = TempDb::new("mission-revision");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store.create_project("q", "Two", "/two").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        for id in ["a", "b"] {
            store
                .create_mission_task(MissionTask {
                    id: id.into(),
                    mission_id: "m".into(),
                    project_id: "p".into(),
                    title: id.into(),
                    prompt: "old".into(),
                    dependencies: vec![],
                    artifact_ids: vec![],
                    state: "draft".into(),
                    session_id: None,
                    run_id: None,
                    error: None,
                })
                .await
                .unwrap();
        }
        store
            .revise_mission_task(
                "m",
                "a",
                Some("q".into()),
                Some("new".into()),
                Some(vec!["b".into()]),
            )
            .await
            .unwrap();
        drop(store);
        let store = db.open().await;
        let tasks = store.list_mission_tasks("m").await.unwrap();
        assert_eq!(tasks[0].prompt, "new");
        assert_eq!(tasks[0].project_id, "q");
        assert_eq!(tasks[0].dependencies, vec!["b"]);
        assert!(store
            .revise_mission_task("m", "b", None, None, Some(vec!["a".into()]))
            .await
            .is_err());
        let (edit, start) = tokio::join!(
            store.revise_mission_task("m", "a", None, Some("raced".into()), None),
            store.start_mission_tasks("m", 1)
        );
        start.unwrap();
        let tasks = store.list_mission_tasks("m").await.unwrap();
        assert_eq!(tasks[0].prompt, if edit.is_ok() { "raced" } else { "new" });
        assert_ne!(tasks[0].state, "draft");
        assert!(store
            .revise_mission_task("m", "a", None, Some("late".into()), None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn mission_tasks_enforce_dependencies_capacity_and_recovery() {
        let db = TempDb::new("mission-tasks");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        let task = |id: &str, deps: Vec<&str>| MissionTask {
            id: id.into(),
            mission_id: "m".into(),
            project_id: "p".into(),
            title: id.into(),
            prompt: "work".into(),
            dependencies: deps.into_iter().map(str::to_owned).collect(),
            artifact_ids: vec![],
            state: "draft".into(),
            session_id: None,
            run_id: None,
            error: None,
        };
        assert!(store
            .create_mission_task(task("bad", vec!["missing"]))
            .await
            .is_err());
        store.create_mission_task(task("a", vec![])).await.unwrap();
        store
            .create_mission_task(task("b", vec!["a"]))
            .await
            .unwrap();
        store
            .create_mission_task(task("c", vec!["b"]))
            .await
            .unwrap();
        store.create_mission_task(task("d", vec![])).await.unwrap();
        assert!(store.claim_mission_task("m", "s0").await.unwrap().is_none());
        store.start_mission_tasks("m", 1).await.unwrap();
        assert_eq!(
            store
                .claim_mission_task("m", "s1")
                .await
                .unwrap()
                .unwrap()
                .id,
            "a"
        );
        assert!(store.claim_mission_task("m", "s2").await.unwrap().is_none());
        store
            .finish_mission_task("a", "failed", Some("r1"), Some("failure"))
            .await
            .unwrap();
        assert_eq!(
            store
                .claim_mission_task("m", "s2")
                .await
                .unwrap()
                .unwrap()
                .id,
            "d"
        );
        let tasks = store.list_mission_tasks("m").await.unwrap();
        assert_eq!(tasks[1].state, "blocked");
        assert_eq!(tasks[2].state, "blocked");
        assert_eq!(store.recover_mission_tasks().await.unwrap(), vec!["m"]);
        assert_eq!(
            store.list_mission_tasks("m").await.unwrap()[3].state,
            "interrupted"
        );
        assert!(store.claim_mission_task("m", "s3").await.unwrap().is_none());
        assert!(store
            .create_mission_task(task("late", vec![]))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn claim_rolls_back_if_claimed_row_cannot_be_decoded() {
        let db = TempDb::new("mission-claim-rollback");
        let store = db.open().await;
        store.create_project("p", "One", "/one").await.unwrap();
        store
            .create_mission("m", "p", "Mission", "", None)
            .await
            .unwrap();
        store
            .create_mission_task(MissionTask {
                id: "a".into(),
                mission_id: "m".into(),
                project_id: "p".into(),
                title: "A".into(),
                prompt: "work".into(),
                dependencies: vec![],
                artifact_ids: vec![],
                state: "draft".into(),
                session_id: None,
                run_id: None,
                error: None,
            })
            .await
            .unwrap();
        store.start_mission_tasks("m", 1).await.unwrap();
        store.with(|conn| { conn.execute("CREATE TRIGGER corrupt_claim AFTER UPDATE OF state ON mission_tasks WHEN NEW.state='running' BEGIN UPDATE mission_tasks SET dependencies='not-json' WHERE id=NEW.id; END", []).unwrap(); Ok(()) }).await.unwrap();
        assert!(store.claim_mission_task("m", "s").await.is_err());
        let tasks = store.list_mission_tasks("m").await.unwrap();
        assert_eq!(tasks[0].state, "pending");
        assert!(tasks[0].session_id.is_none());
        store
            .with(|conn| {
                conn.execute("DROP TRIGGER corrupt_claim", []).unwrap();
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .claim_mission_task("m", "s")
                .await
                .unwrap()
                .unwrap()
                .id,
            "a"
        );
    }

    #[tokio::test]
    async fn mission_sessions_are_durable_and_retries_never_recreate() {
        let db = TempDb::new("mission-sessions");
        let store = db.open().await;
        store.create_project("p1", "One", "/one").await.unwrap();
        store.create_project("p2", "Two", "/two").await.unwrap();
        store
            .create_mission("m1", "p1", "Mission", "Description", None)
            .await
            .unwrap();
        assert_eq!(store.list_missions("p1").await.unwrap().len(), 1);
        assert!(store.list_missions("p2").await.unwrap().is_empty());
        assert!(store
            .create_mission_session("m1", "key", "s1", "p1", "Title", "/one", "prompt")
            .await
            .unwrap());
        assert!(!store
            .create_mission_session("m1", "key", "s2", "p1", "Title", "/one", "prompt")
            .await
            .unwrap());
        assert!(store
            .create_mission_session("m1", "key", "s2", "p1", "Title", "/one", "other")
            .await
            .is_err());
        assert_eq!(
            store
                .get_session("s1")
                .await
                .unwrap()
                .unwrap()
                .project_id
                .as_deref(),
            Some("p1")
        );
        store.rename_session("s1", "Changed").await.unwrap();
        assert!(!store
            .create_mission_session("m1", "key", "s2", "p1", "Title", "/one", "prompt")
            .await
            .unwrap());
        store.delete_session("s1").await.unwrap();
        assert!(!store
            .create_mission_session("m1", "key", "s2", "p1", "Title", "/one", "prompt")
            .await
            .unwrap());
        assert!(store.get_session("s2").await.unwrap().is_none());
        drop(store);
        let store = db.open().await;
        assert_eq!(store.list_mission_sessions("m1").await.unwrap().len(), 1);
        store.delete_mission("m1").await.unwrap();
        assert!(store.get_mission("m1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mission_v12_migration_preserves_deleted_session_request_identity() {
        let db = TempDb::new("mission-v12");
        std::fs::create_dir_all(db.path.parent().unwrap()).unwrap();
        {
            let conn = rusqlite::Connection::open(&db.path).unwrap();
            write_schema_to(&conn, 12);
            conn.execute_batch("INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('p1','One','/one',1,1); INSERT INTO missions VALUES ('m1','p1','Mission','',1,1); INSERT INTO mission_sessions VALUES ('m1','key','gone','Title','prompt',1); PRAGMA user_version=12;").unwrap();
        }
        let store = db.open().await;
        let links = store.list_mission_sessions("m1").await.unwrap();
        assert_eq!(links[0].project_id, "p1");
        assert!(!store
            .create_mission_session("m1", "key", "new", "p1", "Title", "/one", "prompt")
            .await
            .unwrap());
        assert!(store
            .create_mission_session("m1", "key", "new", "p2", "Title", "/two", "prompt")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn mission_pages_have_stable_project_scoped_order_and_bounds() {
        let db = TempDb::new("mission-pages");
        let store = db.open().await;
        store.create_project("p", "P", "/p").await.unwrap();
        store
            .create_project("other", "Other", "/other")
            .await
            .unwrap();
        for id in ["a", "b", "c"] {
            store.create_mission(id, "p", id, "", None).await.unwrap();
        }
        store
            .create_mission("foreign", "other", "Foreign", "", None)
            .await
            .unwrap();
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE missions SET updated_at = CASE id WHEN 'a' THEN 10 WHEN 'b' THEN 30 WHEN 'c' THEN 20 ELSE 1 END",
                    [],
                )
                .map_err(|e| db_err("order missions", e))
            })
            .await
            .unwrap();
        let (first, has_more) = store.list_mission_page("p", 0, 2).await.unwrap();
        assert!(has_more);
        assert_eq!(
            first.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        let (second, has_more) = store.list_mission_page("p", 2, 2).await.unwrap();
        assert!(!has_more);
        assert_eq!(
            second.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(
            store
                .list_mission_page("other", 0, 2)
                .await
                .unwrap()
                .0
                .len(),
            1
        );
        let (empty, has_more) = store.list_mission_page("p", 0, 0).await.unwrap();
        assert!(empty.is_empty());
        assert!(has_more);
        assert!(store
            .list_mission_page("p", 3, 2)
            .await
            .unwrap()
            .0
            .is_empty());
    }

    #[tokio::test]
    async fn create_get_rename_and_delete_session() {
        let db = TempDb::new("session-crud");
        let store = db.open().await;

        let session = store
            .create_session("s_1", "First", "/workspace", None)
            .await
            .expect("create");
        assert_eq!(session.id, "s_1");
        assert_eq!(session.title, "First");
        assert!(session.created_at > 0);
        assert_eq!(session.created_at, session.updated_at);

        // Caller-supplied id, idempotent on repeat.
        let again = store
            .create_session("s_1", "Ignored", "/other", None)
            .await
            .expect("create again");
        assert_eq!(again.title, "First");
        assert_eq!(again.workspace, "/workspace");

        assert_eq!(
            store
                .get_session("s_1")
                .await
                .expect("get")
                .expect("some")
                .title,
            "First"
        );
        assert!(store.get_session("s_missing").await.expect("get").is_none());

        // Appending a message counts as activity and bumps updated_at.
        let before = store
            .get_session("s_1")
            .await
            .expect("get")
            .expect("some")
            .updated_at;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        store
            .append_message(&new_message("s_1", "user", "hi"))
            .await
            .expect("append");
        let after = store
            .get_session("s_1")
            .await
            .expect("get")
            .expect("some")
            .updated_at;
        assert!(after > before, "append_message must touch the session");

        assert!(!store
            .rename_session_if_title("s_1", "Stale", "Must not land")
            .await
            .expect("stale rename loses"));
        assert_eq!(
            store.get_session("s_1").await.unwrap().unwrap().title,
            "First"
        );
        assert!(store
            .rename_session_if_title("s_1", "First", "Renamed")
            .await
            .expect("current rename wins"));
        assert!(!store
            .rename_session_if_title("missing", "First", "Renamed")
            .await
            .expect("missing rename loses"));
        let renamed = store.get_session("s_1").await.expect("get").expect("some");
        assert_eq!(renamed.title, "Renamed");
        assert!(renamed.updated_at >= renamed.created_at);

        assert!(matches!(
            store.rename_session("s_missing", "x").await,
            Err(Error::NotFound(_))
        ));

        store.delete_session("s_1").await.expect("delete");
        assert!(store.get_session("s_1").await.expect("get").is_none());
        // Deleting again is a no-op, not an error.
        store.delete_session("s_1").await.expect("delete again");
    }

    #[tokio::test]
    async fn session_model_selection_round_trips() {
        let db = TempDb::new("session-model");
        let store = db.open().await;

        let created = store
            .create_session("s_1", "First", "/workspace", None)
            .await
            .expect("create");
        assert!(
            created.model.is_none(),
            "a new session follows the install-wide default"
        );

        let selection = SessionModelSelection {
            provider: "gateway".into(),
            model: "model-b".into(),
            reasoning_effort: Some("high".into()),
        };
        store
            .set_session_model("s_1", Some(&selection))
            .await
            .expect("set model");

        let session = store.get_session("s_1").await.unwrap().expect("some");
        assert_eq!(session.model.as_ref(), Some(&selection));
        // The rail summary must carry the same selection, so a second tab shows
        // the right label without loading the session detail.
        let summaries = store.list_sessions().await.expect("list");
        assert_eq!(summaries[0].model.as_ref(), Some(&selection));

        // A selection without an effort stays without one.
        let plain = SessionModelSelection {
            provider: "local".into(),
            model: "tiny".into(),
            reasoning_effort: None,
        };
        store
            .set_session_model("s_1", Some(&plain))
            .await
            .expect("set plain model");
        let session = store.get_session("s_1").await.unwrap().expect("some");
        assert_eq!(session.model.as_ref(), Some(&plain));

        // Clearing restores the install-wide default.
        store
            .set_session_model("s_1", None)
            .await
            .expect("clear model");
        assert!(store
            .get_session("s_1")
            .await
            .unwrap()
            .expect("some")
            .model
            .is_none());

        // An unknown session is a NotFound, not a silent no-op.
        assert!(matches!(
            store.set_session_model("s_missing", Some(&selection)).await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn migration_from_v2_keeps_sessions_and_defaults_the_model() {
        let db = TempDb::new("migrate-v2");
        std::fs::create_dir_all(db.path.parent().expect("parent")).expect("data dir");
        {
            // A database written by the previous build: v1 + v2, no model column.
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, 2);
            conn.pragma_update(None, "user_version", 2)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at)
                 VALUES ('s_old', 'Old', '/w', 1, 2)",
                [],
            )
            .expect("insert v2 session");
        }

        let store = db.open().await;
        let session = store
            .get_session("s_old")
            .await
            .expect("get")
            .expect("the v2 session survives the upgrade");
        assert_eq!(session.title, "Old");
        assert!(
            session.model.is_none(),
            "an upgraded session follows the install-wide default"
        );
        // v2's own index must survive the replayable form of the step: the
        // column moved out of the batch, and the index that depends on it did
        // not.
        let sessions = index_names(&store, "sessions").await;
        assert!(
            sessions.iter().any(|n| n == "idx_sessions_project_id"),
            "v2 indexed sessions(project_id): {sessions:?}"
        );

        // The new column is writable after the migration.
        let selection = SessionModelSelection {
            provider: "gateway".into(),
            model: "model-b".into(),
            reasoning_effort: None,
        };
        store
            .set_session_model("s_old", Some(&selection))
            .await
            .expect("set on upgraded row");
        assert_eq!(
            store
                .get_session("s_old")
                .await
                .unwrap()
                .expect("some")
                .model
                .as_ref(),
            Some(&selection)
        );
    }

    #[tokio::test]
    async fn delete_session_cascades_to_children() {
        let db = TempDb::new("cascade");
        let store = db.open().await;

        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let msg_id = store
            .append_message(&new_message("s_1", "user", "hello"))
            .await
            .expect("message");
        store.create_run("r_1", "s_1", "m", "p").await.expect("run");
        store
            .upsert_agent(&agent("a_1", "s_1", "running"))
            .await
            .expect("agent");
        store
            .create_approval(&approval("ap_1", "s_1", "r_1"))
            .await
            .expect("approval");
        store
            .claim_paths(
                "s_1",
                "a_1",
                &["crates/foo/src/lib.rs".to_string()],
                "write",
            )
            .await
            .expect("claim");

        store.delete_session("s_1").await.expect("delete");

        assert_eq!(count(&store, "SELECT COUNT(*) FROM messages").await, 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM runs").await, 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM agents").await, 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM approvals").await, 0);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM claims").await, 0);
        assert!(msg_id > 0);
    }

    #[tokio::test]
    async fn recovery_queries_prioritize_interrupted_and_bound_results() {
        let db = TempDb::new("recovery-candidates");
        let store = db.open().await;
        for id in ["old_run", "old_agent", "old_done", "recent_a", "recent_b"] {
            store.create_session(id, id, "/w", None).await.unwrap();
        }
        store
            .create_run("run", "old_run", "model", "prompt")
            .await
            .unwrap();
        store
            .upsert_agent(&agent("running", "old_agent", "running"))
            .await
            .unwrap();
        store
            .upsert_agent(&agent("done", "old_done", "completed"))
            .await
            .unwrap();
        store.with(|conn| {
            conn.execute("UPDATE sessions SET updated_at = CASE id WHEN 'old_run' THEN 1 WHEN 'old_agent' THEN 2 WHEN 'old_done' THEN 3 ELSE 100 END", []).map_err(|e| db_err("test", e))?;
            Ok(())
        }).await.unwrap();
        assert_eq!(
            store.running_agent_ids().await.unwrap(),
            vec![("old_agent".into(), "running".into())]
        );
        let (total, ids) = store.recovery_sessions(50, 3, true).await.unwrap();
        assert_eq!(total, 5);
        assert_eq!(ids, ["old_agent", "old_run", "recent_b"]);
        assert_eq!(
            store.recovery_sessions(50, 10, false).await.unwrap().1,
            ["old_run", "recent_b", "recent_a"]
        );
        assert_eq!(
            store.recovery_sessions(50, 0, true).await.unwrap(),
            (5, vec![])
        );
        // Simulate rows left by the preceding server instance.
        store
            .with(|conn| {
                conn.execute("UPDATE runs SET instance_id = 'previous'", [])
                    .map_err(|e| db_err("test runs", e))?;
                conn.execute("UPDATE agents SET instance_id = 'previous'", [])
                    .map_err(|e| db_err("test agents", e))?;
                Ok(())
            })
            .await
            .unwrap();
        store.mark_interrupted_runs().await.unwrap();
        store.mark_interrupted_agents().await.unwrap();
        assert_eq!(
            store.recovery_sessions(50, 10, true).await.unwrap().1,
            ["recent_b", "recent_a"]
        );
    }

    #[tokio::test]
    async fn list_sessions_reports_counts_status_and_ordering() {
        let db = TempDb::new("session-summary");
        let store = db.open().await;

        store
            .create_session("s_a", "A", "/w", None)
            .await
            .expect("a");
        store
            .create_session("s_b", "B", "/w", None)
            .await
            .expect("b");

        store
            .append_message(&new_message("s_a", "user", "one"))
            .await
            .expect("m1");
        store
            .append_message(&new_message("s_a", "assistant", "two"))
            .await
            .expect("m2");
        store
            .create_run("r_1", "s_a", "model", "prompt")
            .await
            .expect("run");
        // s_b is touched last, so it sorts first (updated_at DESC).
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        store.touch_session("s_b").await.expect("touch b");

        let sessions = store.list_sessions().await.expect("list");
        assert_eq!(sessions.len(), 2);
        // Updated_at DESC: s_b was touched after creation.
        assert_eq!(sessions[0].id, "s_b");
        assert_eq!(sessions[0].message_count, 0);
        assert!(sessions[0].last_run_status.is_none());
        assert!(!sessions[0].running);

        let a = sessions.iter().find(|s| s.id == "s_a").expect("s_a");
        assert_eq!(a.message_count, 2);
        assert_eq!(a.last_run_status.as_deref(), Some("running"));
        assert!(a.running);

        store
            .finish_run("r_1", RunStatus::Completed, None)
            .await
            .expect("finish");
        let sessions = store.list_sessions().await.expect("list again");
        let a = sessions.iter().find(|s| s.id == "s_a").expect("s_a");
        assert_eq!(a.last_run_status.as_deref(), Some("completed"));
        assert!(!a.running);
    }

    #[tokio::test]
    async fn history_page_bounds_bytes_and_uses_exclusive_cursor() {
        let db = TempDb::new("history-page");
        let store = db.open().await;
        store.create_session("s", "t", "/w", None).await.unwrap();
        let old = store
            .append_message(&new_message("s", "user", "old"))
            .await
            .unwrap();
        store
            .append_message(&new_message("s", "system", &"s".repeat(10000)))
            .await
            .unwrap();
        let mut child = new_message("s", "user", &"a".repeat(10000));
        child.agent_id = Some("child".into());
        store.append_message(&child).await.unwrap();
        let middle = store
            .append_message(&new_message("s", "assistant", "éé"))
            .await
            .unwrap();
        let newest = store
            .append_message(&new_message("s", "user", "latest"))
            .await
            .unwrap();
        let page = store.list_history_page("s", None, 100, 10).await.unwrap();
        assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![newest]);
        let page = store
            .list_history_page("s", Some(newest), 100, 100)
            .await
            .unwrap();
        assert_eq!(
            page.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![middle, old]
        );
        assert!(store
            .list_history_page("s", None, 0, 100)
            .await
            .unwrap()
            .is_empty());
        let oversized = store.list_history_page("s", None, 100, 1).await.unwrap();
        assert_eq!(oversized.len(), 1);
        assert_eq!(oversized[0].id, newest);
        store.mark_messages_rewound("s", middle).await.unwrap();
        assert_eq!(
            store.list_history_page("s", None, 100, 100).await.unwrap()[0].id,
            middle
        );
    }

    #[tokio::test]
    async fn history_page_omits_oversized_tool_before_loading_payload() {
        let db = TempDb::new("history_oversized_tool");
        let store = db.open().await;
        store.create_session("s", "t", "/w", None).await.unwrap();
        store
            .append_message(&new_message("s", "user", "task"))
            .await
            .unwrap();
        let mut assistant = new_message("s", "assistant", "");
        assistant.tool_calls.push(kc_core::message::ToolCall {
            id: "call".into(),
            name: "echo".into(),
            arguments: "{}".into(),
        });
        store.append_message(&assistant).await.unwrap();
        let mut tool = new_message("s", "tool", &"é".repeat(10000));
        tool.tool_call_id = Some("call".into());
        tool.name = Some("echo".into());
        store.append_message(&tool).await.unwrap();
        let page = store.list_history_page("s", None, 100, 1000).await.unwrap();
        assert_eq!(page.len(), 3);
        assert!(page[0].content.contains("omitted"));
        assert_eq!(page[0].tool_call_id.as_deref(), Some("call"));
        assert_eq!(page[2].content, "task");
        assert_eq!(
            store
                .list_messages("s", 10)
                .await
                .unwrap()
                .last()
                .unwrap()
                .content,
            tool.content
        );
    }

    #[tokio::test]
    async fn messages_keep_insertion_order_and_limit_returns_tail() {
        let db = TempDb::new("messages");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        for i in 0..6 {
            store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
        }
        // Another session must not leak in.
        store
            .create_session("s_2", "t2", "/w", None)
            .await
            .expect("session2");
        store
            .append_message(&new_message("s_2", "user", "other"))
            .await
            .expect("other");

        let all = store.list_messages("s_1", 100).await.expect("all");
        assert_eq!(all.len(), 6);
        assert_eq!(
            all.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            vec!["m0", "m1", "m2", "m3", "m4", "m5"]
        );
        assert!(all.windows(2).all(|w| w[0].id < w[1].id));

        let tail = store.list_messages("s_1", 2).await.expect("tail");
        assert_eq!(
            tail.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            vec!["m4", "m5"]
        );
        assert!(store
            .list_messages("s_1", 0)
            .await
            .expect("zero")
            .is_empty());
    }

    #[tokio::test]
    async fn message_page_obeys_cursor_filter_budget_and_rewind() {
        let db = TempDb::new("message-page");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .create_session("s_2", "t", "/w", None)
            .await
            .expect("other session");
        let first = store
            .append_message(&new_message("s_1", "user", "first"))
            .await
            .unwrap();
        let mut agent = new_message("s_1", "assistant", "éé");
        agent.agent_id = Some("a".into());
        let second = store.append_message(&agent).await.unwrap();
        store
            .append_message(&new_message("s_2", "user", "other"))
            .await
            .unwrap();
        let third = store
            .append_message(&new_message("s_1", "user", &"x".repeat(1000)))
            .await
            .unwrap();
        let page = store
            .list_message_page("s_1", None, None, 2, usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            page.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![second, third]
        );
        assert!(page.has_more);
        let previous = store
            .list_message_page("s_1", None, Some(second), 10, usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            previous.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![first]
        );
        assert!(!previous.has_more);
        let tiny = store
            .list_message_page("s_1", None, None, 10, 1)
            .await
            .unwrap();
        assert_eq!(tiny.messages[0].id, third);
        assert_eq!(tiny.messages.len(), 1);
        assert!(tiny.has_more);
        let filtered = store
            .list_message_page("s_1", Some("a"), None, 10, usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            filtered.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![second]
        );
        assert!(!filtered.has_more);
        let empty = store
            .list_message_page("s_1", None, None, 0, 100)
            .await
            .unwrap();
        assert!(empty.messages.is_empty());
        assert!(empty.has_more);
        store.mark_messages_rewound("s_1", second).await.unwrap();
        let visible = store
            .list_message_page("s_1", None, None, 10, usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            visible.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![first, second]
        );
        assert!(!visible.has_more);
    }

    #[tokio::test]
    async fn rewind_scan_reads_only_calls_and_enforces_aggregate_budget() {
        let db = TempDb::new("rewind-scan-budget");
        let store = db.open().await;
        store.create_session("s_1", "t", "/w", None).await.unwrap();
        let mut msg = new_message("s_1", "assistant", &"x".repeat(1024 * 1024));
        msg.tool_calls = vec![ToolCall {
            id: "1".into(),
            name: "shell".into(),
            arguments: "abc".into(),
        }];
        let size = serde_json::to_string(&msg.tool_calls).unwrap().len();
        store.append_message(&msg).await.unwrap();
        store.append_message(&msg).await.unwrap();
        assert_eq!(
            store
                .rewind_tool_names("s_1", 0, 2, size * 2)
                .await
                .unwrap(),
            vec!["shell", "shell"]
        );
        assert!(store
            .rewind_tool_names("s_1", 0, 2, size * 2 - 1)
            .await
            .is_err());
        assert_eq!(
            store.rewind_tool_names("s_1", 0, 1, size).await.unwrap(),
            vec!["shell"]
        );
    }

    #[tokio::test]
    async fn messages_roundtrip_tool_calls_and_list_after() {
        let db = TempDb::new("messages-after");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        let first = store
            .append_message(&new_message("s_1", "user", "hi"))
            .await
            .expect("first");

        let mut assistant = new_message("s_1", "assistant", "calling");
        assistant.run_id = Some("r_1".to_string());
        assistant.tool_calls = vec![ToolCall {
            id: "call_1".to_string(),
            name: "read_file".to_string(),
            arguments: "{\"path\":\"a\"}".to_string(),
        }];
        store.append_message(&assistant).await.expect("assistant");

        let mut tool = new_message("s_1", "tool", "file contents");
        tool.tool_call_id = Some("call_1".to_string());
        tool.name = Some("read_file".to_string());
        let last = store.append_message(&tool).await.expect("tool");

        let after = store
            .list_messages_after("s_1", first, 10)
            .await
            .expect("after");
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].role, "assistant");
        assert_eq!(after[0].run_id.as_deref(), Some("r_1"));
        assert_eq!(after[0].tool_calls.len(), 1);
        assert_eq!(after[0].tool_calls[0].name, "read_file");
        assert_eq!(after[1].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(after[1].name.as_deref(), Some("read_file"));

        let limited = store
            .list_messages_after("s_1", first, 1)
            .await
            .expect("after limit");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].id, after[0].id);

        let beyond = store
            .list_messages_after("s_1", last, 10)
            .await
            .expect("beyond");
        assert!(beyond.is_empty());
    }

    /// Reasoning is part of the assistant turn: reloading a transcript must not
    /// quietly strip it, including on the incremental and subagent read paths.
    #[tokio::test]
    async fn assistant_reasoning_roundtrips_through_every_message_read_path() {
        let db = TempDb::new("message-reasoning");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        let mut assistant = new_message("s_1", "assistant", "answer");
        assistant.agent_id = Some("a_1".to_string());
        assistant.reasoning_content = Some("consider the options".to_string());
        let id = store.append_message(&assistant).await.expect("assistant");
        store
            .append_message(&new_message("s_1", "user", "next"))
            .await
            .expect("user");

        let rows = store.list_messages("s_1", 10).await.expect("rows");
        assert_eq!(
            rows[0].reasoning_content.as_deref(),
            Some("consider the options")
        );
        assert_eq!(
            rows[0].to_message().reasoning_content.as_deref(),
            Some("consider the options")
        );
        assert_eq!(rows[1].reasoning_content, None);

        let after = store
            .list_messages_after("s_1", id - 1, 10)
            .await
            .expect("after");
        assert_eq!(
            after[0].reasoning_content.as_deref(),
            Some("consider the options")
        );

        let agent_rows = store
            .list_agent_messages("a_1", 10)
            .await
            .expect("agent rows");
        assert_eq!(
            agent_rows[0].reasoning_content.as_deref(),
            Some("consider the options")
        );
    }

    /// A database created by v10 already reports v10, so adding the column to
    /// the same schema step would be skipped forever. The v11 step must repair
    /// the identical startup error users saw from `list_messages`.
    #[tokio::test]
    async fn migration_from_v10_adds_reasoning_content_without_losing_rows() {
        let db = TempDb::new("migrate-v10-reasoning");
        std::fs::create_dir_all(db.path.parent().expect("parent")).expect("data dir");
        {
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, 10);
            conn.pragma_update(None, "user_version", 10)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at)
                 VALUES ('s_1', 'Old', '/w', 1, 2)",
                [],
            )
            .expect("session");
            conn.execute(
                "INSERT INTO messages (session_id, role, content, created_at)
                 VALUES ('s_1', 'assistant', 'old answer', 1)",
                [],
            )
            .expect("message");
        }

        let store = db.open().await;
        let rows = store.list_messages("s_1", 10).await.expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "old answer");
        assert_eq!(rows[0].reasoning_content, None);

        let mut assistant = new_message("s_1", "assistant", "new answer");
        assistant.reasoning_content = Some("new reasoning".to_string());
        store.append_message(&assistant).await.expect("new message");
        let rows = store.list_messages("s_1", 10).await.expect("migrated rows");
        assert_eq!(rows[1].reasoning_content.as_deref(), Some("new reasoning"));
    }

    /// Images are stored beside the text and survive a reload: a vision result
    /// that only lived in memory would come back as a tool result with no pixels
    /// after a restart, and the model would describe an image it cannot see.
    #[tokio::test]
    async fn message_images_roundtrip() {
        let db = TempDb::new("message-images");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        let mut tool = new_message("s_1", "tool", "assets/logo.png — image/png");
        tool.tool_call_id = Some("call_1".to_string());
        tool.name = Some("read_file".to_string());
        tool.images = vec![ImageAttachment {
            media_type: "image/png".into(),
            data: "iVBORw0KGgo=".into(),
            bytes: 8,
            name: Some("assets/logo.png".into()),
        }];
        store.append_message(&tool).await.expect("append image");
        // A text-only row keeps the default, so old and new rows read the same.
        store
            .append_message(&new_message("s_1", "user", "plain"))
            .await
            .expect("append text");

        let rows = store.list_messages("s_1", 10).await.expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].images.len(), 1);
        assert_eq!(rows[0].images[0].media_type, "image/png");
        assert_eq!(rows[0].images[0].data, "iVBORw0KGgo=");
        assert_eq!(rows[0].images[0].bytes, 8);
        assert_eq!(rows[0].images[0].name.as_deref(), Some("assets/logo.png"));
        assert!(rows[1].images.is_empty());
        assert_eq!(rows[0].to_message().images.len(), 1);
    }

    /// A transcript written before v5 has no `images` column. Opening it must add
    /// the column with an empty default, not fail or scramble the old rows.
    #[tokio::test]
    async fn migration_from_v4_adds_an_empty_images_column() {
        let db = TempDb::new("migrate-v4-images");
        std::fs::create_dir_all(db.path.parent().expect("parent")).expect("data dir");
        {
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, 4);
            conn.pragma_update(None, "user_version", 4)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at)
                 VALUES ('s_old', 'Old', '/w', 1, 2)",
                [],
            )
            .expect("insert session");
            conn.execute(
                "INSERT INTO messages (session_id, role, content, created_at)
                 VALUES ('s_old', 'user', 'hello from v4', 1)",
                [],
            )
            .expect("insert message");
        }

        let store = db.open().await;
        let rows = store.list_messages("s_old", 10).await.expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].content, "hello from v4");
        assert!(rows[0].images.is_empty(), "old rows default to no images");
    }

    #[tokio::test]
    async fn settings_are_upserted_and_ordered() {
        let db = TempDb::new("settings");
        let store = db.open().await;

        assert!(store.get_setting("api_key").await.expect("get").is_none());
        store.set_setting("api_key", "sk-1").await.expect("set");
        store.set_setting("model", "gpt").await.expect("set");
        assert_eq!(
            store.get_setting("api_key").await.expect("get").as_deref(),
            Some("sk-1")
        );

        store.set_setting("api_key", "sk-2").await.expect("update");
        assert_eq!(
            store.get_setting("api_key").await.expect("get").as_deref(),
            Some("sk-2")
        );

        let all = store.all_settings().await.expect("all");
        assert_eq!(
            all,
            vec![
                ("api_key".to_string(), "sk-2".to_string()),
                ("model".to_string(), "gpt".to_string())
            ]
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM settings").await, 2);
    }

    #[tokio::test]
    async fn settings_batches_roll_back_an_injected_later_write() {
        let db = TempDb::new("settings-batch-atomic");
        let store = db.open().await;
        store
            .set_settings(&[
                ("first".into(), "old-first".into()),
                ("second".into(), "old-second".into()),
            ])
            .await
            .expect("seed");
        store
            .with(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER reject_second_setting
                     BEFORE UPDATE OF value ON settings
                     WHEN NEW.key = 'second' AND NEW.value = 'new-second'
                     BEGIN SELECT RAISE(ABORT, 'injected settings failure'); END;",
                )
                .map_err(|e| db_err("install settings trigger", e))
            })
            .await
            .expect("trigger");
        let values = vec![
            ("first".to_string(), "new-first".to_string()),
            ("second".to_string(), "new-second".to_string()),
        ];
        assert!(store.set_settings(&values).await.is_err());
        assert_eq!(
            store.get_setting("first").await.expect("first").as_deref(),
            Some("old-first"),
            "the earlier successful statement was rolled back"
        );
        assert_eq!(
            store
                .get_setting("second")
                .await
                .expect("second")
                .as_deref(),
            Some("old-second")
        );
        store
            .with(|conn| {
                conn.execute_batch("DROP TRIGGER reject_second_setting")
                    .map_err(|e| db_err("drop settings trigger", e))
            })
            .await
            .expect("drop trigger");
        store.set_settings(&values).await.expect("retry");
        assert_eq!(
            store.get_setting("first").await.expect("first").as_deref(),
            Some("new-first")
        );
        assert_eq!(
            store
                .get_setting("second")
                .await
                .expect("second")
                .as_deref(),
            Some("new-second")
        );
        store.set_settings(&[]).await.expect("empty batch");
    }

    fn new_terminal(id: &str, cwd: &str, position: i64) -> TerminalSession {
        TerminalSession {
            id: id.to_string(),
            title: TerminalSession::default_title(cwd, position),
            cwd: cwd.to_string(),
            project_id: None,
            position,
            created_at: 0,
            updated_at: 0,
            last_used_at: 0,
        }
    }

    /// A terminal row is what makes the tab strip global: it must come back in
    /// tab order after a reopen, with its title and directory intact, because
    /// that is all a fresh browser has to go on.
    #[tokio::test]
    async fn terminals_are_created_listed_renamed_and_deleted() {
        let db = TempDb::new("terminals");
        let path = db.path.clone();
        {
            let store = db.open().await;
            let first = store
                .create_terminal(&new_terminal("t_2", "/work/karucode", 2))
                .await
                .expect("create second");
            assert_eq!(first.title, "karucode-2");
            assert_eq!(first.cwd, "/work/karucode");
            assert_eq!(first.project_id, None);
            assert!(first.created_at > 0);
            // Inserted out of order on purpose: the list is ordered by position,
            // not by insertion.
            store
                .create_terminal(&new_terminal("t_1", "/work/other", 1))
                .await
                .expect("create first");

            let listed = store.list_terminals().await.expect("list");
            assert_eq!(
                listed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
                vec!["t_1", "t_2"]
            );
            assert_eq!(listed[0].title, "other-1");
        }

        // Reopen: the rows are durable, which is the whole point of the table.
        let reopened = SqliteStore::open(&path).await.expect("reopen");
        let listed = reopened.list_terminals().await.expect("list after reopen");
        assert_eq!(listed.len(), 2, "terminals survive a restart");
        assert_eq!(listed[1].cwd, "/work/karucode");
        assert_eq!(listed[1].title, "karucode-2");

        reopened
            .rename_terminal("t_2", Some("build watcher"))
            .await
            .expect("rename");
        let renamed = reopened
            .get_terminal("t_2")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(renamed.title, "build watcher");
        // Clearing the name restores the derived one rather than an empty tab.
        reopened
            .rename_terminal("t_2", None)
            .await
            .expect("reset title");
        assert_eq!(
            reopened.get_terminal("t_2").await.unwrap().unwrap().title,
            "karucode-2"
        );

        reopened.touch_terminal("t_2").await.expect("touch");
        assert!(
            reopened
                .get_terminal("t_2")
                .await
                .unwrap()
                .unwrap()
                .last_used_at
                >= renamed.last_used_at
        );

        reopened.delete_terminal("t_2").await.expect("delete");
        assert_eq!(
            reopened.terminal_ids().await.expect("ids"),
            vec!["t_1".to_string()]
        );
        assert!(reopened.get_terminal("t_2").await.expect("get").is_none());
        // An unknown id is a clear NotFound, not a silent success.
        assert!(reopened.rename_terminal("t_404", Some("x")).await.is_err());
    }

    /// `default_title` names a tab after its directory, and copes with the root
    /// (which has no basename) instead of producing an empty label.
    #[test]
    fn terminal_default_titles_come_from_the_directory() {
        assert_eq!(
            TerminalSession::default_title("/home/me/project", 1),
            "project-1"
        );
        assert_eq!(TerminalSession::default_title("/", 3), "/-3");
        assert_eq!(TerminalSession::default_title("", 0), "-1");
        assert_eq!(
            TerminalSession::default_title("/tmp/trailing/", 2),
            "trailing-2"
        );
    }

    #[tokio::test]
    async fn runs_are_created_finished_and_interrupted() {
        let db = TempDb::new("runs");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        store
            .create_run("r_1", "s_1", "model-a", "prompt-a")
            .await
            .expect("r1");
        store
            .create_run("r_2", "s_1", "model-b", "prompt-b")
            .await
            .expect("r2");

        let runs = store.list_runs("s_1", 10).await.expect("list");
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.status == "running"));
        assert!(runs.iter().all(|r| r.finished_at.is_none()));
        assert!(runs.iter().all(|r| r.started_at > 0));

        store
            .finish_run("r_1", RunStatus::Failed, Some("boom"))
            .await
            .expect("finish");
        let runs = store.list_runs("s_1", 10).await.expect("list");
        let r1 = runs.iter().find(|r| r.id == "r_1").expect("r1");
        assert_eq!(r1.status, "failed");
        assert_eq!(r1.error.as_deref(), Some("boom"));
        assert!(r1.finished_at.is_some());

        // Only the still-running run is interrupted. A restart leaves the row
        // stamped with the *previous* process's instance id, so simulate that:
        // recovery marks rows that are not this instance's, because the data-dir
        // lock guarantees no peer is live.
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE runs SET instance_id = 'previous-process' WHERE id = 'r_2'",
                    [],
                )
                .map_err(|e| db_err("stamp foreign run", e))?;
                Ok(())
            })
            .await
            .expect("stamp foreign run");
        let interrupted = store.mark_interrupted_runs().await.expect("interrupt");
        assert_eq!(interrupted, 1);
        let runs = store.list_runs("s_1", 10).await.expect("list");
        let r2 = runs.iter().find(|r| r.id == "r_2").expect("r2");
        assert_eq!(r2.status, "interrupted");
        assert!(r2.finished_at.is_some());
        assert_eq!(store.mark_interrupted_runs().await.expect("noop"), 0);

        // Unknown run ids are tolerated on finish.
        store
            .finish_run("r_missing", RunStatus::Completed, None)
            .await
            .expect("finish missing");
    }

    #[tokio::test]
    async fn list_runs_limits_to_newest() {
        let db = TempDb::new("runs-limit");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        for i in 0..5 {
            store
                .create_run(&format!("r_{i}"), "s_1", "m", &format!("p{i}"))
                .await
                .expect("run");
            // started_at has 1ms resolution; keep ordering deterministic.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        store
            .create_session("s_2", "t", "/w", None)
            .await
            .expect("s2");
        store
            .create_run("r_other", "s_2", "m", "p")
            .await
            .expect("other");

        let runs = store.list_runs("s_1", 2).await.expect("list");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "r_4");
        assert_eq!(runs[1].id, "r_3");
        assert!(runs.iter().all(|r| r.session_id == "s_1"));
    }

    #[tokio::test]
    async fn agents_are_upserted_listed_and_interrupted() {
        let db = TempDb::new("agents");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        store
            .upsert_agent(&agent("a_1", "s_1", "running"))
            .await
            .expect("insert");
        store
            .upsert_agent(&agent("a_2", "s_1", "completed"))
            .await
            .expect("insert");
        let created_at = store
            .list_agents("s_1")
            .await
            .expect("list")
            .into_iter()
            .find(|a| a.id == "a_1")
            .expect("a_1")
            .created_at;

        let mut updated = agent("a_1", "s_1", "completed");
        updated.iterations = 7;
        updated.summary = "done".to_string();
        updated.error = Some("warn".to_string());
        updated.owned_paths = vec!["a".to_string(), "b/**".to_string()];
        store.upsert_agent(&updated).await.expect("update");

        let agents = store.list_agents("s_1").await.expect("list");
        assert_eq!(agents.len(), 2);
        let a1 = agents.iter().find(|a| a.id == "a_1").expect("a1");
        assert_eq!(a1.status, "completed");
        assert_eq!(a1.iterations, 7);
        assert_eq!(a1.summary, "done");
        assert_eq!(a1.owned_paths, vec!["a".to_string(), "b/**".to_string()]);
        assert_eq!(
            a1.created_at, created_at,
            "created_at is preserved on update"
        );

        // a_2 completed -> not interrupted; flip it back to running to test
        // counting, stamped as a previous process left it.
        store
            .upsert_agent(&agent("a_2", "s_1", "running"))
            .await
            .expect("reopen");
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE agents SET instance_id = 'previous-process' WHERE id = 'a_2'",
                    [],
                )
                .map_err(|e| db_err("stamp foreign agent", e))?;
                Ok(())
            })
            .await
            .expect("stamp foreign agent");
        assert_eq!(store.mark_interrupted_agents().await.expect("interrupt"), 1);
        let agents = store.list_agents("s_1").await.expect("list");
        let a2 = agents.iter().find(|a| a.id == "a_2").expect("a2");
        assert_eq!(a2.status, "interrupted");
        assert!(a2.finished_at.is_some());
        assert_eq!(store.mark_interrupted_agents().await.expect("noop"), 0);
    }

    #[tokio::test]
    async fn agents_list_workspace_wide_and_carry_session_context() {
        let db = TempDb::new("agents-all");
        let store = db.open().await;
        let p1 = store
            .create_project("p_1", "one", "/w/one")
            .await
            .expect("p1");
        let p2 = store
            .create_project("p_2", "two", "/w/two")
            .await
            .expect("p2");
        store
            .create_session("s_1", "Alpha", "/w/one", Some(&p1.id))
            .await
            .expect("s1");
        store
            .create_session("s_2", "Beta", "/w/two", Some(&p2.id))
            .await
            .expect("s2");
        store
            .upsert_agent(&agent("a_1", "s_1", "running"))
            .await
            .expect("a1");
        store
            .upsert_agent(&agent("a_2", "s_2", "completed"))
            .await
            .expect("a2");

        let all = store.list_agents_all(None).await.expect("all");
        assert_eq!(all.len(), 2);
        let one = all
            .iter()
            .find(|summary| summary.agent.id == "a_1")
            .expect("a_1");
        assert_eq!(one.session_title, "Alpha");
        assert_eq!(one.workspace, "/w/one");
        assert_eq!(one.project_id.as_deref(), Some("p_1"));

        // `Some(project)` is a real filter, not a hint.
        let scoped = store.list_agents_all(Some(&p2.id)).await.expect("scoped");
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].agent.id, "a_2");

        let got = store
            .get_agent("a_2")
            .await
            .expect("get")
            .expect("a_2 present");
        assert_eq!(got.session_title, "Beta");
        assert!(store.get_agent("a_missing").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn conditional_agent_transitions_preserve_terminal_results() {
        let db = TempDb::new("agent-transition-cas");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .upsert_agent(&agent("a_1", "s_1", "running"))
            .await
            .expect("insert");
        assert!(store
            .transition_agent_status("a_1", &["running"], "paused", 111)
            .await
            .expect("pause"));
        let paused = store.list_agents("s_1").await.expect("paused").remove(0);
        assert_eq!(paused.status, "paused");
        assert_eq!(paused.updated_at, 111);
        assert_eq!(paused.task, "review the diff");
        assert_eq!(paused.owned_paths, vec!["crates/foo/**"]);

        let mut terminal = paused.clone();
        terminal.status = "completed".into();
        terminal.iterations = 9;
        terminal.summary = "finished with the answer".into();
        terminal.error = Some("late warning".into());
        terminal.finished_at = Some(222);
        terminal.note = "terminal note".into();
        terminal.progress = Some(1.0);
        store.upsert_agent(&terminal).await.expect("terminal write");
        assert!(!store
            .transition_agent_status("a_1", &["paused"], "running", 333)
            .await
            .expect("late resume loses"));
        assert!(!store
            .transition_agent_status("a_1", &["running", "paused"], "paused", 334)
            .await
            .expect("late pause loses"));
        assert!(!store
            .transition_agent_status("missing", &["running"], "paused", 335)
            .await
            .expect("missing"));
        let after = store.get_agent("a_1").await.expect("get").unwrap().agent;
        assert_eq!(after.status, "completed");
        assert_eq!(after.summary, "finished with the answer");
        assert_eq!(after.error.as_deref(), Some("late warning"));
        assert_eq!(after.iterations, 9);
        assert_eq!(after.finished_at, Some(222));
        assert_eq!(after.updated_at, terminal.updated_at);
        assert_eq!(after.note, "terminal note");
    }

    #[tokio::test]
    async fn paused_agents_are_recovered_and_their_claims_are_released() {
        let db = TempDb::new("agents-paused-recovery");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .create_session("s_2", "t", "/w", None)
            .await
            .expect("session");
        store
            .upsert_agent(&agent("a_running", "s_1", "running"))
            .await
            .expect("running");
        store
            .upsert_agent(&agent("a_paused", "s_2", "paused"))
            .await
            .expect("paused");
        store
            .upsert_agent(&agent("a_done", "s_2", "completed"))
            .await
            .expect("done");
        store
            .with(|conn| {
                conn.execute("UPDATE agents SET instance_id = 'dead-process'", [])
                    .map_err(|e| db_err("stamp dead agents", e))
            })
            .await
            .expect("stamp");
        let live = store.running_agent_ids().await.expect("live");
        assert_eq!(
            live,
            vec![
                ("s_1".to_string(), "a_running".to_string()),
                ("s_2".to_string(), "a_paused".to_string()),
            ]
        );
        store
            .claim_paths("s_2", "a_paused", &["src/lib.rs".into()], "write")
            .await
            .expect("claim");
        assert_eq!(store.list_claims("s_2").await.expect("claims").len(), 1);
        assert_eq!(
            store
                .recovery_sessions(0, 10, true)
                .await
                .expect("candidates")
                .1,
            vec!["s_2".to_string(), "s_1".to_string()]
        );
        assert_eq!(store.mark_interrupted_agents().await.expect("interrupt"), 2);
        for (_, id) in live {
            store.release_claims(&id).await.expect("release");
        }
        assert!(store.list_claims("s_2").await.expect("claims").is_empty());
        assert_eq!(
            store
                .get_agent("a_paused")
                .await
                .expect("get")
                .unwrap()
                .agent
                .status,
            "interrupted"
        );
        assert_eq!(
            store
                .get_agent("a_done")
                .await
                .expect("get")
                .unwrap()
                .agent
                .status,
            "completed"
        );
    }

    #[tokio::test]
    async fn retention_preserves_a_paused_agent_and_its_claim() {
        let db = TempDb::new("retention-paused-agent");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .upsert_agent(&agent("a_paused", "s_1", "paused"))
            .await
            .expect("paused");
        store
            .claim_paths("s_1", "a_paused", &["src/lib.rs".into()], "write")
            .await
            .expect("claim");
        store
            .with(|conn| {
                conn.execute("UPDATE agents SET created_at = 1 WHERE id = 'a_paused'", [])
                    .map_err(|e| db_err("age paused agent", e))
            })
            .await
            .expect("age");
        store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(
            store
                .get_agent("a_paused")
                .await
                .expect("get")
                .expect("paused survives")
                .agent
                .status,
            "paused"
        );
        assert_eq!(store.list_claims("s_1").await.expect("claims").len(), 1);
    }

    #[tokio::test]
    async fn agent_filters_do_not_truncate_matching_public_results() {
        let db = TempDb::new("agents-filter-sql");
        let store = db.open().await;
        store.create_project("p1", "One", "/1").await.expect("p1");
        store.create_project("p2", "Two", "/2").await.expect("p2");
        store
            .create_session("s_1", "one", "/1", Some("p1"))
            .await
            .expect("s1");
        store
            .create_session("s_2", "two", "/2", Some("p2"))
            .await
            .expect("s2");
        store
            .with(|conn| {
                let tx = conn.transaction().map_err(|e| db_err("seed agents", e))?;
                for i in 0..150 {
                    tx.execute(
                        "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status, owned_paths, created_at, updated_at, instance_id, background, note)
                         VALUES (?1, 's_1', 'r', 'n', 'r', 't', 'paused', '[]', ?2, ?2, 'i', 0, '')",
                        params![format!("paused_{i:03}"), 150 - i],
                    ).map_err(|e| db_err("seed paused agent", e))?;
                }
                tx.execute(
                    "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status, owned_paths, created_at, updated_at, instance_id, background, note)
                     VALUES ('running_1', 's_2', 'r', 'n', 'r', 't', 'running', '[]', 200, 200, 'i', 0, '')", [],
                ).map_err(|e| db_err("seed running agent", e))?;
                tx.commit().map_err(|e| db_err("commit agents", e))
            })
            .await
            .expect("seed");
        let paused = vec!["paused".to_string()];
        let filtered = store
            .list_agents_filtered(Some("p1"), Some("s_1"), &paused)
            .await
            .expect("filtered");
        assert_eq!(filtered.len(), 150, "no hidden page cap");
        assert_eq!(filtered[0].agent.id, "paused_000");
        assert_eq!(filtered[149].agent.id, "paused_149");
        assert!(store
            .list_agents_filtered(Some("p2"), Some("s_1"), &paused)
            .await
            .expect("mismatched filters")
            .is_empty());
        let statuses = vec!["paused".to_string(), "running".to_string()];
        assert_eq!(
            store
                .list_agents_filtered(None, Some("s_2"), &statuses)
                .await
                .expect("session filter")
                .len(),
            1
        );
        assert_eq!(
            store
                .list_agents_filtered(None, None, &statuses)
                .await
                .expect("global filter")
                .len(),
            151
        );
    }

    #[tokio::test]
    async fn approvals_resolve_list_and_filter_by_session() {
        let db = TempDb::new("approvals");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("s1");
        store
            .create_session("s_2", "t", "/w", None)
            .await
            .expect("s2");

        store
            .create_approval(&approval("ap_1", "s_1", "r_1"))
            .await
            .expect("ap1");
        store
            .create_approval(&approval("ap_2", "s_2", "r_2"))
            .await
            .expect("ap2");

        let pending = store
            .list_pending_approvals(None)
            .await
            .expect("all pending");
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].payload["command"], "cargo test");

        let pending_s1 = store
            .list_pending_approvals(Some("s_1"))
            .await
            .expect("s1 pending");
        assert_eq!(pending_s1.len(), 1);
        assert_eq!(pending_s1[0].id, "ap_1");

        store
            .resolve_approval("ap_1", "allow_once")
            .await
            .expect("resolve");
        let pending = store
            .list_pending_approvals(None)
            .await
            .expect("all pending");
        assert_eq!(pending.len(), 1);

        let resolved = store
            .with(|conn| {
                conn.query_row(
                    "SELECT status, decision, resolved_at FROM approvals WHERE id = 'ap_1'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                        ))
                    },
                )
                .map_err(|e| db_err("read approval", e))
            })
            .await
            .expect("read approval");
        assert_eq!(resolved.0, "resolved");
        assert_eq!(resolved.1.as_deref(), Some("allow_once"));
        assert!(resolved.2.is_some());

        // Resolving twice is a NotFound, not a panic.
        assert!(matches!(
            store.resolve_approval("ap_1", "deny").await,
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            store.resolve_approval("ap_unknown", "deny").await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn expire_pending_approvals_only_touches_that_run() {
        let db = TempDb::new("approvals-expire");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        store
            .create_approval(&approval("ap_1", "s_1", "r_1"))
            .await
            .expect("ap1");
        store
            .create_approval(&approval("ap_2", "s_1", "r_1"))
            .await
            .expect("ap2");
        store
            .create_approval(&approval("ap_3", "s_1", "r_2"))
            .await
            .expect("ap3");
        store
            .resolve_approval("ap_2", "deny")
            .await
            .expect("resolve ap2");

        let expired = store
            .expire_pending_approvals("r_1", "run cancelled")
            .await
            .expect("expire");
        assert_eq!(expired, 1);
        assert_eq!(
            store
                .expire_pending_approvals("r_1", "again")
                .await
                .expect("noop"),
            0
        );

        let pending = store.list_pending_approvals(None).await.expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "ap_3");

        let row = store
            .with(|conn| {
                conn.query_row(
                    "SELECT status, decision, detail, resolved_at FROM approvals WHERE id = 'ap_1'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .map_err(|e| db_err("read approval", e))
            })
            .await
            .expect("read");
        assert_eq!(row.0, "expired");
        assert_eq!(row.1.as_deref(), Some("expired"));
        assert_eq!(row.2, "cargo test", "detail is preserved");
        assert!(row.3.is_some());
    }

    #[tokio::test]
    async fn claims_report_overlaps_from_other_agents_only() {
        let db = TempDb::new("claims-conflict");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        let mine = store
            .claim_paths(
                "s_1",
                "a_1",
                &["crates/foo/src/lib.rs".to_string()],
                "write",
            )
            .await
            .expect("claim mine");
        assert!(mine.is_empty(), "no other agents yet");

        // Same agent re-claiming its own path is not a conflict.
        let again = store
            .claim_paths(
                "s_1",
                "a_1",
                &["crates/foo/src/lib.rs".to_string()],
                "write",
            )
            .await
            .expect("reclaim");
        assert!(again.is_empty());

        // Identical path from another agent conflicts.
        let identical = store
            .claim_paths(
                "s_1",
                "a_2",
                &["crates/foo/src/lib.rs".to_string()],
                "write",
            )
            .await
            .expect("identical");
        assert_eq!(identical.len(), 1);
        assert_eq!(identical[0].agent_id, "a_1");
        assert_eq!(identical[0].path, "crates/foo/src/lib.rs");

        // Ancestor directory conflicts (prefix at a '/' boundary): both agents
        // that claimed the file below it are reported.
        let ancestor = store
            .claim_paths("s_1", "a_3", &["crates/foo".to_string()], "write")
            .await
            .expect("ancestor");
        let mut owners: Vec<&str> = ancestor.iter().map(|c| c.agent_id.as_str()).collect();
        owners.sort_unstable();
        assert_eq!(owners, vec!["a_1", "a_2"]);

        // Descendant of an existing claim conflicts; every overlapping owner is
        // reported (a_1/a_2 own the file, a_3 owns the parent directory).
        let descendant = store
            .claim_paths(
                "s_1",
                "a_4",
                &["crates/foo/src/lib.rs/inner".to_string()],
                "write",
            )
            .await
            .expect("descendant");
        let mut owners: Vec<&str> = descendant.iter().map(|c| c.agent_id.as_str()).collect();
        owners.sort_unstable();
        assert_eq!(owners, vec!["a_1", "a_2", "a_3"]);

        // Sibling paths and plain string prefixes do not conflict.
        let sibling = store
            .claim_paths(
                "s_1",
                "a_5",
                &["crates/foobar/src/lib.rs".to_string()],
                "write",
            )
            .await
            .expect("sibling");
        assert!(
            sibling.is_empty(),
            "shared prefix without a slash is not a conflict"
        );

        let other_session = {
            store
                .create_session("s_2", "t2", "/w", None)
                .await
                .expect("s2");
            store
                .claim_paths(
                    "s_2",
                    "a_6",
                    &["crates/foo/src/lib.rs".to_string()],
                    "write",
                )
                .await
                .expect("other session")
        };
        assert!(other_session.is_empty(), "claims are scoped per session");

        // Modes are advisory metadata.
        let claims = store.list_claims("s_1").await.expect("list claims");
        assert_eq!(claims.len(), 5);
        assert!(claims.iter().all(|c| c.mode == "write"));

        // Trailing slashes and "./" are normalised.
        let normalised = store
            .claim_paths("s_1", "a_7", &["./crates/foo/".to_string()], "write")
            .await
            .expect("normalised");
        let mut owners: Vec<&str> = normalised.iter().map(|c| c.agent_id.as_str()).collect();
        owners.sort_unstable();
        assert_eq!(owners, vec!["a_1", "a_2", "a_3", "a_4"]);
        let claims = store.list_claims("s_1").await.expect("list claims");
        assert!(claims.iter().any(|c| c.path == "crates/foo"));
    }

    #[tokio::test]
    async fn release_claims_removes_only_that_agent() {
        let db = TempDb::new("claims-release");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        store
            .claim_paths(
                "s_1",
                "a_1",
                &["a/x".to_string(), "b/y".to_string()],
                "write",
            )
            .await
            .expect("a1");
        store
            .claim_paths("s_1", "a_2", &["c/z".to_string()], "read")
            .await
            .expect("a2");
        assert_eq!(store.list_claims("s_1").await.expect("list").len(), 3);

        store.release_claims("a_1").await.expect("release");
        let claims = store.list_claims("s_1").await.expect("list");
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].agent_id, "a_2");
        assert_eq!(claims[0].mode, "read");
        // Releasing again is a no-op.
        store.release_claims("a_1").await.expect("release again");
    }

    #[tokio::test]
    async fn concurrent_writes_from_four_tasks_are_serialised() {
        let db = TempDb::new("concurrent");
        let store = Arc::new(db.open().await);

        let mut handles = Vec::new();
        for task in 0..4u32 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..5u32 {
                    let session = format!("s_{task}_{i}");
                    store
                        .create_session(&session, "t", "/w", None)
                        .await
                        .expect("create session");
                    for j in 0..3u32 {
                        store
                            .append_message(&new_message(&session, "user", &format!("m{j}")))
                            .await
                            .expect("append");
                    }
                    store
                        .set_setting(&format!("k_{task}_{i}"), "v")
                        .await
                        .expect("setting");
                    store
                        .create_run(&format!("r_{task}_{i}"), &session, "m", "p")
                        .await
                        .expect("run");
                }
            }));
        }
        for handle in handles {
            handle.await.expect("task");
        }

        assert_eq!(count(&store, "SELECT COUNT(*) FROM sessions").await, 20);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM messages").await, 60);
        assert_eq!(count(&store, "SELECT COUNT(*) FROM settings").await, 20);
        // Every session kept its own messages in order.
        for task in 0..4 {
            for i in 0..5 {
                let session = format!("s_{task}_{i}");
                let messages = store.list_messages(&session, 10).await.expect("messages");
                assert_eq!(messages.len(), 3);
                assert_eq!(
                    messages
                        .iter()
                        .map(|m| m.content.as_str())
                        .collect::<Vec<_>>(),
                    vec!["m0", "m1", "m2"]
                );
            }
        }
        assert_eq!(store.list_sessions().await.expect("sessions").len(), 20);
    }

    #[tokio::test]
    async fn newer_schema_is_refused_instead_of_silently_accepted() {
        let dir = std::env::temp_dir().join(format!(
            "karucode-store-test-newer-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("karucode.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99i64).unwrap();
        }
        match SqliteStore::open(&path).await {
            Err(Error::Store(message)) => assert!(message.contains("newer"), "{message}"),
            Err(other) => panic!("expected Store error, got {other}"),
            Ok(_) => panic!("a database from a newer build must be refused"),
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn reopen_preserves_rows_and_schema_version() {
        let db = TempDb::new("reopen");
        let path = db.path.clone();

        {
            let store = db.open().await;
            store
                .create_session("s_1", "Persisted", "/w", None)
                .await
                .expect("session");
            store
                .append_message(&new_message("s_1", "user", "hello"))
                .await
                .expect("message");
            store.create_run("r_1", "s_1", "m", "p").await.expect("run");
            store.set_setting("api_key", "sk-1").await.expect("setting");
            store
                .claim_paths("s_1", "a_1", &["src/**".to_string()], "write")
                .await
                .expect("claim");
        }

        let reopened = SqliteStore::open(&path).await.expect("reopen");
        assert_eq!(
            reopened
                .get_session("s_1")
                .await
                .expect("get")
                .expect("some")
                .title,
            "Persisted"
        );
        assert_eq!(
            reopened.list_messages("s_1", 10).await.expect("msg").len(),
            1
        );
        assert_eq!(reopened.list_runs("s_1", 10).await.expect("runs").len(), 1);
        assert_eq!(
            reopened
                .get_setting("api_key")
                .await
                .expect("setting")
                .as_deref(),
            Some("sk-1")
        );
        assert_eq!(reopened.list_claims("s_1").await.expect("claims").len(), 1);

        let version = reopened
            .with(|conn| {
                conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .map_err(|e| db_err("user_version", e))
            })
            .await
            .expect("version");
        assert_eq!(
            version, SCHEMA_VERSION,
            "reopening must not re-run migrations"
        );
    }

    #[tokio::test]
    async fn malformed_json_and_missing_rows_fall_back_to_defaults() {
        let db = TempDb::new("malformed");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO messages (session_id, role, content, tool_calls, created_at)
                     VALUES ('s_1', 'assistant', 'bad json', '{not json', 1)",
                    [],
                )
                .map_err(|e| db_err("insert malformed message", e))?;
                conn.execute(
                    "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status,
                                         owned_paths, created_at, updated_at)
                     VALUES ('a_bad', 's_1', 'r', 'n', 'r', 't', 'running', 'not-json', 1, 1)",
                    [],
                )
                .map_err(|e| db_err("insert malformed agent", e))?;
                conn.execute(
                    "INSERT INTO approvals (id, session_id, run_id, kind, payload, status, created_at)
                     VALUES ('ap_bad', 's_1', 'r', 'other', 'nope', 'pending', 1)",
                    [],
                )
                .map_err(|e| db_err("insert malformed approval", e))?;
                Ok(())
            })
            .await
            .expect("seed malformed rows");

        let messages = store.list_messages("s_1", 10).await.expect("messages");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].tool_calls.is_empty());

        let agents = store.list_agents("s_1").await.expect("agents");
        assert_eq!(agents.len(), 1);
        assert!(agents[0].owned_paths.is_empty());

        let approvals = store
            .list_pending_approvals(Some("s_1"))
            .await
            .expect("approvals");
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].payload, serde_json::Value::Null);

        // Unknown ids yield empty/None rather than errors.
        assert!(store
            .list_messages("s_none", 10)
            .await
            .expect("msgs")
            .is_empty());
        assert!(store
            .list_runs("s_none", 10)
            .await
            .expect("runs")
            .is_empty());
        assert!(store
            .list_agents("s_none")
            .await
            .expect("agents")
            .is_empty());
        assert!(store
            .list_claims("s_none")
            .await
            .expect("claims")
            .is_empty());
        assert!(store.get_setting("nope").await.expect("setting").is_none());
        store.touch_session("s_none").await.expect("touch missing");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn database_file_and_directory_are_restricted() {
        use std::os::unix::fs::PermissionsExt;

        let db = TempDb::new("permissions");
        let store = db.open().await;
        drop(store);

        let dir_mode = std::fs::metadata(&db.dir)
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "data directory must be 0700");

        let file_mode = std::fs::metadata(&db.path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "database file must be 0600");
    }

    #[tokio::test]
    async fn open_enables_wal_and_foreign_keys() {
        let db = TempDb::new("pragmas");
        let store = db.open().await;
        let (mode, sync, fk, timeout) = store
            .with(|conn| {
                conn.query_row(
                    "SELECT (SELECT * FROM pragma_journal_mode),
                            (SELECT * FROM pragma_synchronous),
                            (SELECT * FROM pragma_foreign_keys),
                            (SELECT * FROM pragma_busy_timeout)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .map_err(|e| db_err("pragmas", e))
            })
            .await
            .expect("pragmas");
        assert_eq!(mode.to_lowercase(), "wal", "journal_mode must be WAL");
        assert_eq!(sync, 1, "synchronous must be NORMAL (1)");
        assert_eq!(fk, 1, "foreign_keys must be ON for cascades");
        assert_eq!(timeout, 5000, "busy_timeout should be 5s");
    }

    #[tokio::test]
    async fn path_getter_returns_opened_path() {
        let db = TempDb::new("path");
        let store = db.open().await;
        assert_eq!(store.path(), db.path.as_path());
    }

    // ---- retention, maintenance and diagnostics -------------------------

    /// Backdate every message of `session_id` so it falls outside a `days`
    /// retention window.
    async fn age_messages(store: &SqliteStore, session_id: &str, days: u64) {
        let cutoff = retention_cutoff(days) - 1_000;
        let session_id = session_id.to_string();
        store
            .with(move |conn| {
                conn.execute(
                    "UPDATE messages SET created_at = ?2 WHERE session_id = ?1",
                    params![session_id, cutoff],
                )
                .map_err(|e| db_err("age messages", e))?;
                Ok(())
            })
            .await
            .expect("age messages");
    }

    async fn index_names(store: &SqliteStore, table: &str) -> Vec<String> {
        let sql = format!("PRAGMA index_list('{table}')");
        store
            .with(move |conn| {
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| db_err("index_list prepare", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(1))
                    .map_err(|e| db_err("index_list query", e))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| db_err("index_list rows", e))
            })
            .await
            .expect("index_list")
    }

    #[tokio::test]
    async fn pruning_removes_old_messages_but_keeps_the_newest_per_session() {
        let db = TempDb::new("prune-keep-newest");
        let store = db.open().await;
        let keep = store.retention_keep_messages();
        assert!(keep > 0, "a retention pass must always keep some context");

        store
            .create_session("s_big", "big", "/w", None)
            .await
            .expect("big session");
        store
            .create_session("s_small", "small", "/w", None)
            .await
            .expect("small session");

        // One session well over the keep-window, one comfortably under it.
        for i in 0..(keep + 50) {
            store
                .append_message(&new_message("s_big", "user", &format!("old{i}")))
                .await
                .expect("append big");
        }
        for i in 0..5 {
            store
                .append_message(&new_message("s_small", "user", &format!("small{i}")))
                .await
                .expect("append small");
        }
        age_messages(&store, "s_big", 30).await;
        age_messages(&store, "s_small", 30).await;

        // A message written now must survive even though the session is old.
        store
            .append_message(&new_message("s_big", "user", "recent"))
            .await
            .expect("append recent");

        let report = store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(
            report.messages, 51,
            "only the aged messages past the keep-window are deleted"
        );

        let big = store.list_messages("s_big", 10_000).await.expect("big");
        assert_eq!(big.len(), keep, "the newest {keep} messages are kept");
        assert_eq!(
            big.last().expect("last").content,
            "recent",
            "the newest message always survives"
        );
        assert!(
            big.windows(2).all(|w| w[0].id < w[1].id),
            "kept messages stay in order"
        );

        let small = store.list_messages("s_small", 10_000).await.expect("small");
        assert_eq!(small.len(), 5, "a session under the window loses nothing");

        // A second pass has nothing left to do.
        let again = store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(again.messages, 0);
    }

    #[tokio::test]
    async fn pruning_with_zero_days_is_a_noop() {
        let db = TempDb::new("prune-zero");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        for i in 0..10 {
            store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
        }
        // Even ancient rows survive: 0 means "keep everything".
        age_messages(&store, "s_1", 3650).await;

        let report = store.prune_messages_older_than(0).await.expect("prune");
        assert!(report.is_empty(), "0 days must delete nothing: {report:?}");
        assert_eq!(report.total(), 0);
        assert_eq!(
            store
                .list_messages("s_1", 100)
                .await
                .expect("messages")
                .len(),
            10
        );
    }

    #[tokio::test]
    async fn pruning_removes_orphaned_runs_agents_and_approvals() {
        let db = TempDb::new("prune-orphans");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        // A finished run/agent/approval whose transcript is about to be pruned.
        // The message carries the run and agent ids, so the run and agent really
        // do have a transcript until the message goes.
        let mut old = new_message("s_1", "assistant", "old");
        old.run_id = Some("r_old".to_string());
        old.agent_id = Some("a_old".to_string());
        store.append_message(&old).await.expect("message");
        store
            .create_run("r_old", "s_1", "m", "p")
            .await
            .expect("run");
        store
            .finish_run("r_old", RunStatus::Completed, None)
            .await
            .expect("finish");
        store
            .upsert_agent(&agent("a_old", "s_1", "completed"))
            .await
            .expect("agent");
        store
            .create_approval(&approval("ap_old", "s_1", "r_old"))
            .await
            .expect("approval");
        store
            .resolve_approval("ap_old", "allow_once")
            .await
            .expect("resolve");

        // A live run must never be pruned, however old the session looks. It is
        // created after the aging step below so its `started_at` stays current.
        age_messages(&store, "s_1", 30).await;
        store
            .with(|conn| {
                conn.execute("UPDATE runs SET started_at = 1", [])
                    .map_err(|e| db_err("age runs", e))?;
                conn.execute("UPDATE agents SET created_at = 1", [])
                    .map_err(|e| db_err("age agents", e))?;
                conn.execute("UPDATE approvals SET created_at = 1", [])
                    .map_err(|e| db_err("age approvals", e))?;
                Ok(())
            })
            .await
            .expect("age rows");
        store
            .create_run("r_live", "s_1", "m", "p")
            .await
            .expect("live run");

        // The newest message of a session is always kept, so the first pass
        // leaves the transcript (and therefore the run) in place.
        let report = store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(report.messages, 0, "the newest message is always kept");
        assert_eq!(report.runs, 0, "a run with a transcript is not orphaned");

        // Drop the transcript and prune again to exercise the orphan path.
        store
            .with(|conn| {
                conn.execute("DELETE FROM messages", [])
                    .map_err(|e| db_err("drop messages", e))?;
                Ok(())
            })
            .await
            .expect("drop messages");
        let report = store
            .prune_messages_older_than(30)
            .await
            .expect("prune orphans");
        assert_eq!(report.runs, 1, "the finished, transcript-less run goes");
        assert_eq!(report.agents, 1, "the finished, transcript-less agent goes");
        assert_eq!(report.approvals, 1, "the approval of the pruned run goes");

        let runs = store.list_runs("s_1", 10).await.expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, "r_live", "a running run is never pruned");
    }

    /// `claims` has only a `session_id` foreign key, so a pruned agent's path
    /// claims used to outlive it (measured: 0 agents, 1 claim) and every later
    /// subagent touching those paths was warned about an agent that no longer
    /// existed. The runtime's `release_claims` cannot be relied on: a claim left
    /// behind by a crash is exactly the kind that needs sweeping.
    #[tokio::test]
    async fn pruning_an_agent_releases_its_path_claims() {
        let db = TempDb::new("prune-claims");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .upsert_agent(&agent("a_old", "s_1", "completed"))
            .await
            .expect("agent");
        store
            .upsert_agent(&agent("a_live", "s_1", "running"))
            .await
            .expect("live agent");
        store
            .claim_paths("s_1", "a_old", &["src/lib.rs".to_string()], "write")
            .await
            .expect("claim");
        store
            .claim_paths("s_1", "a_live", &["src/other.rs".to_string()], "write")
            .await
            .expect("live claim");
        store
            .with(|conn| {
                conn.execute("UPDATE agents SET created_at = 1", [])
                    .map_err(|e| db_err("age agents", e))?;
                Ok(())
            })
            .await
            .expect("age agents");
        // The agent has no transcript, so this pass prunes it.
        let report = store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(report.agents, 1, "the finished, transcript-less agent goes");

        let claims = store.list_claims("s_1").await.expect("claims");
        assert!(
            claims.iter().all(|c| c.agent_id != "a_old"),
            "a dead agent's claims must not outlive it: {claims:?}"
        );
        assert!(
            claims.iter().any(|c| c.agent_id == "a_live"),
            "a live agent's claims are not retention's business: {claims:?}"
        );
    }

    /// The retention pass's orphan check is a `NOT EXISTS` over
    /// `messages(run_id)`. Without an index that is a correlated full scan per
    /// candidate row - 1.2 s for 149 runs over 33k messages, inside the
    /// transaction that holds the single writer. The plan must be a search, not
    /// a scan of `messages`.
    #[tokio::test]
    async fn the_retention_orphan_check_uses_the_run_id_index() {
        let db = TempDb::new("plan-run-id");
        let store = db.open().await;
        let indexes = index_names(&store, "messages").await;
        assert!(
            indexes.iter().any(|n| n == "idx_messages_run_id"),
            "the prune needs messages(run_id): {indexes:?}"
        );

        let plan: Vec<String> = store
            .with(|conn| {
                let mut stmt = conn
                    .prepare(
                        "EXPLAIN QUERY PLAN
                         SELECT 1 FROM runs
                          WHERE status <> 'running'
                            AND NOT EXISTS (
                                  SELECT 1 FROM messages m WHERE m.run_id = runs.id
                            )",
                    )
                    .map_err(|e| db_err("plan prepare", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(3))
                    .map_err(|e| db_err("plan rows", e))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| db_err("plan collect", e))
            })
            .await
            .expect("plan");
        let joined = plan.join("\n");
        assert!(
            joined.contains("idx_messages_run_id"),
            "the subquery must be answered by the index, not a scan:\n{joined}"
        );
        assert!(
            !joined.contains("SCAN m\n") && !joined.ends_with("SCAN m"),
            "the subquery must not scan messages:\n{joined}"
        );
    }

    #[tokio::test]
    async fn orphan_blob_sweeps_use_the_checkpoint_content_index() {
        let db = TempDb::new("plan-checkpoint-content-id");
        let store = db.open().await;
        let indexes = index_names(&store, "checkpoints").await;
        assert!(indexes
            .iter()
            .any(|name| name == "idx_checkpoints_content_id"));
        let plan = store
            .with(|conn| {
                let mut stmt = conn
                    .prepare(
                        "EXPLAIN QUERY PLAN DELETE FROM checkpoint_blobs
                         WHERE NOT EXISTS (SELECT 1 FROM checkpoints c WHERE c.content_id = checkpoint_blobs.id)",
                    )
                    .map_err(|e| db_err("blob plan", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(3))
                    .map_err(|e| db_err("blob plan rows", e))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| db_err("blob plan collect", e))
            })
            .await
            .expect("plan")
            .join("
");
        assert!(
            plan.contains("idx_checkpoints_content_id"),
            "blob cleanup must search checkpoints by content_id:
{plan}"
        );
    }

    #[tokio::test]
    async fn db_size_bytes_returns_a_plausible_value() {
        let db = TempDb::new("db-size");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        for i in 0..20 {
            store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
        }

        let size = store.db_size_bytes().await.expect("db size");
        let on_disk = std::fs::metadata(&db.path).expect("metadata").len();
        assert!(size > 0, "a populated database has a size");
        assert!(
            size >= on_disk,
            "the reported size includes the WAL ({size} < {on_disk})"
        );
        assert!(
            size < 64 * 1024 * 1024,
            "a fresh test database is not huge: {size}"
        );
    }

    #[tokio::test]
    async fn vacuum_and_incremental_vacuum_succeed_and_keep_the_data() {
        let db = TempDb::new("vacuum");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        for i in 0..200 {
            store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
        }
        let before = store.db_size_bytes().await.expect("size");

        store
            .incremental_vacuum()
            .await
            .expect("incremental vacuum");
        store.vacuum().await.expect("vacuum");

        let after = store.db_size_bytes().await.expect("size");
        assert!(after > 0);
        assert!(
            after <= before,
            "vacuum must not grow the file ({before} -> {after})"
        );
        assert_eq!(
            store
                .list_messages("s_1", 1000)
                .await
                .expect("messages")
                .len(),
            200,
            "vacuum preserves the rows"
        );
        assert!(store.integrity_check().await.expect("check").ok);
    }

    /// A panic inside a store closure must not brick the store.
    ///
    /// `with` runs caller-supplied code while holding the writer mutex. With
    /// `std::sync::Mutex` one panic poisoned it and every later call failed with
    /// "database mutex poisoned" for the rest of the process's life - a
    /// permanently dead server from one bad row, with no recovery path.
    #[tokio::test]
    async fn a_panicking_closure_does_not_poison_the_writer() {
        let db = TempDb::new("panic-no-poison");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        // The join error of a panicking closure is what a caller sees; the point
        // of the test is what happens *afterwards*.
        let panicked = store
            .with(|_conn| -> Result<()> { panic!("a tool panicked mid-query") })
            .await;
        assert!(panicked.is_err(), "the panic surfaces as an error");

        // The writer is still usable: no "poisoned" error, and the connection is
        // not stuck inside a transaction the panic left open.
        store
            .append_message(&new_message("s_1", "user", "after the panic"))
            .await
            .expect("the store recovers from a panicking closure");
        assert_eq!(
            store
                .list_messages("s_1", 10)
                .await
                .expect("messages")
                .len(),
            1
        );

        // Reads through the pool are unaffected too.
        assert_eq!(store.list_sessions().await.expect("sessions").len(), 1);
    }

    #[tokio::test]
    async fn a_second_open_on_the_same_data_dir_is_refused() {
        let db = TempDb::new("single-instance");
        let store = db.open().await;

        match SqliteStore::open(&db.path).await {
            Err(Error::Config(message)) => {
                assert!(
                    message.contains("another karucode instance"),
                    "the error must name the cause: {message}"
                );
                assert!(
                    message.contains("data directory"),
                    "the error must name the data directory: {message}"
                );
            }
            Err(other) => panic!("expected a Config error, got {other}"),
            Ok(_) => panic!("two stores must not share one data directory"),
        }

        // The lock is released when the store is dropped, so a restart works.
        drop(store);
        let reopened = SqliteStore::open(&db.path).await.expect("reopen");
        assert!(!reopened.instance_id().is_empty());
    }

    #[tokio::test]
    async fn recovery_marks_dead_instances_rows_and_spares_this_one() {
        let db = TempDb::new("instance-scoped-recovery");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        // Rows this instance owns: a live run in this process. Recovery must not
        // touch them, because this process is the one running them.
        store
            .create_run("r_mine", "s_1", "m", "p")
            .await
            .expect("run");
        store
            .upsert_agent(&agent("a_mine", "s_1", "running"))
            .await
            .expect("agent");

        // Rows a *previous* process left behind. The exclusive data-dir lock
        // guarantees that process is gone, so these are exactly what recovery
        // exists to clean up.
        let mine = store.instance_id().to_string();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO runs (id, session_id, status, model, prompt, started_at, instance_id)
                     VALUES ('r_theirs', 's_1', 'running', 'm', 'p', 1, 'dead-instance')",
                    [],
                )
                .map_err(|e| db_err("insert foreign run", e))?;
                conn.execute(
                    "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status,
                                         created_at, updated_at, instance_id)
                     VALUES ('a_theirs', 's_1', 'r', 'n', 'r', 't', 'running', 1, 1, 'dead-instance')",
                    [],
                )
                .map_err(|e| db_err("insert foreign agent", e))?;
                Ok(())
            })
            .await
            .expect("seed foreign rows");

        assert_eq!(store.mark_interrupted_runs().await.expect("runs"), 1);
        assert_eq!(store.mark_interrupted_agents().await.expect("agents"), 1);

        let runs = store.list_runs("s_1", 10).await.expect("runs");
        let mine_run = runs.iter().find(|r| r.id == "r_mine").expect("r_mine");
        let their_run = runs.iter().find(|r| r.id == "r_theirs").expect("r_theirs");
        assert_eq!(
            mine_run.status, "running",
            "this process's own live run must survive recovery"
        );
        assert_eq!(
            their_run.status, "interrupted",
            "a dead instance's leftover run must be recovered"
        );

        let agents = store.list_agents("s_1").await.expect("agents");
        let mine_agent = agents.iter().find(|a| a.id == "a_mine").expect("a_mine");
        let their_agent = agents
            .iter()
            .find(|a| a.id == "a_theirs")
            .expect("a_theirs");
        assert_eq!(mine_agent.status, "running");
        assert_eq!(their_agent.status, "interrupted");

        // The instance id is stamped on everything this store writes.
        let stamped: i64 = store
            .with(move |conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM runs WHERE instance_id = ?1",
                    params![mine],
                    |row| row.get(0),
                )
                .map_err(|e| db_err("count stamped", e))
            })
            .await
            .expect("count stamped");
        assert_eq!(stamped, 1);
    }

    #[tokio::test]
    async fn open_pragmas_are_actually_in_effect() {
        let db = TempDb::new("pragmas-verified");
        let store = db.open().await;

        let (mode, fk, timeout, auto_vacuum) = store
            .with(|conn| {
                conn.query_row(
                    "SELECT (SELECT * FROM pragma_journal_mode),
                            (SELECT * FROM pragma_foreign_keys),
                            (SELECT * FROM pragma_busy_timeout),
                            (SELECT * FROM pragma_auto_vacuum)",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
                .map_err(|e| db_err("pragmas", e))
            })
            .await
            .expect("pragmas");
        assert_eq!(mode.to_lowercase(), "wal");
        assert_eq!(fk, 1, "foreign_keys must really be on for cascades");
        assert_eq!(timeout, 5000);
        assert_eq!(
            auto_vacuum, 2,
            "a new database is created with auto_vacuum=INCREMENTAL"
        );

        // The read-only pool must see the same WAL database.
        let reader_mode: String = store
            .read(|conn| {
                conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .map_err(|e| db_err("reader journal_mode", e))
            })
            .await
            .expect("reader pragma");
        assert_eq!(reader_mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn a_healthy_database_passes_quick_check_and_integrity_check() {
        let db = TempDb::new("integrity");
        let store = db.open().await;
        // `open` ran quick_check before migrating.
        assert!(
            store.last_integrity_check().is_some(),
            "open records when it last checked the file"
        );

        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .append_message(&new_message("s_1", "user", "hi"))
            .await
            .expect("append");

        let report = store.integrity_check().await.expect("integrity check");
        assert!(report.ok, "a healthy database reports ok: {report:?}");
        assert!(report.problems.is_empty());
        assert!(report.checked_at > 0);
        assert!(store.last_integrity_check().expect("stamp") >= report.checked_at);
    }

    #[tokio::test]
    async fn hot_query_indexes_exist() {
        let db = TempDb::new("indexes");
        let store = db.open().await;

        let messages = index_names(&store, "messages").await;
        assert!(
            messages.iter().any(|n| n == "idx_messages_agent_id"),
            "list_agent_messages needs messages(agent_id, id): {messages:?}"
        );
        assert!(messages.iter().any(|n| n == "idx_messages_session_id"));

        let approvals = index_names(&store, "approvals").await;
        assert!(
            approvals.iter().any(|n| n == "idx_approvals_run_id"),
            "expire_pending_approvals needs approvals(run_id): {approvals:?}"
        );
        assert!(
            approvals
                .iter()
                .any(|n| n == "idx_approvals_session_status"),
            "list_pending_approvals needs approvals(session_id, status): {approvals:?}"
        );

        let sessions = index_names(&store, "sessions").await;
        assert!(
            sessions.iter().any(|n| n == "idx_sessions_updated_at"),
            "list_sessions orders by sessions.updated_at: {sessions:?}"
        );
    }

    #[tokio::test]
    async fn migration_from_v3_adds_instance_id_and_indexes() {
        let db = TempDb::new("migrate-v3");
        std::fs::create_dir_all(db.path.parent().expect("parent")).expect("data dir");
        {
            // A database written by the previous build: v1..v3, no instance_id.
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, 3);
            conn.pragma_update(None, "user_version", 3)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at)
                 VALUES ('s_old', 'Old', '/w', 1, 2)",
                [],
            )
            .expect("insert session");
            conn.execute(
                "INSERT INTO runs (id, session_id, status, model, prompt, started_at)
                 VALUES ('r_old', 's_old', 'running', 'm', 'p', 1)",
                [],
            )
            .expect("insert run");
            conn.execute(
                "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status,
                                     created_at, updated_at)
                 VALUES ('a_old', 's_old', 'r_old', 'n', 'r', 't', 'running', 1, 1)",
                [],
            )
            .expect("insert agent");
        }

        let store = db.open().await;
        let version: i64 = store
            .with(|conn| {
                conn.query_row("PRAGMA user_version", [], |row| row.get(0))
                    .map_err(|e| db_err("user_version", e))
            })
            .await
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION);
        assert!(
            version >= 4,
            "v4 is the migration that added instance_id (this fixture starts at v3)"
        );

        // Pre-v4 rows keep the empty instance id. That is not this process's id,
        // so recovery treats them as a dead process's leftovers and marks them
        // interrupted — which is correct: they were `running` when the old
        // binary died.
        let legacy: String = store
            .with(|conn| {
                conn.query_row(
                    "SELECT instance_id FROM runs WHERE id = 'r_old'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| db_err("legacy instance_id", e))
            })
            .await
            .expect("legacy instance_id");
        assert_eq!(legacy, "");
        assert_eq!(
            store.mark_interrupted_runs().await.expect("recovery"),
            1,
            "a pre-v4 running row belongs to a dead process and must be recovered"
        );

        // The upgraded database is fully usable.
        store
            .create_session("s_new", "New", "/w", None)
            .await
            .expect("create after upgrade");
        store
            .append_message(&new_message("s_new", "user", "hi"))
            .await
            .expect("append after upgrade");
        assert!(store.integrity_check().await.expect("check").ok);
        assert!(index_names(&store, "messages")
            .await
            .iter()
            .any(|n| n == "idx_messages_agent_id"));
    }

    // A multi-thread runtime, not `#[tokio::test]`: the point of the test is
    // that a blocked read on one worker does not stall the write on another.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_long_read_does_not_block_a_concurrent_write() {
        let db = TempDb::new("read-write-concurrency");
        let store = Arc::new(db.open().await);
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        for i in 0..50 {
            store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
        }

        let (started_tx, mut started_rx) = tokio::sync::mpsc::channel::<()>(1);
        let (release_tx, mut release_rx) = tokio::sync::mpsc::channel::<()>(1);
        let reader = {
            let store = store.clone();
            tokio::spawn(async move {
                store
                    .read(move |conn| {
                        let n: i64 = conn
                            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
                            .map_err(|e| db_err("count", e))?;
                        started_tx.blocking_send(()).ok();
                        // Hold the read connection until the test releases it.
                        release_rx.blocking_recv();
                        Ok(n)
                    })
                    .await
            })
        };

        tokio::time::timeout(Duration::from_secs(10), started_rx.recv())
            .await
            .expect("the read must start")
            .expect("reader signalled");

        // With one connection behind one mutex this write would queue behind the
        // read and time out; with the read-only pool it commits immediately.
        tokio::time::timeout(
            Duration::from_secs(5),
            store.append_message(&new_message("s_1", "user", "written during the read")),
        )
        .await
        .expect("a write must not queue behind a long read")
        .expect("append during read");

        release_tx.send(()).await.ok();
        let read = reader.await.expect("reader task").expect("read");
        assert_eq!(read, 50, "the read saw the pre-write snapshot");
        assert_eq!(
            store
                .list_messages("s_1", 100)
                .await
                .expect("messages")
                .len(),
            51
        );
    }

    // ---- rewind ----------------------------------------------------------

    /// A conversation of three prompt/answer pairs, returning the message ids in
    /// order so a test can name a rewind point.
    async fn conversation(store: &SqliteStore, session_id: &str) -> Vec<i64> {
        let mut ids = Vec::new();
        for turn in 0..3 {
            ids.push(
                store
                    .append_message(&new_message(session_id, "user", &format!("prompt {turn}")))
                    .await
                    .expect("append user"),
            );
            ids.push(
                store
                    .append_message(&new_message(
                        session_id,
                        "assistant",
                        &format!("answer {turn}"),
                    ))
                    .await
                    .expect("append assistant"),
            );
        }
        ids
    }

    #[tokio::test]
    async fn rewinding_hides_the_tail_and_keeps_the_chosen_prompt() {
        let db = TempDb::new("rewind-hide");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let ids = conversation(&store, "s_1").await;
        assert_eq!(ids.len(), 6);

        // Rewind to the *second* prompt: everything after it disappears, and the
        // prompt itself survives because the user wants to rewrite or retry it.
        let point = store.rewind_point("s_1", ids[2]).await.expect("point");
        assert_eq!(point.user_message_id, ids[2]);
        assert_eq!(point.keep_through, ids[2]);
        assert_eq!(point.prompt, "prompt 1");
        assert_eq!(point.messages_removed, 3, "answer 1, prompt 2, answer 2");

        assert_eq!(
            store
                .mark_messages_rewound("s_1", point.keep_through)
                .await
                .expect("rewind"),
            3
        );

        let visible = store.list_messages("s_1", 100).await.expect("list");
        assert_eq!(
            visible
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["prompt 0", "answer 0", "prompt 1"],
            "the tail is hidden and the chosen prompt is the last visible message"
        );
        assert!(
            visible.iter().all(|m| m.rewound_at.is_none()),
            "a visible row must not carry the mark"
        );

        // The rows still exist: a rewind is a mark, not a delete.
        assert_eq!(count(&store, "SELECT COUNT(*) FROM messages").await, 6);

        // The session count the rail shows follows the visible transcript.
        let summary = store.list_sessions().await.expect("sessions");
        assert_eq!(summary[0].message_count, 3);

        // Rewinding the same point again is a no-op, not a second hide.
        assert_eq!(
            store
                .mark_messages_rewound("s_1", point.keep_through)
                .await
                .expect("second rewind"),
            0
        );
    }

    #[tokio::test]
    async fn rewinding_an_assistant_turn_resolves_to_its_prompt() {
        let db = TempDb::new("rewind-normalise");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let ids = conversation(&store, "s_1").await;

        // Picking the answer means "the turn that produced this answer", so the
        // cut lands before the answer and keeps its prompt.
        let point = store.rewind_point("s_1", ids[3]).await.expect("point");
        assert_eq!(point.user_message_id, ids[2]);
        assert_eq!(point.prompt, "prompt 1");

        // A tool result behaves the same way.
        let tool_id = store
            .append_message(&new_message("s_1", "tool", "result"))
            .await
            .expect("tool");
        let point = store.rewind_point("s_1", tool_id).await.expect("point");
        assert_eq!(point.user_message_id, ids[4], "the newest prompt owns it");
    }

    #[tokio::test]
    async fn a_rewind_point_must_belong_to_the_session_it_names() {
        let db = TempDb::new("rewind-scope");
        let store = db.open().await;
        store
            .create_session("s_1", "one", "/w", None)
            .await
            .expect("session one");
        store
            .create_session("s_2", "two", "/w", None)
            .await
            .expect("session two");
        let other = store
            .append_message(&new_message("s_2", "user", "elsewhere"))
            .await
            .expect("message");

        // Ids are global, so a cross-session id is a real mistake and must be
        // refused rather than rewinding the wrong conversation.
        let err = store.rewind_point("s_1", other).await.unwrap_err();
        assert!(err.to_string().contains("does not belong"), "{err}");
        assert!(store.rewind_point("s_1", 9999).await.is_err());
    }

    #[tokio::test]
    async fn a_rewind_before_any_prompt_is_refused() {
        let db = TempDb::new("rewind-no-prompt");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let assistant = store
            .append_message(&new_message("s_1", "assistant", "unsolicited"))
            .await
            .expect("assistant");
        let err = store.rewind_point("s_1", assistant).await.unwrap_err();
        assert!(err.to_string().contains("nothing to rewind to"), "{err}");
    }

    fn checkpoint(session_id: &str, path: &str, content: &str, message_id: i64) -> FileCheckpoint {
        FileCheckpoint {
            id: 0,
            session_id: session_id.to_string(),
            run_id: "r_1".to_string(),
            agent_id: None,
            call_id: "call_1".to_string(),
            tool: "edit_file".to_string(),
            path: path.to_string(),
            existed: true,
            content: content.to_string(),
            capture_failed: false,
            message_id,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn checkpoints_round_trip_identically_content_sharing_their_bytes() {
        let db = TempDb::new("checkpoint-roundtrip");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");

        store
            .record_checkpoint(&checkpoint("s_1", "a.rs", "fn a() {}", 10))
            .await
            .expect("first");
        store
            .record_checkpoint(&checkpoint("s_1", "b.rs", "fn a() {}", 11))
            .await
            .expect("second");
        store
            .record_checkpoint(&checkpoint("s_1", "empty.rs", "", 12))
            .await
            .expect("empty file");

        let rows = store.list_checkpoints("s_1", 0).await.expect("list");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].content, "fn a() {}");
        assert_eq!(rows[1].content, "fn a() {}", "identical bytes survive");
        assert_eq!(rows[2].content, "", "an empty file is empty, not missing");
        assert_eq!(rows[2].path, "empty.rs");

        // Identical content is stored once.
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            2,
            "one blob for the shared bytes, one for the empty file"
        );

        // The region filter is what a rewind uses: strictly after the message.
        let after = store.list_checkpoints("s_1", 10).await.expect("after");
        assert_eq!(
            after.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
            vec!["b.rs", "empty.rs"]
        );
    }

    #[tokio::test]
    async fn checkpoint_metadata_and_individual_content_are_lazy() {
        let db = TempDb::new("checkpoint-lazy");
        let store = db.open().await;
        store.create_session("s_1", "t", "/w", None).await.unwrap();
        let large = "x".repeat(1024 * 1024);
        let first = store
            .record_checkpoint(&checkpoint("s_1", "a", &large, 10))
            .await
            .unwrap();
        let second = store
            .record_checkpoint(&checkpoint("s_1", "a", &large, 11))
            .await
            .unwrap();
        let empty = store
            .record_checkpoint(&checkpoint("s_1", "empty", "", 12))
            .await
            .unwrap();
        let mut failed = checkpoint("s_1", "failed", "", 13);
        failed.capture_failed = true;
        let failed_id = store.record_checkpoint(&failed).await.unwrap();
        let rows = store.list_checkpoint_metadata("s_1", 10).await.unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![second, empty, failed_id]
        );
        assert!(rows.iter().all(|r| r.content.is_empty()));
        assert!(rows[2].capture_failed);
        assert!(store
            .checkpoint_content_bounded(first, large.len() - 1)
            .await
            .is_err());
        assert_eq!(
            store
                .checkpoint_content_bounded(first, large.len())
                .await
                .unwrap(),
            Some(large.clone())
        );
        assert_eq!(store.checkpoint_content_bounded(-1, 0).await.unwrap(), None);
        assert_eq!(store.checkpoint_content(first).await.unwrap(), Some(large));
        assert_eq!(
            store.checkpoint_content(empty).await.unwrap(),
            Some(String::new())
        );
        assert_eq!(store.checkpoint_content(failed_id).await.unwrap(), None);
        assert_eq!(store.checkpoint_content(-1).await.unwrap(), None);
    }

    #[tokio::test]
    async fn deleting_a_checkpoint_releases_an_unshared_blob_only() {
        let db = TempDb::new("checkpoint-delete");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let shared = store
            .record_checkpoint(&checkpoint("s_1", "a.rs", "shared", 1))
            .await
            .expect("first");
        store
            .record_checkpoint(&checkpoint("s_1", "b.rs", "shared", 2))
            .await
            .expect("second");
        let unique = store
            .record_checkpoint(&checkpoint("s_1", "c.rs", "unique", 3))
            .await
            .expect("third");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            2
        );

        // Deleting one of two rows that share a payload keeps the payload.
        store
            .delete_checkpoint(shared)
            .await
            .expect("delete shared");
        assert_eq!(count(&store, "SELECT COUNT(*) FROM checkpoints").await, 2);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            2
        );

        // Deleting the last user of a payload frees it.
        store
            .delete_checkpoint(unique)
            .await
            .expect("delete unique");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            1
        );
    }

    /// Deleting a session must take its file before-images with it.
    ///
    /// `checkpoint_blobs` has no foreign key in either direction - the only link
    /// runs `checkpoints.content_id -> checkpoint_blobs.id` - so the session
    /// cascade used to leave every payload on disk: measured 72 blobs / 12.7 MB
    /// for one deleted session, the opposite of what deleting a private session
    /// promises.
    #[tokio::test]
    async fn deleting_a_session_releases_its_checkpoint_blobs() {
        let db = TempDb::new("delete-session-blobs");
        let store = db.open().await;
        store
            .create_session("s_1", "doomed", "/w", None)
            .await
            .expect("session");
        store
            .create_session("s_kept", "kept", "/w", None)
            .await
            .expect("session");
        store
            .record_checkpoint(&checkpoint("s_1", "a.rs", "private bytes", 1))
            .await
            .expect("a");
        store
            .record_checkpoint(&checkpoint("s_1", "b.rs", "other private bytes", 2))
            .await
            .expect("b");
        // A payload both sessions share must survive the delete.
        store
            .record_checkpoint(&checkpoint("s_1", "shared.rs", "shared bytes", 3))
            .await
            .expect("shared in s_1");
        store
            .record_checkpoint(&checkpoint("s_kept", "shared.rs", "shared bytes", 4))
            .await
            .expect("shared in s_kept");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            3
        );

        store.delete_session("s_1").await.expect("delete");

        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoints").await,
            1,
            "the cascade removed s_1's journal rows"
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            1,
            "only the payload another session still references survives"
        );
        assert_eq!(
            store
                .list_checkpoints("s_kept", 0)
                .await
                .expect("list")
                .first()
                .expect("kept row")
                .content,
            "shared bytes",
            "the surviving blob is still readable"
        );
    }

    #[tokio::test]
    async fn an_uncapturable_checkpoint_keeps_its_place_but_no_bytes() {
        let db = TempDb::new("checkpoint-uncapturable");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let mut row = checkpoint("s_1", "huge.bin", "", 1);
        row.capture_failed = true;
        store.record_checkpoint(&row).await.expect("record");

        let rows = store.list_checkpoints("s_1", 0).await.expect("list");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].capture_failed);
        assert!(
            !rows[0].undoable(),
            "a missing before-image cannot be undone"
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            0
        );
    }

    #[tokio::test]
    async fn a_created_file_is_a_checkpoint_that_did_not_exist() {
        let db = TempDb::new("checkpoint-created");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let mut row = checkpoint("s_1", "new.rs", "", 1);
        row.existed = false;
        store.record_checkpoint(&row).await.expect("record");

        let rows = store.list_checkpoints("s_1", 0).await.expect("list");
        assert!(!rows[0].existed, "undoing a create removes the file");
        assert!(rows[0].undoable());
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            0,
            "there is no before-image to store"
        );
    }

    #[tokio::test]
    async fn retention_sweeps_journal_rows_whose_message_is_gone() {
        let db = TempDb::new("checkpoint-retention");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let keep = store.retention_keep_messages();
        let mut ids = Vec::new();
        for i in 0..(keep + 10) {
            let id = store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
            ids.push(id);
        }
        // One journal row on a message that will be pruned, one on a survivor.
        store
            .record_checkpoint(&checkpoint("s_1", "old.rs", "old", ids[0]))
            .await
            .expect("old checkpoint");
        let survivor = *ids.last().expect("last");
        store
            .record_checkpoint(&checkpoint("s_1", "new.rs", "new", survivor))
            .await
            .expect("new checkpoint");

        age_messages(&store, "s_1", 30).await;
        store.prune_messages_older_than(30).await.expect("prune");

        let rows = store.list_checkpoints("s_1", 0).await.expect("list");
        assert_eq!(
            rows.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(),
            vec!["new.rs"],
            "the journal must not outlive the transcript it describes"
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            1,
            "and its payload goes with it"
        );
    }

    /// Retention keeps an unbound row only while its run is running. Unbound
    /// rows are never selected by rewind: after a dead writer there is no safe
    /// transcript position at which to replay the before-image.
    #[tokio::test]
    async fn retention_keeps_an_in_flight_checkpoint_that_is_not_bound_yet() {
        let db = TempDb::new("checkpoint-in-flight");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .create_run("r_1", "s_1", "m", "p")
            .await
            .expect("active run");
        store
            .create_run("r_stale", "s_1", "m", "p")
            .await
            .expect("stale run");
        store
            .finish_run("r_stale", RunStatus::Interrupted, None)
            .await
            .expect("stale run finished");
        let keep = store.retention_keep_messages();
        for i in 0..(keep + 10) {
            store
                .append_message(&new_message("s_1", "user", &format!("m{i}")))
                .await
                .expect("append");
        }
        let in_flight = store
            .record_checkpoint(&checkpoint("s_1", "working.rs", "before", 0))
            .await
            .expect("in-flight checkpoint");
        let mut stale = checkpoint("s_1", "stale.rs", "crash residue", 0);
        stale.run_id = "r_stale".to_string();
        let stale = store
            .record_checkpoint(&stale)
            .await
            .expect("stale checkpoint");
        assert!(
            store
                .list_checkpoint_metadata("s_1", 0)
                .await
                .expect("rewind metadata")
                .is_empty(),
            "unbound metadata cannot be placed on the transcript timeline"
        );
        assert!(
            store
                .list_checkpoints("s_1", 0)
                .await
                .expect("rewind rows")
                .is_empty(),
            "stale before-images must never reach rewind replay"
        );
        age_messages(&store, "s_1", 30).await;

        store.prune_messages_older_than(30).await.expect("prune");

        assert_eq!(
            store
                .checkpoint_content(in_flight)
                .await
                .expect("active content"),
            Some("before".to_string()),
            "the active run's payload survives with it"
        );
        assert_eq!(
            store
                .checkpoint_content(stale)
                .await
                .expect("stale content"),
            None,
            "a dead writer's unbound checkpoint is swept"
        );

        // Once the runtime binds it to a kept message it is a normal journal row
        // again, and a later prune keeps it for the same reason.
        let survivor = *store
            .list_messages("s_1", 10_000)
            .await
            .expect("messages")
            .last()
            .map(|m| &m.id)
            .expect("a survivor");
        store
            .set_checkpoint_message_id(in_flight, survivor)
            .await
            .expect("bind");
        store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoints").await,
            1,
            "a bound row whose message survives is kept"
        );
    }

    #[tokio::test]
    async fn checkpoint_binding_is_atomic_and_retries_only_unbound_rows() {
        let db = TempDb::new("checkpoint-bind-atomic");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let first = store
            .record_checkpoint(&checkpoint("s_1", "first", "one", 0))
            .await
            .expect("first");
        let second = store
            .record_checkpoint(&checkpoint("s_1", "second", "two", 0))
            .await
            .expect("second");
        store
            .with(|conn| {
                conn.execute_batch(
                    "CREATE TRIGGER reject_checkpoint_bind
                     BEFORE UPDATE OF message_id ON checkpoints
                     WHEN NEW.path = 'second' AND NEW.message_id = 42
                     BEGIN SELECT RAISE(ABORT, 'injected checkpoint bind failure'); END;",
                )
                .map_err(|e| db_err("install checkpoint trigger", e))
            })
            .await
            .expect("trigger");
        let error = store.bind_checkpoints(&[first, second], 42).await;
        assert!(error.is_err(), "the injected second-row write must fail");
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM checkpoints WHERE message_id = 0",
            )
            .await,
            2,
            "the first row rolled back with the failing second row"
        );

        store
            .with(|conn| {
                conn.execute_batch("DROP TRIGGER reject_checkpoint_bind")
                    .map_err(|e| db_err("drop checkpoint trigger", e))
            })
            .await
            .expect("drop trigger");
        assert_eq!(
            store
                .bind_checkpoints(&[first, second], 42)
                .await
                .expect("retry"),
            2
        );
        let third = store
            .record_checkpoint(&checkpoint("s_1", "third", "three", 0))
            .await
            .expect("third");
        assert_eq!(
            store
                .bind_checkpoints(&[first, third], 43)
                .await
                .expect("partially old retry"),
            1
        );
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM checkpoints WHERE message_id = 42",
            )
            .await,
            2,
            "a retry must not rebind completed rows to a later message"
        );
        let listed = store.list_checkpoints("s_1", 42).await.expect("listed");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, third);
        assert!(store.bind_checkpoints(&[], 0).await.expect("empty") == 0);
        assert!(store.bind_checkpoints(&[third], 0).await.is_err());
    }

    #[tokio::test]
    async fn last_rewind_at_reports_the_newest_mark() {
        let db = TempDb::new("rewind-timestamp");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        let ids = conversation(&store, "s_1").await;
        assert_eq!(store.last_rewind_at("s_1").await.expect("none"), None);

        store
            .mark_messages_rewound("s_1", ids[2])
            .await
            .expect("rewind");
        let at = store
            .last_rewind_at("s_1")
            .await
            .expect("read")
            .expect("some");
        assert!(at > 0);

        // A message appended after the rewind is visible and does not move the
        // mark.
        store
            .append_message(&new_message("s_1", "user", "rewritten"))
            .await
            .expect("append");
        assert_eq!(store.last_rewind_at("s_1").await.unwrap(), Some(at));
    }

    // ---- token accounting -------------------------------------------------

    fn usage(
        session_id: &str,
        run_id: &str,
        agent_id: Option<&str>,
        input: u32,
        output: u32,
        cached: Option<u32>,
    ) -> UsageRecord {
        UsageRecord {
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
            agent_id: agent_id.map(str::to_string),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
        }
    }

    #[tokio::test]
    async fn usage_totals_are_summed_per_agent() {
        let db = TempDb::new("usage-totals");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .create_session("s_2", "other", "/w", None)
            .await
            .expect("session");

        // Two main-agent calls, one of which the endpoint reported no cache
        // figure for: the ratio must not treat that as a 0% hit.
        store
            .record_usage(&usage("s_1", "r_1", None, 100, 10, Some(90)))
            .await
            .expect("usage");
        store
            .record_usage(&usage("s_1", "r_1", None, 200, 20, None))
            .await
            .expect("usage");
        // One subagent with a single call, and one with none recorded at all.
        store
            .record_usage(&usage("s_1", "r_1", Some("a_1"), 50, 5, Some(10)))
            .await
            .expect("usage");
        // Another session's call must not leak into s_1.
        store
            .record_usage(&usage("s_2", "r_9", None, 1_000, 100, Some(0)))
            .await
            .expect("usage");

        let totals = store.usage_totals("s_1").await.expect("totals");
        assert_eq!(totals.len(), 2, "main agent plus one subagent");

        let main = totals.get(&None).expect("main agent").to_owned();
        assert_eq!(main.calls, 2);
        assert_eq!(main.input_tokens, 300);
        assert_eq!(main.output_tokens, 30);
        assert_eq!(main.cached_tokens, 90);
        assert_eq!(main.cached_known_calls, 1, "only one call reported a cache");
        assert_eq!(
            main.last_input_tokens, 200,
            "the newest call's prompt size is the window fill"
        );
        let ratio = main.cache_hit_ratio().expect("ratio");
        assert!((ratio - 0.3).abs() < 1e-9, "ratio is 90/300, got {ratio}");

        let child = totals.get(&Some("a_1".to_string())).expect("subagent");
        assert_eq!(child.calls, 1);
        assert_eq!(child.input_tokens, 50);
        assert_eq!(child.output_tokens, 5);
        assert_eq!(child.cached_tokens, 10);
        assert_eq!(child.last_input_tokens, 50);

        // A session with no recorded call answers "nothing", not an error.
        assert!(store
            .usage_totals("s_missing")
            .await
            .expect("totals")
            .is_empty());
    }

    #[tokio::test]
    async fn session_stats_count_the_whole_session_not_a_page() {
        let db = TempDb::new("session-stats");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .create_session("s_2", "other", "/w", None)
            .await
            .expect("session");

        // Two main-agent turns: a prompt, then an answer that calls two tools and
        // whose results come back as tool rows.
        store
            .append_message(&new_message("s_1", "user", "do the thing"))
            .await
            .expect("user");
        let mut assistant = new_message("s_1", "assistant", "working");
        assistant.run_id = Some("r_1".to_string());
        assistant.tool_calls = vec![
            ToolCall {
                id: "c1".to_string(),
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
            ToolCall {
                id: "c2".to_string(),
                name: "run_command".to_string(),
                arguments: "{}".to_string(),
            },
        ];
        store.append_message(&assistant).await.expect("assistant");
        for (call, name) in [("c1", "read_file"), ("c2", "run_command")] {
            let mut tool = new_message("s_1", "tool", "ok");
            tool.tool_call_id = Some(call.to_string());
            tool.name = Some(name.to_string());
            store.append_message(&tool).await.expect("tool");
        }

        // A subagent's own turn, which must be counted separately from the main
        // conversation.
        let mut child = new_message("s_1", "assistant", "child work");
        child.agent_id = Some("a_1".to_string());
        child.tool_calls = vec![ToolCall {
            id: "c3".to_string(),
            name: "read_file".to_string(),
            arguments: "{}".to_string(),
        }];
        store.append_message(&child).await.expect("child");
        // Another session's rows must not be counted.
        store
            .append_message(&new_message("s_2", "user", "unrelated"))
            .await
            .expect("other");

        // Two finished runs (so working time is their sum) and one still live.
        store.create_run("r_1", "s_1", "m", "p").await.expect("run");
        store
            .finish_run("r_1", RunStatus::Completed, None)
            .await
            .expect("finish");
        store.create_run("r_2", "s_1", "m", "p").await.expect("run");
        store
            .finish_run("r_2", RunStatus::Failed, Some("boom"))
            .await
            .expect("finish");
        store.create_run("r_3", "s_1", "m", "p").await.expect("run");
        // Give the finished runs a measurable duration: `finish_run` stamps now,
        // which can land in the same millisecond. Add an in-flight run's start.
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE runs SET started_at = 1000, finished_at = 1500 WHERE id = 'r_1'",
                    [],
                )
                .map_err(|e| db_err("age r_1", e))?;
                conn.execute(
                    "UPDATE runs SET started_at = 2000, finished_at = 2400 WHERE id = 'r_2'",
                    [],
                )
                .map_err(|e| db_err("age r_2", e))?;
                Ok(())
            })
            .await
            .expect("age runs");

        let stats = store.session_stats("s_1").await.expect("stats");
        assert_eq!(stats.messages_total, 5, "user, assistant, 2 tools, child");
        assert_eq!(stats.messages_main, 4);
        assert_eq!(stats.messages_subagent, 1);
        assert_eq!(stats.messages_by_role.get("tool"), Some(&2));
        assert_eq!(stats.messages_main_by_role.get("user"), Some(&1));
        assert_eq!(
            stats.messages_main_by_role.get("tool"),
            Some(&2),
            "the child made no tool result row"
        );

        assert_eq!(stats.tool_calls_total, 3, "two main + one child call");
        assert_eq!(stats.tool_calls.get("read_file"), Some(&2));
        assert_eq!(stats.tool_calls.get("run_command"), Some(&1));

        assert_eq!(stats.runs_total, 3);
        assert_eq!(stats.runs_by_status.get("completed"), Some(&1));
        assert_eq!(stats.runs_by_status.get("failed"), Some(&1));
        assert_eq!(stats.runs_by_status.get("running"), Some(&1));
        assert_eq!(
            stats.working_ms, 900,
            "500ms + 400ms; the live run's duration is not counted"
        );

        // A session with nothing in it answers zeros rather than an error.
        let empty = store.session_stats("s_missing").await.expect("stats");
        assert_eq!(empty, SessionStats::default());
    }

    #[tokio::test]
    async fn usage_without_a_cache_figure_reports_no_ratio() {
        let db = TempDb::new("usage-no-cache");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .record_usage(&usage("s_1", "r_1", None, 40, 4, None))
            .await
            .expect("usage");

        let totals = store.usage_totals("s_1").await.expect("totals");
        let main = totals.get(&None).expect("main agent");
        assert_eq!(main.calls, 1);
        assert_eq!(main.cached_known_calls, 0);
        assert!(
            main.cache_hit_ratio().is_none(),
            "unknown must not be reported as a 0% hit rate"
        );
    }

    #[tokio::test]
    async fn deleting_a_session_removes_its_usage_rows() {
        let db = TempDb::new("usage-cascade");
        let store = db.open().await;
        store
            .create_session("s_1", "t", "/w", None)
            .await
            .expect("session");
        store
            .record_usage(&usage("s_1", "r_1", None, 10, 1, Some(5)))
            .await
            .expect("usage");

        store.delete_session("s_1").await.expect("delete");
        // The rows go with the session rather than outliving it as an orphan
        // measurement attributed to an id nothing can resolve any more.
        let count: i64 = store
            .with(|conn| {
                conn.query_row("SELECT COUNT(*) FROM usage_calls", [], |row| row.get(0))
                    .map_err(|e| db_err("count usage", e))
            })
            .await
            .expect("count");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn a_v7_database_migrates_forward_through_v8_and_v9_without_losing_anything() {
        let db = TempDb::new("migrate-v7-forward");
        std::fs::create_dir_all(&db.dir).expect("dir");
        {
            // Build a real v7 database. It has to be written step by step, not by
            // opening a current store and taking it back: `user_version = 7` on a
            // table that already has v9's columns is the *restore* shape, which is
            // a different test.
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, 7);
            conn.pragma_update(None, "user_version", 7)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at)
                 VALUES ('s_1', 'Kept', '/w', 1, 2)",
                [],
            )
            .expect("insert session");
            conn.execute(
                "INSERT INTO messages (session_id, role, content, created_at)
                 VALUES ('s_1', 'user', 'hello', 1)",
                [],
            )
            .expect("insert message");
            conn.execute(
                "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status,
                                     created_at, updated_at)
                 VALUES ('a_1', 's_1', 'r_1', 'n', 'r', 't', 'completed', 1, 1)",
                [],
            )
            .expect("insert agent");
        }

        let store = db.open().await;
        let session = store.get_session("s_1").await.unwrap().expect("session");
        assert_eq!(session.title, "Kept");
        assert_eq!(
            store
                .list_messages("s_1", 10)
                .await
                .expect("messages")
                .len(),
            1
        );
        // v9's columns arrived with their defaults, so an agent written by v7
        // reads back as "a blocking spawn that never reported".
        let agent = store
            .list_agents("s_1")
            .await
            .expect("agents")
            .into_iter()
            .next()
            .expect("the v7 agent");
        assert!(!agent.background);
        assert_eq!(agent.note, "");
        assert_eq!(agent.progress, None);
        // And v8's table is usable straight away.
        store
            .record_usage(&usage("s_1", "r_1", None, 7, 1, None))
            .await
            .expect("usage");
        assert_eq!(store.usage_totals("s_1").await.expect("totals").len(), 1);
        assert_eq!(version_of(&store).await, SCHEMA_VERSION);
    }

    /// The `.dump`-restore regression: `sqlite3 db .dump` does not carry
    /// `user_version`, so restoring a dump into a fresh file yields the *current*
    /// schema with version 0. Migrations replay from v1 over tables that already
    /// have every column, and used to die with "duplicate column name" -- a
    /// restored database that could never be opened again.
    #[tokio::test]
    async fn a_v20_database_gains_the_checkpoint_blob_index_without_losing_rows() {
        let db = TempDb::new("migrate-v20-checkpoint-index");
        std::fs::create_dir_all(&db.dir).expect("dir");
        {
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, 20);
            conn.pragma_update(None, "user_version", 20)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id,title,workspace,created_at,updated_at,control) VALUES ('s','S','/w',1,1,0)",
                [],
            )
            .expect("session");
            conn.execute(
                "INSERT INTO checkpoint_blobs (id,sha,data) VALUES (7,'sha',x'61')",
                [],
            )
            .expect("blob");
            conn.execute(
                "INSERT INTO checkpoints (id,session_id,run_id,path,existed,content_id,capture_failed,message_id,created_at)
                 VALUES (9,'s','r','old',1,7,0,5,1)",
                [],
            )
            .expect("checkpoint");
            let index: Option<String> = conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_checkpoints_content_id'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .expect("index before");
            assert!(index.is_none());
        }
        let store = db.open().await;
        assert_eq!(version_of(&store).await, SCHEMA_VERSION);
        assert!(index_names(&store, "checkpoints")
            .await
            .contains(&"idx_checkpoints_content_id".to_string()));
        let checkpoint = store.checkpoint_content(9).await.expect("content");
        assert_eq!(checkpoint.as_deref(), Some("a"));

        drop(store);
        let conn = rusqlite::Connection::open(&db.path).expect("replay open");
        conn.pragma_update(None, "user_version", 0)
            .expect("simulate dump restore");
        drop(conn);
        let replayed = db.open().await;
        assert_eq!(version_of(&replayed).await, SCHEMA_VERSION);
        assert_eq!(
            replayed
                .checkpoint_content(9)
                .await
                .expect("content")
                .as_deref(),
            Some("a"),
            "the idempotent index migration preserves checkpoint payloads"
        );
    }

    #[tokio::test]
    async fn a_restored_dump_with_a_stale_version_opens_and_keeps_its_rows() {
        let db = TempDb::new("restore-dump");
        std::fs::create_dir_all(&db.dir).expect("dir");
        {
            // Exactly what a restore produces: a full current schema, rows in
            // every table, and `user_version = 0`.
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, SCHEMA_VERSION);
            conn.pragma_update(None, "user_version", 0)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, project_id, model, created_at,
                                       updated_at)
                 VALUES ('s_1', 'Restored', '/w', NULL, NULL, 1, 2)",
                [],
            )
            .expect("insert session");
            conn.execute(
                "INSERT INTO messages (session_id, role, content, images, created_at)
                 VALUES ('s_1', 'user', 'hello', '[]', 1)",
                [],
            )
            .expect("insert message");
            conn.execute(
                "INSERT INTO runs (id, session_id, status, started_at, instance_id)
                 VALUES ('r_1', 's_1', 'completed', 1, '')",
                [],
            )
            .expect("insert run");
            conn.execute(
                "INSERT INTO agents (id, session_id, parent_run_id, name, role, task, status,
                                     created_at, updated_at, instance_id, background, note,
                                     progress)
                 VALUES ('a_1', 's_1', 'r_1', 'n', 'r', 't', 'completed', 1, 1, '', 1,
                         'waiting on the build', 0.5)",
                [],
            )
            .expect("insert agent");
        }

        // The whole point: this used to be a hard error.
        let store = db.open().await;
        assert_eq!(
            version_of(&store).await,
            SCHEMA_VERSION,
            "the replay must stamp the version rather than fail"
        );

        // And nothing was lost or reset by the replay.
        let session = store.get_session("s_1").await.unwrap().expect("session");
        assert_eq!(session.title, "Restored");
        assert_eq!(
            store
                .list_messages("s_1", 10)
                .await
                .expect("messages")
                .len(),
            1
        );
        let agent = store
            .list_agents("s_1")
            .await
            .expect("agents")
            .into_iter()
            .next()
            .expect("agent");
        assert!(agent.background, "a replayed ADD COLUMN keeps the data");
        assert_eq!(agent.note, "waiting on the build");
        assert_eq!(agent.progress, Some(0.5));

        // A second open is a no-op, not another replay. (The first store must be
        // dropped: the data directory is single-instance by design.)
        drop(store);
        let reopened = SqliteStore::open(&db.path).await.expect("reopen");
        assert_eq!(version_of(&reopened).await, SCHEMA_VERSION);
        assert_eq!(
            reopened
                .list_messages("s_1", 10)
                .await
                .expect("messages")
                .len(),
            1
        );
    }

    /// `add_column_if_missing` interpolates table names into DDL, which SQLite
    /// cannot parameterise. Every name it can see is a literal in
    /// [`ADDED_COLUMNS`]; this keeps a typo from becoming a column that is
    /// silently skipped on a fresh database.
    #[tokio::test]
    async fn every_added_column_names_a_real_table_and_column() {
        let db = TempDb::new("added-columns");
        let store = db.open().await;
        let tables: Vec<String> = store
            .with(|conn| {
                let mut stmt = conn
                    .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                    .map_err(|e| db_err("master prepare", e))?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| db_err("master rows", e))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| db_err("master collect", e))
            })
            .await
            .expect("tables");
        for table in SCHEMA_TABLES {
            assert!(
                tables.iter().any(|t| t == table),
                "SCHEMA_TABLES names {table}, which no migration creates: {tables:?}"
            );
        }
        for (version, table, column, _) in ADDED_COLUMNS {
            assert!(version <= &SCHEMA_VERSION, "v{version} is in the future");
            let columns: Vec<String> = store
                .with(move |conn| {
                    let mut stmt = conn
                        .prepare(&format!("PRAGMA table_info({table})"))
                        .map_err(|e| db_err("table_info prepare", e))?;
                    let rows = stmt
                        .query_map([], |row| row.get::<_, String>(1))
                        .map_err(|e| db_err("table_info rows", e))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|e| db_err("table_info collect", e))
                })
                .await
                .expect("table_info");
            assert!(
                columns.iter().any(|c| c == column),
                "ADDED_COLUMNS claims {table}.{column} (v{version}), but a migrated \
                 database does not have it: {columns:?}"
            );
        }
    }

    /// The restore shape end to end: a `.dump`-restored database opens *and* a
    /// retention pass over it behaves - the aged rows go, the newest survive, and
    /// the journal's payloads are released with the transcript they described.
    ///
    /// This is the pair the fix has to deliver together: the restore replays
    /// every migration over an already-current schema, and the pass that then
    /// runs on the timer must work on the result.
    #[tokio::test]
    async fn a_restored_dump_opens_and_a_retention_pass_behaves_on_it() {
        let db = TempDb::new("restore-then-prune");
        std::fs::create_dir_all(&db.dir).expect("dir");
        let keep = RETENTION_KEEP_MESSAGES;
        {
            let conn = rusqlite::Connection::open(&db.path).expect("raw open");
            write_schema_to(&conn, SCHEMA_VERSION);
            conn.pragma_update(None, "user_version", 0)
                .expect("version");
            conn.execute(
                "INSERT INTO sessions (id, title, workspace, created_at, updated_at)
                 VALUES ('s_1', 'Restored', '/w', 1, 2)",
                [],
            )
            .expect("session");
            // Older messages than the keep-window, all far outside a 30-day
            // window, plus a checkpoint whose payload must go with its message.
            for i in 0..(keep + 5) {
                conn.execute(
                    "INSERT INTO messages (session_id, role, content, created_at)
                     VALUES ('s_1', 'user', ?1, 1)",
                    params![format!("m{i}")],
                )
                .expect("message");
            }
            conn.execute(
                "INSERT INTO checkpoint_blobs (sha, data) VALUES ('deadbeef', ?1)",
                params![b"before-image".to_vec()],
            )
            .expect("blob");
            conn.execute(
                "INSERT INTO checkpoints
                    (session_id, run_id, call_id, tool, path, existed, content_id,
                     capture_failed, message_id, created_at)
                 VALUES ('s_1', 'r_1', 'c1', 'edit_file', 'old.rs', 1, 1, 0, 1, 1)",
                [],
            )
            .expect("checkpoint");
        }

        let store = db.open().await;
        assert_eq!(version_of(&store).await, SCHEMA_VERSION);
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM messages").await,
            (keep + 5) as i64
        );

        let report = store.prune_messages_older_than(30).await.expect("prune");
        assert_eq!(report.messages, 5, "only the rows past the keep-window go");
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM messages").await,
            keep as i64,
            "the newest messages of the session survive"
        );
        // The checkpoint pointed at message 1, which was pruned, so the journal
        // row and its payload are gone too.
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoints").await,
            0,
            "the journal must not outlive the transcript it described"
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM checkpoint_blobs").await,
            0,
            "and its payload is released with it"
        );
        // The pass is repeatable on a restored database.
        assert!(store
            .prune_messages_older_than(30)
            .await
            .expect("second prune")
            .is_empty());
        store.incremental_vacuum().await.expect("vacuum");
    }
}

#[cfg(test)]
mod project_tests {
    use super::tests::TempDb;
    use kc_core::store::Store;

    #[tokio::test]
    async fn projects_are_unique_by_path_and_count_their_sessions() {
        let db = TempDb::new("projects");
        let store = db.open().await;

        let project = store
            .create_project("p_1", "Karucode", "/home/me/karucode")
            .await
            .expect("create project");
        assert_eq!(project.session_count, 0);

        let duplicate = store
            .create_project("p_2", "Other", "/home/me/karucode")
            .await;
        assert!(duplicate.is_err(), "two projects may not share a directory");

        store
            .create_session("s_1", "First", "/home/me/karucode", Some("p_1"))
            .await
            .expect("session in project");
        store
            .create_session("s_2", "Loose", "/elsewhere", None)
            .await
            .expect("session without project");

        let projects = store.list_projects().await.expect("list projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].session_count, 1);

        let scoped = store
            .list_sessions_for_project("p_1")
            .await
            .expect("scoped sessions");
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "s_1");
        assert_eq!(scoped[0].project_id.as_deref(), Some("p_1"));
        assert_eq!(store.list_sessions().await.unwrap().len(), 2);

        let session = store.get_session("s_1").await.unwrap().unwrap();
        assert_eq!(session.project_id.as_deref(), Some("p_1"));
    }

    #[tokio::test]
    async fn a_legacy_session_can_be_attached_to_a_project() {
        let db = TempDb::new("attach");
        let store = db.open().await;
        store
            .create_session("s_old", "Legacy", "/legacy/root", None)
            .await
            .expect("legacy session");
        store
            .create_project("p_1", "Legacy", "/legacy/root")
            .await
            .expect("project");

        store
            .assign_session_project("s_old", "p_1")
            .await
            .expect("attach");
        let session = store.get_session("s_old").await.unwrap().unwrap();
        assert_eq!(session.project_id.as_deref(), Some("p_1"));
        assert_eq!(session.workspace, "/legacy/root");

        // Unknown projects and unknown sessions are a NotFound, never a silent
        // update of a different row.
        assert!(store.assign_session_project("s_old", "nope").await.is_err());
        assert!(store.assign_session_project("nope", "p_1").await.is_err());
    }

    #[tokio::test]
    async fn renaming_and_deleting_a_project_detaches_its_sessions() {
        let db = TempDb::new("project-delete");
        let store = db.open().await;
        store
            .create_project("p_1", "Old name", "/tmp/project")
            .await
            .expect("project");
        store
            .create_session("s_1", "Work", "/tmp/project", Some("p_1"))
            .await
            .expect("session");

        store
            .rename_project("p_1", "New name")
            .await
            .expect("rename");
        let project = store.get_project("p_1").await.unwrap().unwrap();
        assert_eq!(project.name, "New name");
        assert!(store.rename_project("missing", "x").await.is_err());

        store.delete_project("p_1").await.expect("delete");
        assert!(store.get_project("p_1").await.unwrap().is_none());
        let session = store.get_session("s_1").await.unwrap().unwrap();
        assert_eq!(
            session.project_id, None,
            "sessions survive a project delete but lose the reference"
        );
        assert!(store.list_projects().await.unwrap().is_empty());
    }
}
