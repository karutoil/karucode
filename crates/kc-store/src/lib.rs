//! SQLite persistence.
//!
//! One writer connection guarded by a mutex, WAL mode, synchronous=NORMAL. All
//! queries run on the blocking pool so the async runtime is never blocked.

use async_trait::async_trait;
use kc_core::config::SessionModelSelection;
use kc_core::error::{Error, Result};
use kc_core::message::ToolCall;
use kc_core::store::*;
use rusqlite::{params, Row};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Current schema version. Bump together with a new arm in [`SqliteStore::migrate`].
const SCHEMA_VERSION: i64 = 3;

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
const MIGRATION_2: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    path       TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_path ON projects(path);

ALTER TABLE sessions ADD COLUMN project_id TEXT;
CREATE INDEX IF NOT EXISTS idx_sessions_project_id ON sessions(project_id);
"#;

/// Schema v3: per-session model selection. NULL follows the install-wide
/// default; otherwise a JSON [`SessionModelSelection`] (`{provider, model,
/// reasoning_effort?}`) pins this session to a profile + model.
const MIGRATION_3: &str = r#"
ALTER TABLE sessions ADD COLUMN model TEXT;
"#;

pub struct SqliteStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
    path: PathBuf,
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

fn db_err(context: &str, e: rusqlite::Error) -> Error {
    Error::Store(format!("{context}: {e}"))
}

fn limit_i64(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
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

fn summarize_sessions(
    conn: &rusqlite::Connection,
    project_id: Option<&str>,
) -> Result<Vec<SessionSummary>> {
    let base =
        "SELECT s.id, s.title, s.workspace, s.project_id, s.model, s.created_at, s.updated_at,
                       (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id) AS message_count,
                       (SELECT r.status FROM runs r
                         WHERE r.session_id = s.id
                         ORDER BY r.started_at DESC, r.id DESC LIMIT 1) AS last_run_status,
                       EXISTS(SELECT 1 FROM runs r
                               WHERE r.session_id = s.id AND r.status = 'running') AS running
                  FROM sessions s";
    let sql = match project_id {
        Some(_) => format!("{base} WHERE s.project_id = ?1 ORDER BY s.updated_at DESC, s.id DESC"),
        None => format!("{base} ORDER BY s.updated_at DESC, s.id DESC"),
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| db_err("list_sessions prepare", e))?;
    let rows = match project_id {
        Some(id) => stmt.query_map(params![id], map_session_summary),
        None => stmt.query_map([], map_session_summary),
    }
    .map_err(|e| db_err("list_sessions query", e))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| db_err("list_sessions rows", e))
}

fn map_stored_message(row: &Row<'_>) -> rusqlite::Result<StoredMessage> {
    let tool_calls = json_vec::<ToolCall>(row.get("tool_calls")?, "messages.tool_calls");
    Ok(StoredMessage {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        run_id: row.get("run_id")?,
        agent_id: row.get("agent_id")?,
        role: row.get("role")?,
        content: row.get("content")?,
        tool_calls,
        tool_call_id: row.get("tool_call_id")?,
        name: row.get("name")?,
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

impl SqliteStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                let existed = parent.exists();
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                if !existed {
                    // Settings may hold an API key: never leave the directory
                    // readable by other local users.
                    restrict_permissions(parent, 0o700)?;
                }
            }
        }
        let db_path = path.clone();
        let conn = tokio::task::spawn_blocking(move || -> Result<rusqlite::Connection> {
            let conn = rusqlite::Connection::open(&db_path)
                .map_err(|e| Error::Store(format!("open {}: {e}", db_path.display())))?;
            // Tighten the file mode before WAL/journal files are created so they
            // inherit it (SQLite copies the database file's permissions).
            restrict_permissions(&db_path, 0o600)?;
            conn.pragma_update(None, "journal_mode", "WAL").ok();
            conn.pragma_update(None, "synchronous", "NORMAL").ok();
            conn.pragma_update(None, "foreign_keys", "ON").ok();
            conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
            Ok(conn)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))??;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        };
        store.migrate().await?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a blocking closure with exclusive access to the connection.
    pub(crate) async fn with<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut rusqlite::Connection) -> Result<T> + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| Error::Store("database mutex poisoned".into()))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| Error::Store(format!("blocking task failed: {e}")))?
    }

    /// Forward-only, idempotent migrations driven by `PRAGMA user_version`.
    ///
    /// Each step runs in its own transaction: the DDL and the version bump are
    /// committed together, so a crash mid-migration leaves the previous version
    /// intact. `IF NOT EXISTS` everywhere means a re-run is harmless.
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
                match version {
                    1 => tx
                        .execute_batch(MIGRATION_1)
                        .map_err(|e| db_err("migration v1", e))?,
                    2 => tx
                        .execute_batch(MIGRATION_2)
                        .map_err(|e| db_err("migration v2", e))?,
                    3 => tx
                        .execute_batch(MIGRATION_3)
                        .map_err(|e| db_err("migration v3", e))?,
                    other => {
                        return Err(Error::Store(format!("missing migration v{other}")));
                    }
                }
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

