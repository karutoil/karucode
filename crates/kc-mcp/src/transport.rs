//! Transport abstraction shared by the stdio and streamable-HTTP MCP clients.

use crate::rpc::McpError;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// One way of talking JSON-RPC to an MCP server.
///
/// Implementations are cheap to clone behind an `Arc` and must never leak
/// configuration secrets (headers, env values) through [`McpError`] or logs.
#[async_trait]
pub(crate) trait Transport: Send + Sync {
    /// Prepare the connection: spawn the child process for stdio, nothing for
    /// HTTP. Idempotent.
    async fn open(&self) -> Result<(), McpError>;

    /// Send a request and wait for its response.
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError>;

    /// Fire-and-forget notification (`notifications/initialized`).
    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError>;

    /// Close the connection and reap any child process.
    async fn close(&self);

    /// Synchronous last-resort close used by the public owner's `Drop`. Unlike
    /// dropping a future, this must signal a stdio child before returning; its
    /// retained supervisor performs the asynchronous reap.
    fn close_now(&self);

    /// `false` once a stdio child has exited; always `true` for HTTP.
    fn alive(&self) -> bool;

    /// Short label for status reporting: `stdio` or `http`.
    fn kind(&self) -> &'static str;
}

/// Await `future` with a deadline and an optional cancellation token.
pub(crate) async fn bounded<F>(
    future: F,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<Value, McpError>
where
    F: std::future::Future<Output = Result<Value, McpError>>,
{
    match cancel {
        Some(token) => tokio::select! {
            biased;
            _ = token.cancelled() => Err(McpError::Cancelled),
            result = tokio::time::timeout(timeout, future) => match result {
                Ok(value) => value,
                Err(_) => Err(McpError::Timeout(timeout)),
            },
        },
        None => match tokio::time::timeout(timeout, future).await {
            Ok(value) => value,
            Err(_) => Err(McpError::Timeout(timeout)),
        },
    }
}
