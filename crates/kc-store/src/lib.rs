//! SQLite persistence.
//!
//! One writer connection guarded by a mutex, WAL mode, synchronous=NORMAL. All
//! queries run on the blocking pool so the async runtime is never blocked.

use async_trait::async_trait;
use kc_core::error::{Error, Result};
use kc_core::store::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct SqliteStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
    path: PathBuf,
}

impl SqliteStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let db_path = path.clone();
        let db_path = path.clone();
        let conn = tokio::task::spawn_blocking(move || -> Result<rusqlite::Connection> {
            let conn = rusqlite::Connection::open(&db_path)
                .map_err(|e| Error::Store(format!("open {}: {e}", db_path.display())))?;
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

    async fn migrate(&self) -> Result<()> {
        todo!("kc-store: implement schema migrations")
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn create_session(&self, _id: &str, _title: &str, _workspace: &str) -> Result<Session> {
        todo!("kc-store: create_session")
    }
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        todo!("kc-store: list_sessions")
    }
    async fn get_session(&self, _id: &str) -> Result<Option<Session>> {
        todo!("kc-store: get_session")
    }
    async fn rename_session(&self, _id: &str, _title: &str) -> Result<()> {
        todo!("kc-store: rename_session")
    }
    async fn delete_session(&self, _id: &str) -> Result<()> {
        todo!("kc-store: delete_session")
    }
    async fn touch_session(&self, _id: &str) -> Result<()> {
        todo!("kc-store: touch_session")
    }
    async fn append_message(&self, _message: &NewMessage) -> Result<i64> {
        todo!("kc-store: append_message")
    }
    async fn list_messages(&self, _session_id: &str, _limit: usize) -> Result<Vec<StoredMessage>> {
        todo!("kc-store: list_messages")
    }
    async fn list_messages_after(&self, _session_id: &str, _after_id: i64, _limit: usize) -> Result<Vec<StoredMessage>> {
        todo!("kc-store: list_messages_after")
    }
    async fn create_run(&self, _id: &str, _session_id: &str, _model: &str, _prompt: &str) -> Result<()> {
        todo!("kc-store: create_run")
    }
    async fn finish_run(&self, _run_id: &str, _status: RunStatus, _error: Option<&str>) -> Result<()> {
        todo!("kc-store: finish_run")
    }
    async fn mark_interrupted_runs(&self) -> Result<usize> {
        todo!("kc-store: mark_interrupted_runs")
    }
    async fn list_runs(&self, _session_id: &str, _limit: usize) -> Result<Vec<RunRecord>> {
        todo!("kc-store: list_runs")
    }
    async fn upsert_agent(&self, _agent: &AgentRecord) -> Result<()> {
        todo!("kc-store: upsert_agent")
    }
    async fn list_agents(&self, _session_id: &str) -> Result<Vec<AgentRecord>> {
        todo!("kc-store: list_agents")
    }
    async fn mark_interrupted_agents(&self) -> Result<usize> {
        todo!("kc-store: mark_interrupted_agents")
    }
    async fn create_approval(&self, _record: &ApprovalRecord) -> Result<()> {
        todo!("kc-store: create_approval")
    }
    async fn resolve_approval(&self, _id: &str, _decision: &str) -> Result<()> {
        todo!("kc-store: resolve_approval")
    }
    async fn expire_pending_approvals(&self, _run_id: &str, _reason: &str) -> Result<usize> {
        todo!("kc-store: expire_pending_approvals")
    }
    async fn list_pending_approvals(&self, _session_id: Option<&str>) -> Result<Vec<ApprovalRecord>> {
        todo!("kc-store: list_pending_approvals")
    }
    async fn get_setting(&self, _key: &str) -> Result<Option<String>> {
        todo!("kc-store: get_setting")
    }
    async fn set_setting(&self, _key: &str, _value: &str) -> Result<()> {
        todo!("kc-store: set_setting")
    }
    async fn all_settings(&self) -> Result<Vec<(String, String)>> {
        todo!("kc-store: all_settings")
    }
    async fn claim_paths(&self, _session_id: &str, _agent_id: &str, _paths: &[String], _mode: &str) -> Result<Vec<ClaimRecord>> {
        todo!("kc-store: claim_paths")
    }
    async fn list_claims(&self, _session_id: &str) -> Result<Vec<ClaimRecord>> {
        todo!("kc-store: list_claims")
    }
    async fn release_claims(&self, _agent_id: &str) -> Result<()> {
        todo!("kc-store: release_claims")
    }
}
