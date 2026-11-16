//! `plugin/state/*`: the store behind a plugin's own durable data (§R2).
//!
//! A plugin is a process, not a database. It can hold state in memory, but that
//! dies with the process — and a plugin is restarted on a crash, on a rebuild and
//! on every server upgrade. This adapter is the durable alternative: the plugin
//! asks the host, the host asks the store, and the row belongs to that plugin's
//! namespace.
//!
//! The bounds live in `kc-plugin-host` (`state_call`), not here: this type is a
//! thin translation between the trait and [`kc_store`]'s methods, so the host can
//! be tested against an in-memory map and the limits have exactly one home.
//!
//! # What a plugin cannot do
//!
//! * reach another plugin's rows — every method takes the plugin name from the
//!   host, not from the request;
//! * reach another session's rows — the owner id is filled in by the host from
//!   the session/project the call was made in;
//! * grow without limit — a value, a row count and a per-scope byte budget are
//!   all enforced before the write lands;
//! * define a table, a column or a route — this is one table with a fixed shape,
//!   and `plugin_state` is not exposed by any HTTP route except the one the
//!   operator uses to inspect and clear a plugin's data.

use kc_core::plugin::{PluginStateBackend, MAX_STATE_LIST};
use kc_store::SqliteStore;
use std::sync::Arc;

/// Byte-for-byte adapter over the store's `plugin_state_*` methods.
pub struct StoreStateBackend {
    store: Arc<SqliteStore>,
}

impl StoreStateBackend {
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl PluginStateBackend for StoreStateBackend {
    async fn get(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        key: &str,
    ) -> kc_core::Result<Option<String>> {
        self.store.plugin_state_get(plugin, scope, owner, key).await
    }

    async fn set(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        key: &str,
        value: &str,
    ) -> kc_core::Result<i64> {
        self.store
            .plugin_state_set(plugin, scope, owner, key, value)
            .await
    }

    async fn delete(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        key: &str,
    ) -> kc_core::Result<bool> {
        self.store
            .plugin_state_delete(plugin, scope, owner, key)
            .await
    }

    async fn list(
        &self,
        plugin: &str,
        scope: &str,
        owner: &str,
        prefix: &str,
        limit: usize,
    ) -> kc_core::Result<Vec<(String, String)>> {
        self.store
            .plugin_state_list(plugin, scope, owner, prefix, limit.min(MAX_STATE_LIST))
            .await
    }

    async fn bytes(&self, plugin: &str, scope: &str, owner: &str) -> kc_core::Result<i64> {
        self.store.plugin_state_bytes(plugin, scope, owner).await
    }
}
