//! Test harness: a real router, a real SQLite store and the real runtime, with
//! a scripted provider factory and one approval-requesting tool. No network.
//!
//! Everything is exercised through `tower::ServiceExt::oneshot` so the tests
//! drive exactly the same middlewares and handlers as production.

#![allow(dead_code)]

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use kc_core::config::{
    ApprovalMode, Config, McpServerConfig, ModelConfig, PluginConfig, TerminalConfig, TitlesConfig,
};
use kc_core::paths::Workspace;
use kc_core::provider::{ChatRequest, Provider, ProviderChunk, ProviderFactory, ProviderStream};
use kc_core::store::Store;
use kc_core::tool::{Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOutcome, ToolSpec};
use kc_core::Error;
use kc_core::Result;
use kc_mcp::McpManager;
use kc_plugin_host::PluginHost;
use kc_runtime::{Runtime, RuntimeDeps};
use kc_server::app::build_router;
use kc_server::settings::SettingsHandle;
use kc_server::state::{base_tool_registry, AppState, AppStateDeps, PluginHooks, ProjectRegistry};
use kc_store::SqliteStore;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Scripted provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Script {
    /// Stream these text chunks, then usage + finish.
    Text(Vec<String>),
    /// One tool call, then finish with `tool_calls`.
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    /// Fail before the stream starts.
    Fail(String),
    /// Stream one chunk and then never finish (keeps a run active).
    Hang,
    /// Hold a response until the test explicitly releases it.
    GatedText(Arc<tokio::sync::Notify>, String),
}

pub fn text(value: &str) -> Script {
    Script::Text(vec![value.to_string()])
}