#[async_trait]
impl Store for SqliteStore {
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

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        self.with(|conn| summarize_sessions(conn, None)).await
    }

    async fn list_sessions_for_project(&self, project_id: &str) -> Result<Vec<SessionSummary>> {
        let project_id = project_id.to_string();
        self.with(move |conn| summarize_sessions(conn, Some(&project_id)))
            .await
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let id = id.to_string();
        self.with(move |conn| {
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
            // Messages/runs/agents/approvals/claims go with it (ON DELETE CASCADE).
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map_err(|e| db_err("delete_session", e))?;
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
        self.with(|conn| {
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
        self.with(move |conn| {
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
            let tx = conn.transaction().map_err(|e| db_err("append_message begin", e))?;
            tx.execute(
                "INSERT INTO messages
                    (session_id, run_id, agent_id, role, content, tool_calls, tool_call_id, name, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    message.session_id,
                    message.run_id,
                    message.agent_id,
                    message.role,
                    message.content,
                    tool_calls,
                    message.tool_call_id,
                    message.name,
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
        self.with(move |conn| {
            // Newest N by id, then re-sorted ascending: the caller gets the tail
            // of the conversation in chronological order.
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM (
                         SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                                tool_call_id, name, created_at
                           FROM messages WHERE session_id = ?1
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

    async fn list_messages_after(
        &self,
        session_id: &str,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let session_id = session_id.to_string();
        let limit = limit_i64(limit);
        self.with(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, run_id, agent_id, role, content, tool_calls,
                            tool_call_id, name, created_at
                       FROM messages
                      WHERE session_id = ?1 AND id > ?2
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
        self.with(move |conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO runs (id, session_id, status, model, prompt, error, started_at, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL)
                 ON CONFLICT(id) DO NOTHING",
                params![id, session_id, RunStatus::Running.as_str(), model, prompt, now],
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
        self.with(|conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE runs SET status = ?1, finished_at = ?2 WHERE status = 'running'",
                    params![RunStatus::Interrupted.as_str(), now],
                )
                .map_err(|e| db_err("mark_interrupted_runs", e))?;
            Ok(n)
        })
        .await
    }

    async fn list_runs(&self, session_id: &str, limit: usize) -> Result<Vec<RunRecord>> {
        let session_id = session_id.to_string();
        let limit = limit_i64(limit);
        self.with(move |conn| {
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

    async fn upsert_agent(&self, agent: &AgentRecord) -> Result<()> {
        let agent = agent.clone();
        self.with(move |conn| {
            let now = now_ms();
            let owned_paths = serde_json::to_string(&agent.owned_paths)
                .map_err(|e| Error::Store(format!("encode owned_paths: {e}")))?;
            let updated_at = if agent.updated_at > 0 { agent.updated_at } else { now };
            let created_at = if agent.created_at > 0 { agent.created_at } else { now };
            conn.execute(
                "INSERT INTO agents
                    (id, session_id, parent_run_id, parent_agent_id, name, role, task, status,
                     model, iterations, summary, error, owned_paths, created_at, updated_at, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                     finished_at     = excluded.finished_at",
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
                    agent.finished_at
                ],
            )
            .map_err(|e| db_err("upsert_agent", e))?;
            Ok(())
        })
        .await
    }

    async fn list_agents(&self, session_id: &str) -> Result<Vec<AgentRecord>> {
        let session_id = session_id.to_string();
        self.with(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, parent_run_id, parent_agent_id, name, role, task,
                            status, model, iterations, summary, error, owned_paths, created_at,
                            updated_at, finished_at
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

    async fn mark_interrupted_agents(&self) -> Result<usize> {
        self.with(|conn| {
            let now = now_ms();
            let n = conn
                .execute(
                    "UPDATE agents SET status = ?1, updated_at = ?2, finished_at = ?2
                      WHERE status = 'running'",
                    params![AgentStatus::Interrupted.as_str(), now],
                )
                .map_err(|e| db_err("mark_interrupted_agents", e))?;
            Ok(n)
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
        self.with(move |conn| {
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
        self.with(move |conn| {
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

    async fn all_settings(&self) -> Result<Vec<(String, String)>> {
        self.with(|conn| {
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
        self.with(move |conn| {
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

        store
            .rename_session("s_1", "Renamed")
            .await
            .expect("rename");
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
            conn.execute_batch(MIGRATION_1).expect("v1");
            conn.execute_batch(MIGRATION_2).expect("v2");
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

        // Only the still-running run is interrupted.
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

        // a_2 completed -> not interrupted; flip it back to running to test counting.
        store
            .upsert_agent(&agent("a_2", "s_1", "running"))
            .await
            .expect("reopen");
        assert_eq!(store.mark_interrupted_agents().await.expect("interrupt"), 1);
        let agents = store.list_agents("s_1").await.expect("list");
        let a2 = agents.iter().find(|a| a.id == "a_2").expect("a2");
        assert_eq!(a2.status, "interrupted");
        assert!(a2.finished_at.is_some());
        assert_eq!(store.mark_interrupted_agents().await.expect("noop"), 0);
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