pub fn tool_call(name: &str, arguments: &str) -> Script {
    Script::ToolCall {
        id: format!("call_{name}"),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

#[derive(Debug, Clone)]
pub enum ModelsMode {
    Ok(Vec<String>),
    Fail(String),
}

struct FactoryInner {
    scripts: Mutex<VecDeque<Script>>,
    models: Mutex<ModelsMode>,
    /// Every provider request's message contents, in call order. Tests use this
    /// to prove what the model actually saw (tool results included).
    prompts: Mutex<Vec<Vec<String>>>,
}

pub struct ScriptedFactory {
    inner: Arc<FactoryInner>,
}

impl ScriptedFactory {
    pub fn new(scripts: Vec<Script>, models: ModelsMode) -> Self {
        Self {
            inner: Arc::new(FactoryInner {
                scripts: Mutex::new(scripts.into()),
                models: Mutex::new(models),
                prompts: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Every message content the scripted model has been sent, one entry per call.
    pub fn prompts(&self) -> Vec<Vec<String>> {
        self.inner.prompts.lock().unwrap().clone()
    }

    pub fn push(&self, script: Script) {
        self.inner.scripts.lock().unwrap().push_back(script);
    }

    fn next_script(&self) -> Script {
        self.inner
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text("ok"))
    }
}

struct ScriptedProvider {
    inner: Arc<FactoryInner>,
}

#[async_trait]
impl ProviderFactory for ScriptedFactory {
    async fn build(&self, _config: &ModelConfig) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(ScriptedProvider {
            inner: Arc::clone(&self.inner),
        }))
    }

    fn supported(&self) -> Vec<String> {
        vec!["openai_compatible".into()]
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<ProviderStream> {
        if let Some(admission) = &request.admission {
            admission.admit().await?;
        }
        self.inner.prompts.lock().unwrap().push(
            request
                .messages
                .iter()
                .map(|message| message.content.clone())
                .collect(),
        );
        let script = self
            .inner
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| text("ok"));
        match script {
            Script::Text(chunks) => {
                let mut items: Vec<Result<ProviderChunk>> = chunks
                    .into_iter()
                    .map(|delta| Ok(ProviderChunk::Text(delta)))
                    .collect();
                items.push(Ok(ProviderChunk::Usage {
                    input_tokens: 3,
                    output_tokens: 5,
                    cached_tokens: None,
                }));
                items.push(Ok(ProviderChunk::Finish {
                    reason: "stop".into(),
                }));
                Ok(Box::pin(futures_util::stream::iter(items)))
            }
            Script::ToolCall {
                id,
                name,
                arguments,
            } => {
                let items: Vec<Result<ProviderChunk>> = vec![
                    Ok(ProviderChunk::ToolCallDelta {
                        index: 0,
                        id: Some(id),
                        name: Some(name),
                        arguments_delta: Some(arguments),
                    }),
                    Ok(ProviderChunk::Finish {
                        reason: "tool_calls".into(),
                    }),
                ];
                Ok(Box::pin(futures_util::stream::iter(items)))
            }
            Script::GatedText(gate, text) => {
                gate.notified().await;
                Ok(Box::pin(futures_util::stream::iter(vec![
                    Ok(ProviderChunk::Text(text)),
                    Ok(ProviderChunk::Finish {
                        reason: "stop".into(),
                    }),
                ])))
            }
            Script::Fail(message) => Err(Error::Provider(message)),
            Script::Hang => Ok(Box::pin(
                futures_util::stream::iter(vec![Ok(ProviderChunk::Text("working...".into()))])
                    .chain(futures_util::stream::pending()),
            )),
        }
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        match self.inner.models.lock().unwrap().clone() {
            ModelsMode::Ok(models) => Ok(models),
            ModelsMode::Fail(message) => Err(Error::Provider(message)),
        }
    }
}

// ---------------------------------------------------------------------------
// A tool that asks for approval
// ---------------------------------------------------------------------------

pub struct ApprovalTool;

#[async_trait]
impl Tool for ApprovalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "needs_approval".into(),
            description: "Asks for shell-command approval and reports the answer.".into(),
            parameters: json!({"type": "object", "properties": {}}),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Shell
    }

    async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let decision = ctx
            .approvals
            .ask(
                kc_core::approval::ApprovalKind::ShellCommand,
                "Run a command",
                "echo hi",
                json!({"command": "echo hi"}),
            )
            .await;
        if decision.is_allowed() {
            ToolOutcome::ok(format!("approved: {}", decision.as_str()))
        } else {
            ToolOutcome::err(format!("denied: {}", decision.as_str()))
        }
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

pub struct HarnessBuilder {
    scripts: Vec<Script>,
    models: ModelsMode,
    auth_token: Option<String>,
    lock_settings: bool,
    web_dir: Option<PathBuf>,
    extra_tools: Vec<Arc<dyn Tool>>,
    plugin_configs: Vec<PluginConfig>,
    mcp_configs: Vec<McpServerConfig>,
    bind_host: String,
    /// `None` = read `KARUCODE_ALLOWED_HOSTS` like the bootstrap does.
    allowed_hosts: Option<Option<String>>,
    /// Allowlist for `POST /api/projects`. Empty (the default) is "no
    /// allowlist", which is what an install with `KARUCODE_PROJECT_ROOTS` unset
    /// gets.
    project_roots: Vec<PathBuf>,
    /// `[terminals]`. `None` uses the defaults (on, `/bin/sh`-ish, 16 slots).
    terminals: Option<TerminalConfig>,
    /// `[limits]`. Tests that exercise command environment forwarding use this to
    /// build the same initial settings as a config file.
    limits: Option<kc_core::config::Limits>,
    retention_days: u64,
    /// `allow_temp_dir = false`: the workspace is the only allowed root, so a
    /// `..` escape is refused the way it is in a normal install outside `/tmp`.
    strict_root: bool,
    /// `[titles]`. `None` keeps the shipped default, which is AI titles **on**.
    titles: Option<TitlesConfig>,
    /// `[approvals] mode`. `None` keeps the shipped default, `ask`.
    approval_mode: Option<ApprovalMode>,
}

impl HarnessBuilder {
    pub fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts,
            models: ModelsMode::Ok(vec!["test-model".into()]),
            auth_token: None,
            lock_settings: false,
            web_dir: None,
            extra_tools: Vec::new(),
            plugin_configs: Vec::new(),
            mcp_configs: Vec::new(),
            bind_host: "127.0.0.1".into(),
            allowed_hosts: None,
            project_roots: Vec::new(),
            terminals: None,
            limits: None,
            retention_days: 0,
            strict_root: false,
            titles: None,
            approval_mode: None,
        }
    }

    /// Configure `[terminals]`. The default is the shipped one (enabled, one
    /// shell per tab, 16 slots); a test that wants a disabled or tiny terminal
    /// sets it here rather than reaching into the live settings.
    pub fn terminals(mut self, config: TerminalConfig) -> Self {
        self.terminals = Some(config);
        self
    }

    pub fn limits(mut self, config: kc_core::config::Limits) -> Self {
        self.limits = Some(config);
        self
    }

    pub fn retention_days(mut self, days: u64) -> Self {
        self.retention_days = days;
        self
    }

    /// Configure `project_roots` (`KARUCODE_PROJECT_ROOTS`), which turns project
    /// creation into an allowlisted operation.
    pub fn project_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.project_roots = roots;
        self
    }

    /// Confine the file tools to the project root only, by turning
    /// `allow_temp_dir` off.
    ///
    /// The default is *on*, and deliberately so - the temp directory is a second
    /// allowed root (see `Config::extra_roots` and the `security` doc topic). That
    /// makes `/tmp` a legal destination, which is why a test that wants to prove a
    /// `..` escape is refused has to ask for project-only confinement first: the
    /// harness workspace lives under `std::env::temp_dir()`, so with the default
    /// `../../etc/passwd` genuinely resolves to `<tmp>/etc/passwd` and is *allowed*.
    /// Without this, an escape assertion passes or fails depending on what happens
    /// to be in `/tmp`, which is not a test.
    pub fn strict_workspace(mut self) -> Self {
        self.strict_root = true;
        self
    }

    /// Override `[titles]`. The shipped default is AI titles **on** (see the
    /// `session-titles` doc topic), and the harness models are scripted, so a
    /// titling turn is answered by the next script exactly like a run turn.
    /// A test that asserts the *deterministic* title has to turn the model title
    /// off with [`.no_ai_titles`](Self::no_ai_titles), or it is racing the titler.
    pub fn titles(mut self, config: TitlesConfig) -> Self {
        self.titles = Some(config);
        self
    }

    /// Turn the background AI title off, leaving the deterministic title the
    /// server derives from the first message. Use this in any test that asserts
    /// a title without first arranging a titling response.
    pub fn no_ai_titles(mut self) -> Self {
        self.titles = Some(TitlesConfig {
            enabled: false,
            ..TitlesConfig::default()
        });
        self
    }

    pub fn models(mut self, models: ModelsMode) -> Self {
        self.models = models;
        self
    }

    pub fn auth_token(mut self, token: &str) -> Self {
        self.auth_token = Some(token.to_string());
        self
    }

    pub fn lock_settings(mut self) -> Self {
        self.lock_settings = true;
        self
    }

    /// Run with `approvals.mode = auto`, so a mutating tool call is not parked.
    pub fn auto_approvals(mut self) -> Self {
        self.approval_mode = Some(ApprovalMode::Auto);
        self
    }

    pub fn web_dir(mut self, dir: PathBuf) -> Self {
        self.web_dir = Some(dir);
        self
    }

    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.extra_tools.push(tool);
        self
    }

    pub fn plugin(mut self, config: PluginConfig) -> Self {
        self.plugin_configs.push(config);
        self
    }

    pub fn mcp(mut self, config: McpServerConfig) -> Self {
        self.mcp_configs.push(config);
        self
    }

    /// Bind address the fake process reports (`KARUCODE_HOST`).
    pub fn bind_host(mut self, host: &str) -> Self {
        self.bind_host = host.to_string();
        self
    }

    /// Explicit `KARUCODE_ALLOWED_HOSTS` value, bypassing the process
    /// environment (which is shared by every test in the binary).
    pub fn allowed_hosts(mut self, raw: &str) -> Self {
        self.allowed_hosts = Some(Some(raw.to_string()));
        self
    }

    pub async fn build(self) -> Harness {
        let root = std::env::temp_dir().join(format!(
            "kc-server-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let workspace_dir = root.join("workspace");
        let data_dir = root.join("data");
        std::fs::create_dir_all(&workspace_dir).expect("workspace dir");
        std::fs::create_dir_all(&data_dir).expect("data dir");

        let web_dir = self.web_dir;
        let db_path = data_dir.join("karucode.db");
        let allowed_hosts = self
            .allowed_hosts
            .unwrap_or_else(|| std::env::var(kc_server::host::ALLOWED_HOSTS_ENV).ok());
        let config = Config {
            host: self.bind_host.clone(),
            workspace_root: workspace_dir.clone(),
            data_dir: data_dir.clone(),
            auth_token: self.auth_token,
            lock_settings: self.lock_settings,
            web_dir: web_dir.clone(),
            // The settings snapshot is what a plugin reload re-reads, so it must
            // list the entries the host was actually given — the bootstrap wires
            // it that way too.
            plugins: self.plugin_configs.clone(),
            terminals: self.terminals.clone().unwrap_or_default(),
            limits: self.limits.clone().unwrap_or_default(),
            project_roots: self.project_roots.clone(),
            retention_days: self.retention_days,
            allow_temp_dir: !self.strict_root,
            titles: self.titles.clone().unwrap_or_default(),
            approvals: kc_core::config::ApprovalPolicy {
                mode: self.approval_mode.unwrap_or_default(),
                ..Default::default()
            },
            model: ModelConfig {
                base_url: "http://127.0.0.1:9/v1".into(),
                model: "test-model".into(),
                api_key: None,
                ..ModelConfig::default()
            },
            ..Config::default()
        };

        let store = Arc::new(SqliteStore::open(&db_path).await.expect("store"));
        let settings = SettingsHandle::new(config.clone(), store.clone())
            .await
            .expect("settings");
        let factory = Arc::new(ScriptedFactory::new(self.scripts, self.models));
        // Mirrors the bootstrap: the workspace carries the configured extra roots
        // (the temp dir by default), and the registry re-applies them to projects.
        let workspace = Arc::new(
            Workspace::with_extra_roots(&workspace_dir, config.extra_roots()).expect("workspace"),
        );

        // Mirrors the bootstrap: externals start concurrently and a broken one
        // is isolated as a failed diagnostic.
        let mut plugin_host = PluginHost::discover(&self.plugin_configs, &data_dir);
        let mut mcp_manager = McpManager::new(&self.mcp_configs);
        tokio::join!(plugin_host.start_all(), mcp_manager.connect_all());
        let plugins = Arc::new(AsyncMutex::new(plugin_host));
        let mcp = Arc::new(AsyncMutex::new(mcp_manager));

        let mut builtin_tools = kc_tools::builtin_tools();
        builtin_tools.extend(self.extra_tools);
        let tools = base_tool_registry(builtin_tools.clone(), Vec::new(), Vec::new());

        // Mirrors the bootstrap: one default project for the workspace, loaded
        // into the registry the runtime resolves runs against.
        let projects = Arc::new(ProjectRegistry::with_extra_roots(
            Arc::clone(&workspace),
            workspace.extra_roots().to_vec(),
        ));
        let default_project = kc_core::ids::ProjectId::new();
        store
            .create_project(
                default_project.as_str(),
                "test-workspace",
                &workspace.root().to_string_lossy(),
            )
            .await
            .expect("default project");
        projects
            .insert(default_project.as_str(), workspace.root())
            .expect("register default project");

        let runtime = Runtime::new(RuntimeDeps {
            store: store.clone(),
            providers: factory.clone(),
            tools,
            settings: settings.clone(),
            hooks: Arc::new(PluginHooks::new(Arc::clone(&plugins))),
            workspaces: Arc::clone(&projects) as Arc<dyn kc_runtime::WorkspaceProvider>,
        });
        runtime.recover().await.expect("recover");

        let web_auth = kc_server::web_auth::WebAuth::load(store.clone())
            .await
            .expect("web auth");

        let state = AppState::new(AppStateDeps {
            settings,
            store,
            runtime,
            providers: factory.clone(),
            projects,
            plugins,
            mcp,
            builtin_tools,
            web_dir,
            db_path,
            bind_host: self.bind_host,
            allowed_hosts,
            web_auth,
        });
        state.reload_tools().await;
        let router = build_router(state.clone());

        Harness {
            router,
            state,
            factory,
            root,
            workspace_dir,
        }
    }
}

pub struct Harness {
    pub router: Router,
    pub state: AppState,
    pub factory: Arc<ScriptedFactory>,
    pub root: PathBuf,
    pub workspace_dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Best effort: on Unix an open SQLite file can be unlinked safely.
        std::fs::remove_dir_all(&self.root).ok();
    }
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "response was not JSON ({e}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn error_code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The documented error message, for asserting on what a rejection says.
    pub fn error_message(&self) -> String {
        self.json()["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PATCH | Method::PUT | Method::DELETE
    )
}

impl Harness {
    pub fn workspace_path(&self) -> &std::path::Path {
        &self.workspace_dir
    }

    /// Send a request with the CSRF header added automatically for mutations.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> TestResponse {
        self.send_with(method, path, body, headers, true).await
    }

    pub async fn send_with(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
        add_csrf: bool,
    ) -> TestResponse {
        let mut builder = Request::builder().method(method.clone()).uri(path);
        let mut has_host = false;
        let mut has_client = false;
        let mut has_content_type = false;
        for (key, value) in headers {
            let key_lower = key.to_ascii_lowercase();
            if key_lower == "host" {
                has_host = true;
            }
            if key_lower == "x-karucode-client" {
                has_client = true;
            }
            if key_lower == "content-type" {
                has_content_type = true;
            }
            builder = builder.header(*key, *value);
        }
        if !has_host {
            builder = builder.header("host", "127.0.0.1:8787");
        }
        if is_mutating(&method) && add_csrf && !has_client {
            builder = builder.header("x-karucode-client", "web");
        }
        if body.is_some() && !has_content_type {
            builder = builder.header("content-type", "application/json");
        }
        builder = builder.header("accept", "application/json");

        let body = match body {
            Some(value) => Body::from(serde_json::to_vec(&value).expect("serialize body")),
            None => Body::empty(),
        };
        let request = builder.body(body).expect("request");
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router call");
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .expect("read body");
        TestResponse {
            status,
            headers,
            body,
        }
    }

    pub async fn get(&self, path: &str) -> TestResponse {
        self.send(Method::GET, path, None, &[]).await
    }

    pub async fn get_with(&self, path: &str, headers: &[(&str, &str)]) -> TestResponse {
        self.send(Method::GET, path, None, headers).await
    }

    pub async fn post(&self, path: &str, body: Value) -> TestResponse {
        self.send(Method::POST, path, Some(body), &[]).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> TestResponse {
        self.send(Method::PATCH, path, Some(body), &[]).await
    }

    pub async fn delete(&self, path: &str) -> TestResponse {
        self.send(Method::DELETE, path, None, &[]).await
    }

    pub async fn create_session(&self, title: Option<&str>) -> String {
        let body = match title {
            Some(title) => json!({ "title": title }),
            None => json!({}),
        };
        let response = self.post("/api/sessions", body).await;
        assert_eq!(response.status, StatusCode::OK, "body: {}", response.text());
        response.json()["id"]
            .as_str()
            .expect("session id")
            .to_string()
    }

    /// Open an SSE stream. The response body is returned unpolled.
    pub async fn open_events(
        &self,
        query: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Body) {
        let path = format!("/api/events{query}");
        let mut builder = Request::builder().method(Method::GET).uri(path);
        builder = builder.header("host", "127.0.0.1:8787");
        builder = builder.header("accept", "text/event-stream");
        for (key, value) in headers {
            builder = builder.header(*key, *value);
        }
        let request = builder.body(Body::empty()).expect("sse request");
        let response = self.router.clone().oneshot(request).await.expect("sse");
        let status = response.status();
        let headers = response.headers().clone();
        (status, headers, response.into_body())
    }

    /// Poll `GET /api/approvals` until one pending approval exists.
    pub async fn wait_for_approval(&self, session_id: &str) -> Value {
        for _ in 0..300 {
            let response = self
                .get(&format!("/api/approvals?session={session_id}"))
                .await;
            assert_eq!(response.status, StatusCode::OK);
            let approvals = response.json()["approvals"].clone();
            if let Some(first) = approvals.as_array().and_then(|list| list.first()) {
                return first.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no pending approval appeared for {session_id}");
    }

    /// Poll the session detail until it reports no active run.
    pub async fn wait_until_idle(&self, session_id: &str) {
        for _ in 0..300 {
            let response = self.get(&format!("/api/sessions/{session_id}")).await;
            assert_eq!(response.status, StatusCode::OK);
            if response.json()["running"] == json!(false) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("session {session_id} never went idle");
    }

    /// Poll the session detail until it reports an active run.
    ///
    /// Needed to steer or queue a follow-up against a run: submitting returns
    /// once the run row exists, but `running` is derived from the runtime's map,
    /// which the background task registers a moment later.
    pub async fn wait_until_running(&self, session_id: &str) {
        for _ in 0..300 {
            let response = self.get(&format!("/api/sessions/{session_id}")).await;
            assert_eq!(response.status, StatusCode::OK);
            if response.json()["running"] == json!(true) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("session {session_id} never started running");
    }
}

/// Alias kept explicit at the call site: several mode tests wait for a hang-based
/// run to register before steering at it.
pub async fn wait_for_running(h: &Harness, session_id: &str) {
    h.wait_until_running(session_id).await;
}

/// Wait for the session to settle, without asserting it is idle right away.
///
/// A cancelled run unwinds asynchronously, so `running` can still be true for a
/// moment; this is the "best effort settle" the mode tests need after a cancel.
pub async fn wait_until_idle_ignored(h: &Harness, session_id: &str) {
    for _ in 0..300 {
        let response = h.get(&format!("/api/sessions/{session_id}")).await;
        if response.status == StatusCode::OK && response.json()["running"] == json!(false) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// SSE helpers
// ---------------------------------------------------------------------------

/// Parse complete `data:` blocks out of `buffer`, keeping the incomplete tail.
pub fn drain_sse(buffer: &mut String) -> Vec<Value> {
    let mut events = Vec::new();
    while let Some(boundary) = buffer.find("\n\n") {
        let block = buffer[..boundary].to_string();
        buffer.drain(..boundary + 2);
        let data: String = block
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.trim_start())
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            events.push(value);
        }
    }
    events
}

/// Collect events until one has `type == until` (bounded by 5s).
pub async fn collect_events_until(body: Body, until: &str) -> Vec<Value> {
    use futures_util::StreamExt;
    let mut stream = body.into_data_stream();
    let mut buffer = String::new();
    let mut events = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => {
                panic!("timed out waiting for SSE `{until}`; received: {events:?}");
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        events.extend(drain_sse(&mut buffer));
                        if events.iter().any(|event| event["type"] == until) {
                            return events;
                        }
                    }
                    Some(Err(e)) => panic!("SSE stream error: {e}"),
                    None => panic!("SSE stream ended before `{until}`; received: {events:?}"),
                }
            }
        }
    }
}

/// Read whatever arrives within `window` (keepalives produce no `data:` events).
pub async fn events_within(body: Body, window: Duration) -> Vec<Value> {
    use futures_util::StreamExt;
    let mut stream = body.into_data_stream();
    let mut buffer = String::new();
    let mut events = Vec::new();
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        events.extend(drain_sse(&mut buffer));
                    }
                    Some(Err(_)) | None => break,
                }
            }
        }
    }
    events
}

/// Convenience for tests that need the concrete store trait object.
pub fn store_of(harness: &Harness) -> Arc<dyn Store> {
    harness.state.store()
}
