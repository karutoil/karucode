//! End-to-end tests for the plugin host against `tests/fixtures/fake_plugin.sh`.
//!
//! The fixture is a POSIX shell script that runs an inline python3 JSON-RPC
//! server, so these tests do not depend on any cargo-built binary.

use async_trait::async_trait;
use kc_core::approval::{ApprovalDecision, ApprovalRequest};
use kc_core::config::{ApprovalPolicy, Limits, ModelConfig, PluginConfig, SubagentConfig};
use kc_core::hooks::Hooks;
use kc_core::ids::{RunId, SessionId};
use kc_core::paths::Workspace;
use kc_core::plugin::{PluginStatus, ToolCallContext};
use kc_core::settings::Settings;
use kc_core::tool::{
    ApprovalGate, ApprovalHandle, Tool, ToolCallRequest, ToolClass, ToolCtx, ToolOrigin,
    ToolOutcome,
};
use kc_plugin_host::{PluginHost, PluginTimeouts};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/fake_plugin.sh"
);

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-plugin-host-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Plugin config that runs the fixture; `env_name` sets the manifest name the
/// fixture reports (it must match the slot name).
fn plugin(name: &str, env_name: &str) -> PluginConfig {
    let mut env = BTreeMap::new();
    env.insert("FAKE_PLUGIN_NAME".to_string(), env_name.to_string());
    PluginConfig {
        name: name.to_string(),
        enabled: true,
        command: vec!["sh".to_string(), FIXTURE.to_string()],
        env,
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    }
}

struct TestSettings;

#[async_trait]
impl Settings for TestSettings {
    async fn model(&self) -> ModelConfig {
        ModelConfig::default()
    }
    async fn limits(&self) -> Limits {
        Limits::default()
    }
    async fn approval_policy(&self) -> ApprovalPolicy {
        ApprovalPolicy::default()
    }
    async fn subagent_config(&self) -> SubagentConfig {
        SubagentConfig::default()
    }
}

struct AllowGate;

#[async_trait]
impl ApprovalGate for AllowGate {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::AllowOnce
    }
}

fn ctx(root: &std::path::Path, cancel: CancellationToken) -> ToolCtx {
    ToolCtx {
        workspace: Arc::new(Workspace::new(root).unwrap()),
        cancel,
        approvals: ApprovalHandle::new(
            Arc::new(AllowGate),
            SessionId::from("s_test"),
            RunId::from("r_test"),
            None,
        ),
        events: kc_core::events::noop_sink(),
        settings: Arc::new(TestSettings),
        depth: 0,
        supports_images: false,
    }
}

fn find<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> &'a Arc<dyn Tool> {
    tools
        .iter()
        .find(|t| t.spec().name == name)
        .unwrap_or_else(|| {
            panic!(
                "tool `{name}` not exposed; got {:?}",
                tools.iter().map(|t| t.spec().name).collect::<Vec<_>>()
            )
        })
}

async fn call(
    tool: &Arc<dyn Tool>,
    root: &std::path::Path,
    args: serde_json::Value,
) -> ToolOutcome {
    tool.invoke(
        ToolCallRequest {
            call_id: "call_1".into(),
            name: tool.spec().name.clone(),
            arguments: args,
        },
        ctx(root, CancellationToken::new()),
    )
    .await
}

#[tokio::test]
async fn handshake_lists_namespaced_tools() {
    let dir = temp_dir("handshake");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    assert_eq!(host.plugin_count(), 1);
    host.start_all().await;

    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Ready, "{:?}", diags[0].error);
    let manifest = diags[0].manifest.as_ref().expect("manifest");
    assert_eq!(manifest.name, "fake");
    assert_eq!(manifest.protocol, "1");
    assert_eq!(
        manifest.hooks,
        vec![
            "before_prompt".to_string(),
            "on_run_end".to_string(),
            "on_notify".to_string()
        ]
    );
    assert!(diags[0].pid.is_some());
    // Eight tool behaviours plus the five notification-frame fixtures.
    assert_eq!(diags[0].tools.len(), 13);

    let tools = host.tools();
    let echo = find(&tools, "plugin__fake__echo");
    let spec = echo.spec();
    assert_eq!(
        spec.origin,
        ToolOrigin::Plugin {
            name: "fake".into()
        }
    );
    assert!(spec.description.contains("Echo the text argument back."));
    assert!(spec.description.contains("[plugin: fake v0.1.0]"));
    assert_eq!(
        spec.parameters["properties"]["text"]["type"],
        serde_json::json!("string")
    );
    // A plugin's declared class is a hint only: every plugin tool is exposed as
    // `external`, which always needs approval. `echo` declares `read_only` and
    // must still not be auto-approved.
    assert_eq!(echo.class(), ToolClass::External);
    assert!(echo.requires_approval());
    // Default class is `external`, which always needs approval.
    find(&tools, "plugin__fake__crash");
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn tools_call_round_trips_arguments_and_meta() {
    let dir = temp_dir("call");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();

    let echo = call(
        find(&tools, "plugin__fake__echo"),
        &dir,
        serde_json::json!({"text": "hi"}),
    )
    .await;
    assert!(echo.ok, "{}", echo.content);
    assert_eq!(echo.content, "echo: hi");

    let add = call(
        find(&tools, "plugin__fake__add"),
        &dir,
        serde_json::json!({"a": 2, "b": 40}),
    )
    .await;
    assert!(add.ok, "{}", add.content);
    assert_eq!(add.content, "42");
    assert_eq!(add.meta, Some(serde_json::json!({"total": 42})));

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn remote_tool_errors_become_failed_outcomes() {
    let dir = temp_dir("remote-error");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();

    let boom = call(
        find(&tools, "plugin__fake__boom"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(!boom.ok);
    assert!(
        boom.content.contains("boom failed on purpose"),
        "{}",
        boom.content
    );

    // `broken` is listed but not implemented: the plugin answers with a
    // JSON-RPC error, which must surface as an actionable failure.
    let unknown = call(
        find(&tools, "plugin__fake__broken"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(!unknown.ok);
    assert!(
        unknown.content.contains("unknown tool broken"),
        "{}",
        unknown.content
    );
    assert!(unknown.content.contains("-32601"), "{}", unknown.content);
    // A bad remote tool must not take the plugin down.
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn crash_mid_call_fails_the_request_and_marks_the_plugin_failed() {
    let dir = temp_dir("crash");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();

    let out = call(
        find(&tools, "plugin__fake__crash"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(!out.ok);
    assert!(
        out.content.contains("process stopped") || out.content.contains("exited"),
        "{}",
        out.content
    );

    // The slot must reflect the crash, not stay "ready".
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Failed);
    let error = diags[0].error.clone().unwrap_or_default();
    assert!(error.contains("exit code 3"), "{error}");
    // Failed plugins expose no tools.
    assert!(host.tools().is_empty());

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_dead_plugin_is_restarted_once_per_failed_call() {
    let dir = temp_dir("restart");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();
    let crash = find(&tools, "plugin__fake__crash").clone();
    let echo = find(&tools, "plugin__fake__echo").clone();

    let out = call(&crash, &dir, serde_json::json!({})).await;
    assert!(!out.ok);
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Failed);
    assert!(host.tools().is_empty());

    // The tool handle survives the crash: the next call restarts the process.
    let out = call(&echo, &dir, serde_json::json!({"text": "again"})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "echo: again");
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Ready, "{:?}", diags[0].error);
    assert_eq!(diags[0].restart_count, 1);
    assert_eq!(host.tools().len(), 12);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn restarts_are_capped_at_three_per_process_lifetime() {
    let dir = temp_dir("restart-cap");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let crash = find(&host.tools(), "plugin__fake__crash").clone();

    // First call kills the freshly started process (no restart yet), then three
    // calls each spend one restart before crashing again.
    for _ in 0..4 {
        let out = call(&crash, &dir, serde_json::json!({})).await;
        assert!(!out.ok, "crash must not report success: {}", out.content);
    }
    assert_eq!(host.diagnostics()[0].restart_count, 3);

    let out = call(&crash, &dir, serde_json::json!({})).await;
    assert!(!out.ok);
    assert!(
        out.content.contains("already been restarted 3 times"),
        "{}",
        out.content
    );

    // An explicit enable clears the budget again.
    let diag = host.set_enabled("fake", true).await.unwrap();
    assert_eq!(diag.status, PluginStatus::Ready);
    assert_eq!(diag.restart_count, 0);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn call_timeout_is_reported_with_the_budget() {
    let dir = temp_dir("timeout");
    let mut cfg = plugin("fake", "fake");
    cfg.env.insert("FAKE_PLUGIN_SLOW_SECS".into(), "10".into());
    let mut host = PluginHost::discover(&[cfg], &dir);
    host.set_timeouts(PluginTimeouts {
        call: Duration::from_millis(300),
        ..PluginTimeouts::default()
    });
    host.start_all().await;
    let tools = host.tools();

    let started = std::time::Instant::now();
    let out = call(
        find(&tools, "plugin__fake__slow"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.content.contains("timed out"), "{}", out.content);
    assert!(
        out.content.contains("plugin__fake__slow"),
        "{}",
        out.content
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // A timeout is not a crash: the plugin stays usable.
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn hooks_fan_out_and_feed_the_prompt() {
    let dir = temp_dir("hooks");
    let mut host = PluginHost::discover(
        &[plugin("fake-a", "fake-a"), plugin("fake-b", "fake-b")],
        &dir,
    );
    host.start_all().await;

    let notes = host.before_prompt("s_123", "/tmp/ws").await;
    assert_eq!(notes.len(), 2, "{notes:?}");
    // Plugin order, not completion order: the notes go into the system prompt
    // on every call, and a reshuffle would break the provider's prefix cache.
    assert!(notes[0].contains("fake-a"), "{notes:?}");
    assert!(notes[1].contains("fake-b"), "{notes:?}");
    for _ in 0..5 {
        assert_eq!(host.before_prompt("s_123", "/tmp/ws").await, notes);
    }

    // Unknown hooks simply produce nothing.
    let other = host
        .call_hook("on_model_request", serde_json::json!({}))
        .await;
    assert!(other.is_empty());
    // Reserved hook that the fixture does not advertise either.
    host.on_run_end("s_123", "r_1", "completed").await;
    host.on_tool_result("s_123", "read_file", true).await;

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn enable_and_disable_start_and_stop_the_process() {
    let dir = temp_dir("toggle");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    assert_eq!(host.tools().len(), 12);
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);

    let diag = host.set_enabled("fake", false).await.unwrap();
    assert_eq!(diag.status, PluginStatus::Disabled);
    assert!(diag.pid.is_none());
    assert!(diag.tools.is_empty());
    assert!(host.tools().is_empty());

    let diag = host.set_enabled("fake", true).await.unwrap();
    assert_eq!(diag.status, PluginStatus::Ready);
    assert!(diag.pid.is_some());
    assert_eq!(host.tools().len(), 12);
    // Enabling resets the restart budget.
    assert_eq!(diag.restart_count, 0);

    let err = host.set_enabled("nope", true).await.unwrap_err();
    assert_eq!(err.code(), "not_found");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn manifest_name_mismatch_fails_with_a_clear_error() {
    let dir = temp_dir("mismatch");
    let mut host = PluginHost::discover(&[plugin("mine", "someone-else")], &dir);
    host.start_all().await;
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Failed);
    let err = diags[0].error.clone().unwrap();
    assert!(err.contains("someone-else"), "{err}");
    assert!(err.contains("mine"), "{err}");
    assert!(host.tools().is_empty());
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_hanging_handshake_is_bounded_by_the_handshake_timeout() {
    let dir = temp_dir("hang");
    let mut cfg = plugin("fake", "fake");
    cfg.env.insert("FAKE_PLUGIN_HANG".into(), "1".into());
    let mut host = PluginHost::discover(&[cfg], &dir);
    host.set_timeouts(PluginTimeouts {
        handshake: Duration::from_millis(300),
        ..PluginTimeouts::default()
    });
    let started = std::time::Instant::now();
    host.start_all().await;
    assert!(started.elapsed() < Duration::from_secs(5));
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Failed);
    assert!(
        diags[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("initialize"),
        "{:?}",
        diags[0].error
    );
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_broken_plugin_does_not_block_other_plugins() {
    let dir = temp_dir("isolated");
    let mut broken = PluginConfig {
        name: "broken".into(),
        enabled: true,
        command: vec!["sh".into(), "-c".into(), "exit 7".into()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    broken
        .settings
        .insert("unused".into(), serde_json::json!(true));
    let mut host = PluginHost::discover(&[broken, plugin("fake", "fake")], &dir);
    host.start_all().await;

    let diags = host.diagnostics();
    assert_eq!(diags[0].name, "broken");
    assert_eq!(diags[0].status, PluginStatus::Failed);
    assert!(diags[0]
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("initialize"));
    assert_eq!(diags[1].name, "fake");
    assert_eq!(diags[1].status, PluginStatus::Ready);

    // The healthy plugin still answers.
    let tools = host.tools();
    let out = call(
        find(&tools, "plugin__fake__echo"),
        &dir,
        serde_json::json!({"text": "still here"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn shell_class_tools_are_hidden_without_the_shell_permission() {
    let dir = temp_dir("permissions");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let names: Vec<String> = host.tools().iter().map(|t| t.spec().name).collect();
    assert!(!names.contains(&"plugin__fake__deploy".to_string()));
    assert!(names.contains(&"plugin__fake__echo".to_string()));
    host.shutdown().await;

    // Granting the permission in the config exposes it. The class it is exposed
    // with is still `external` — the declared `shell` only gated exposure.
    let mut granted = plugin("fake", "fake");
    granted.permissions.push("shell".into());
    let mut host = PluginHost::discover(&[granted], &dir);
    host.start_all().await;
    let tools = host.tools();
    let deploy = find(&tools, "plugin__fake__deploy");
    assert_eq!(deploy.class(), ToolClass::External);
    assert!(deploy.requires_approval());
    let out = call(deploy, &dir, serde_json::json!({})).await;
    assert!(out.ok, "{}", out.content);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_stopped_plugin_reports_actionable_tool_errors() {
    let dir = temp_dir("stopped");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let echo = find(&host.tools(), "plugin__fake__echo").clone();
    host.set_enabled("fake", false).await.unwrap();

    // The tool object survives but the plugin is disabled: no panic, clear text.
    let out = call(&echo, &dir, serde_json::json!({"text": "x"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("disabled"), "{}", out.content);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn hooks_handle_stays_live_across_enable_and_disable() {
    let dir = temp_dir("hooks-handle");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;

    // The server can hand this to the runtime and keep the host for admin work.
    let hooks: Arc<dyn Hooks> = Arc::new(host.hooks_handle());
    assert_eq!(hooks.before_prompt("s_1", "/w").await.len(), 1);

    host.set_enabled("fake", false).await.unwrap();
    assert!(hooks.before_prompt("s_1", "/w").await.is_empty());

    host.set_enabled("fake", true).await.unwrap();
    assert_eq!(hooks.before_prompt("s_1", "/w").await.len(), 1);

    // PluginHost itself implements Hooks too.
    let direct = host.before_prompt("s_1", "/w").await;
    assert_eq!(direct.len(), 1);

    host.shutdown().await;
    assert!(hooks.before_prompt("s_1", "/w").await.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

/// Inline plugin whose manifest requests more permissions than the operator
/// configures for it.
const GREEDY_PLUGIN: &str = r#"
import json, sys

MANIFEST = {"name": "greedy", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"],
            "permissions": ["files.read", "shell", "network"], "hooks": []}
TOOLS = [
    {"name": "deploy", "description": "Shell-class tool.",
     "parameters": {"type": "object", "properties": {}}, "class": "shell"},
    {"name": "peek", "description": "Read-only tool.",
     "parameters": {"type": "object", "properties": {}}, "class": "read_only"},
]

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True, "content": "done"}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

fn greedy_config(name: &str, dir: &std::path::Path, permissions: Vec<String>) -> PluginConfig {
    let script = dir.join("greedy_plugin.py");
    if !script.exists() {
        std::fs::write(&script, GREEDY_PLUGIN).unwrap();
    }
    PluginConfig {
        name: name.to_string(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions,
        settings: BTreeMap::new(),
    }
}

#[tokio::test]
async fn configured_permissions_are_authoritative_over_the_manifest() {
    let dir = temp_dir("config-permissions");

    // Config grants only `files.read`; the manifest's `shell`/`network` are
    // ignored, so the shell-class tool is not exposed and diagnostics say why.
    let config = greedy_config("greedy", &dir, vec!["files.read".into()]);
    let mut host = PluginHost::discover(&[config], &dir);
    host.start_all().await;
    let diags = host.diagnostics_for("greedy").unwrap();
    assert_eq!(diags.status, PluginStatus::Ready, "{:?}", diags.error);
    let warning = diags.error.clone().unwrap_or_default();
    assert!(warning.contains("ignored"), "{warning}");
    assert!(warning.contains("shell"), "{warning}");
    assert!(warning.contains("network"), "{warning}");
    let names: Vec<String> = host.tools().iter().map(|t| t.spec().name).collect();
    assert!(
        !names.contains(&"plugin__greedy__deploy".to_string()),
        "manifest permissions must not override the config: {names:?}"
    );
    assert!(names.contains(&"plugin__greedy__peek".to_string()));
    host.shutdown().await;

    // No configured permissions: fail-closed. The manifest's declaration is
    // ignored, so the shell-class tool stays hidden and diagnostics say why.
    let config = greedy_config("greedy", &dir, Vec::new());
    let mut host = PluginHost::discover(&[config], &dir);
    host.start_all().await;
    let warning = host
        .diagnostics_for("greedy")
        .unwrap()
        .error
        .clone()
        .unwrap_or_default();
    assert!(warning.contains("ignored"), "{warning}");
    assert!(warning.contains("shell"), "{warning}");
    let names: Vec<String> = host.tools().iter().map(|t| t.spec().name).collect();
    assert!(
        !names.contains(&"plugin__greedy__deploy".to_string()),
        "an empty permissions list must not honour the manifest: {names:?}"
    );
    host.shutdown().await;

    // `permissions = ["*"]` is the explicit opt-in that honours the manifest.
    let config = greedy_config("greedy", &dir, vec!["*".into()]);
    let mut host = PluginHost::discover(&[config], &dir);
    host.start_all().await;
    assert!(
        host.diagnostics_for("greedy").unwrap().error.is_none(),
        "nothing is ignored when the operator trusts the manifest"
    );
    let names: Vec<String> = host.tools().iter().map(|t| t.spec().name).collect();
    assert!(names.contains(&"plugin__greedy__deploy".to_string()));
    host.shutdown().await;

    // The config can also grant more than the manifest declared.
    let config = greedy_config("greedy", &dir, vec!["shell".into()]);
    let mut host = PluginHost::discover(&[config], &dir);
    host.start_all().await;
    let names: Vec<String> = host.tools().iter().map(|t| t.spec().name).collect();
    assert!(names.contains(&"plugin__greedy__deploy".to_string()));
    host.shutdown().await;

    std::fs::remove_dir_all(&dir).ok();
}

/// Sets a variable in the *process* environment for the duration of a test.
///
/// Environment mutation is process-global and the test harness runs tests in
/// parallel threads, so every test that uses this must pick names no other test
/// touches and must restore the previous value even when the test panics.
struct EnvGuard {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

/// Inline plugin that reports its own process environment through a tool.
const ENV_PLUGIN: &str = r#"
import json, os, sys

MANIFEST = {"name": "envprobe", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "permissions": [], "hooks": []}
TOOLS = [{"name": "env", "description": "Report the process environment.",
          "parameters": {"type": "object", "properties": {}}, "class": "read_only"}]

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"ok": True, "content": json.dumps(dict(os.environ))}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

#[tokio::test]
async fn a_secret_in_the_parent_environment_does_not_reach_the_plugin() {
    let dir = temp_dir("env");
    let script = dir.join("env_plugin.py");
    std::fs::write(&script, ENV_PLUGIN).unwrap();

    // Names unique to this test: the environment is process-global.
    let _auth = EnvGuard::set("KARUCODE_AUTH_TOKEN", "super-secret-token");
    let _openai = EnvGuard::set("OPENAI_API_KEY", "sk-should-not-leak");
    let _aws = EnvGuard::set("AWS_SECRET_ACCESS_KEY", "aws-should-not-leak");
    let _github = EnvGuard::set("GITHUB_TOKEN", "gh-should-not-leak");
    let _custom = EnvGuard::set("KARUCODE_PLUGIN_TEST_LEAK", "nope");

    let mut cfg = PluginConfig {
        name: "envprobe".into(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    // The configured table wins and is deliberately not filtered: an operator
    // may hand a plugin its own credential on purpose.
    cfg.env
        .insert("PLUGIN_OWN_TOKEN".into(), "configured-on-purpose".into());

    let mut host = PluginHost::discover(&[cfg], &dir);
    host.start_all().await;
    let tools = host.tools();
    let out = call(
        find(&tools, "plugin__envprobe__env"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    let env: BTreeMap<String, String> = serde_json::from_str(&out.content).unwrap();

    for name in [
        "KARUCODE_AUTH_TOKEN",
        "OPENAI_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "GITHUB_TOKEN",
        "KARUCODE_PLUGIN_TEST_LEAK",
    ] {
        assert!(
            !env.contains_key(name),
            "{name} leaked into the plugin environment: {env:?}"
        );
    }
    // The allowlist still lets ordinary tooling start.
    assert!(
        env.contains_key("PATH"),
        "PATH must be passed through: {env:?}"
    );
    // The host's own protocol variables are set.
    assert_eq!(
        env.get("KARUCODE_PLUGIN_NAME").map(String::as_str),
        Some("envprobe")
    );
    assert_eq!(
        env.get("KARUCODE_PLUGIN_PROTOCOL").map(String::as_str),
        Some("1")
    );
    // The configured table wins.
    assert_eq!(
        env.get("PLUGIN_OWN_TOKEN").map(String::as_str),
        Some("configured-on-purpose")
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Inline plugin whose `before_prompt` hook returns far more than the cap.
const BIG_HOOK_PLUGIN: &str = r#"
import json, sys

MANIFEST = {"name": "bighook", "version": "0.1.0", "protocol": "1",
            "capabilities": ["hooks"], "permissions": [], "hooks": ["before_prompt"]}
BIG = "x" * (64 * 1024)

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": []}})
    elif method == "hooks/run":
        send({"jsonrpc": "2.0", "id": rid, "result": {"output": BIG}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

#[tokio::test]
async fn oversized_hook_output_is_capped_before_it_reaches_the_prompt() {
    let dir = temp_dir("bighook");
    let script = dir.join("big_hook_plugin.py");
    std::fs::write(&script, BIG_HOOK_PLUGIN).unwrap();
    let cfg = PluginConfig {
        name: "bighook".into(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    let mut host = PluginHost::discover(&[cfg], &dir);
    host.start_all().await;

    let notes = host.before_prompt("s_1", "/w").await;
    assert_eq!(notes.len(), 1, "{notes:?}");
    // 64 KiB in, at most 4 KiB plus the marker out.
    assert!(
        notes[0].len() <= 4 * 1024 + 32,
        "hook output must be capped, got {} bytes",
        notes[0].len()
    );
    assert!(
        notes[0].ends_with("[… truncated]"),
        "the truncation must be visible: {}",
        &notes[0][notes[0].len().saturating_sub(40)..]
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Panels (plugin-contributed inspector tabs)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_plugin_panel_round_trips_its_data() {
    let dir = temp_dir("panel-roundtrip");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;

    let groups = host.panels();
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0].plugin, "fake");
    assert_eq!(groups[0].version, "0.1.0");
    let ids: Vec<&str> = groups[0]
        .panels
        .iter()
        .map(|p| p.hosted_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "plugin__fake__todo",
            "plugin__fake__via-tool",
            "plugin__fake__broken",
            "plugin__fake__no-panels-read",
        ],
        "panel order must follow the manifest, not a hash map"
    );
    let todo = &groups[0].panels[0];
    assert_eq!(todo.id, "todo");
    assert_eq!(todo.title, "Todo");
    assert_eq!(todo.icon.as_deref(), Some("check"));
    assert_eq!(todo.refresh_ms, 5_000);

    // The session/workspace the caller asked about reaches the plugin.
    let ctx = ToolCallContext {
        session_id: "s_panel".into(),
        run_id: "r_panel".into(),
        project: Some("p_1".into()),
        workspace: "/tmp/ws".into(),
        ..Default::default()
    };
    let payload = host.panel("fake", "todo", &ctx).await.unwrap();
    assert_eq!(payload.title, "Todo");
    assert_eq!(payload.data["counts"]["done"], serde_json::json!(1));
    assert_eq!(
        payload.data["context"]["session_id"],
        serde_json::json!("s_panel")
    );
    assert_eq!(
        payload.data["context"]["workspace"],
        serde_json::json!("/tmp/ws")
    );

    // A panel that answers with no title keeps the manifest's label.
    let via_tool = host.panel("fake", "via-tool", &ctx).await.unwrap();
    assert_eq!(via_tool.title, "Via tool");
    assert_eq!(via_tool.data["from"], serde_json::json!("panels/read"));

    assert_eq!(
        host.diagnostics()[0].status,
        PluginStatus::Ready,
        "reading a panel must not disturb the plugin"
    );
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_failing_panel_reports_a_readable_error() {
    let dir = temp_dir("panel-error");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let ctx = ToolCallContext::default();

    let err = host.panel("fake", "broken", &ctx).await.unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("the todo list could not be read"),
        "{message}"
    );
    assert!(message.contains("broken"), "{message}");
    assert!(message.contains("fake"), "{message}");

    // An unknown panel names the ones the plugin does declare.
    let err = host.panel("fake", "nope", &ctx).await.unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert!(err.to_string().contains("plugin__fake__todo"), "{err}");

    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_plugin_without_the_method_falls_back_to_the_panels_tool() {
    let dir = temp_dir("panel-fallback");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let ctx = ToolCallContext::default();

    // The fixture answers `-32601` for this panel, exactly like a plugin built
    // before `panels/read` existed. The host falls back to the named tool and
    // uses its `meta` as the payload.
    let payload = host.panel("fake", "no-panels-read", &ctx).await.unwrap();
    assert_eq!(payload.title, "Legacy");
    assert!(payload.data.is_null(), "{:?}", payload.data);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_plugin_that_declares_no_panels_contributes_no_tabs() {
    // The fixture is the only plugin that declares panels; this inline one does
    // not, and must produce neither a tab nor an error.
    let dir = temp_dir("panel-none");
    let script = dir.join("plain.py");
    std::fs::write(
        &script,
        r#"
import json, sys
MANIFEST = {"name": "plain", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "permissions": [], "hooks": []}
for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        out = MANIFEST
    elif method == "tools/list":
        out = {"tools": []}
    elif method == "shutdown":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid, "result": None}) + "\n")
        sys.stdout.flush()
        break
    else:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid,
            "error": {"code": -32601, "message": "unknown"}}) + "\n")
        sys.stdout.flush()
        continue
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": rid, "result": out}) + "\n")
    sys.stdout.flush()
"#,
    )
    .unwrap();
    let cfg = PluginConfig {
        name: "plain".into(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    let mut host = PluginHost::discover(&[cfg], &dir);
    host.start_all().await;
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Ready, "{:?}", diags[0].error);
    assert!(diags[0].error.is_none(), "{:?}", diags[0].error);
    assert!(host.panels().is_empty());
    assert!(host.panels_handle().panels().is_empty());
    // Asking for a panel it never declared is a 404-style answer, not a panic.
    let err = host
        .panel("plain", "todo", &ToolCallContext::default())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Inline plugin that declares a panel naming a tool it does not advertise.
const BAD_PANEL_PLUGIN: &str = r#"
import json, sys

MANIFEST = {"name": "badpanel", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "permissions": [], "hooks": [],
            "panels": [{"id": "todo", "title": "Todo", "source": "tool",
                        "tool": "todo_list"}]}

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": []}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

#[tokio::test]
async fn a_panel_naming_a_missing_tool_is_a_discovery_error_not_a_crash() {
    let dir = temp_dir("panel-badtool");
    let script = dir.join("bad_panel.py");
    std::fs::write(&script, BAD_PANEL_PLUGIN).unwrap();
    let cfg = PluginConfig {
        name: "badpanel".into(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    let mut host = PluginHost::discover(&[cfg], &dir);
    host.start_all().await;

    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Failed);
    let error = diags[0].error.clone().unwrap_or_default();
    assert!(error.contains("todo_list"), "{error}");
    assert!(error.contains("tools/list"), "{error}");
    assert!(host.panels().is_empty());
    // The rest of the host is unaffected.
    assert!(host.tools().is_empty());

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// A panel read is not free: it must not hang on a plugin that never answers.
#[tokio::test]
async fn a_panel_read_is_bounded_by_the_call_timeout() {
    let dir = temp_dir("panel-timeout");
    let mut cfg = plugin("fake", "fake");
    cfg.env.insert("FAKE_PLUGIN_HANG_PANEL".into(), "1".into());
    let mut host = PluginHost::discover(&[cfg], &dir);
    host.set_timeouts(PluginTimeouts {
        call: Duration::from_millis(300),
        ..PluginTimeouts::default()
    });
    host.start_all().await;

    let started = std::time::Instant::now();
    let err = host
        .panel("fake", "todo", &ToolCallContext::default())
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    let message = err.to_string();
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("todo"), "{message}");

    // A timeout is not a crash: the plugin stays usable.
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Call context (tools/call)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tools_call_carries_the_session_and_workspace() {
    let dir = temp_dir("call-context");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();

    let out = call(
        find(&tools, "plugin__fake__context"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    let meta = out.meta.expect("meta");
    let context = &meta["context"];
    assert_eq!(context["session_id"], serde_json::json!("s_test"));
    assert_eq!(context["run_id"], serde_json::json!("r_test"));
    assert!(context["agent_id"].is_null());
    // The workspace root is canonicalised by `Workspace::new`, so compare it
    // against the canonical form rather than the temp path we passed in.
    let root = std::fs::canonicalize(&dir).unwrap();
    assert_eq!(
        context["workspace"],
        serde_json::json!(root.to_string_lossy())
    );
    assert_eq!(context["depth"], serde_json::json!(0));

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_plugin_that_ignores_the_call_context_keeps_working() {
    // The fixture's `echo` ignores `params.context` entirely: it must behave
    // exactly as it did before the field existed.
    let dir = temp_dir("call-context-ignored");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();
    let out = call(
        find(&tools, "plugin__fake__echo"),
        &dir,
        serde_json::json!({"text": "unchanged"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "echo: unchanged");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Write `<data_dir>/plugins/<dir_name>/plugin.toml` running the fixture, and
/// return the manifest path. `extra` is appended verbatim so a test can add keys.
fn write_manifest(data_dir: &std::path::Path, dir_name: &str, name: &str, extra: &str) -> PathBuf {
    let plugin_dir = data_dir.join("plugins").join(dir_name);
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = plugin_dir.join("plugin.toml");
    let body = format!(
        "name = \"{name}\"\ncommand = [\"sh\", \"{FIXTURE}\"]\nenv = {{ FAKE_PLUGIN_NAME = \"{name}\" }}\n{extra}"
    );
    std::fs::write(&manifest, body).unwrap();
    manifest
}

#[tokio::test]
async fn rescan_hot_loads_a_new_manifest_and_hot_unloads_it_again() {
    let dir = temp_dir("rescan-add");
    let mut host = PluginHost::discover(&[], &dir);
    host.start_all().await;
    assert_eq!(host.plugin_count(), 0);

    // A plugin installed into the data dir *after* startup is picked up without
    // restarting the server: this is the hot-load path.
    write_manifest(&dir, "fresh", "fresh", "");
    let report = host.rescan(&[]).await;
    assert_eq!(report.added, vec!["fresh".to_string()]);
    assert!(report.changed());
    let diags = host.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].status, PluginStatus::Ready, "{:?}", diags[0].error);
    assert_eq!(host.tools().len(), 12);

    // Removing the directory unloads it: no process, no tools, no diagnostics.
    std::fs::remove_dir_all(dir.join("plugins").join("fresh")).unwrap();
    let report = host.rescan(&[]).await;
    assert_eq!(report.removed, vec!["fresh".to_string()]);
    assert!(host.diagnostics().is_empty());
    assert!(host.tools().is_empty());

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_rescan_of_an_unchanged_tree_disturbs_nothing() {
    let dir = temp_dir("rescan-same");
    let cfg = plugin("fake", "fake");
    let mut host = PluginHost::discover(std::slice::from_ref(&cfg), &dir);
    host.start_all().await;
    let pid = host.diagnostics()[0].pid.expect("pid");

    let report = host.rescan(std::slice::from_ref(&cfg)).await;
    assert_eq!(report.unchanged, vec!["fake".to_string()]);
    assert!(!report.changed());
    // The same process must still be there: a watcher tick may not churn the
    // plugin's state (or lose its in-memory data) on every poll.
    assert_eq!(host.diagnostics()[0].pid, Some(pid));
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    assert_eq!(host.tools().len(), 12);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn rescan_restarts_a_plugin_whose_config_changed() {
    let dir = temp_dir("rescan-change");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let pid = host.diagnostics()[0].pid.expect("pid");

    let mut changed = plugin("fake", "fake");
    changed.env.insert("FAKE_PLUGIN_EXTRA".into(), "1".into());
    let report = host.rescan(&[changed]).await;
    assert_eq!(report.restarted, vec!["fake".to_string()]);
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Ready, "{:?}", diags[0].error);
    assert_ne!(
        diags[0].pid,
        Some(pid),
        "the process should have been replaced"
    );
    assert_eq!(host.tools().len(), 12);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn rescan_restarts_a_plugin_whose_file_was_rebuilt() {
    let dir = temp_dir("rescan-rebuild");
    // Copy the fixture so a test can "rebuild" it without touching the repo.
    let script = dir.join("plugin.sh");
    std::fs::copy(FIXTURE, &script).unwrap();
    let plugin_dir = dir.join("plugins").join("fake");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = plugin_dir.join("plugin.toml");
    std::fs::write(
        &manifest,
        format!(
            "name = \"fake\"\ncommand = [\"sh\", \"{}\"]\nenv = {{ FAKE_PLUGIN_NAME = \"fake\" }}\n",
            script.display()
        ),
    )
    .unwrap();

    let mut host = PluginHost::discover(&[], &dir);
    host.start_all().await;
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    let pid = host.diagnostics()[0].pid.expect("pid");

    // "Rebuild the plugin": the command line is byte-identical, only the file
    // changed. A pure config diff would miss this and keep running stale code.
    let mut body = std::fs::read_to_string(&script).unwrap();
    body.push_str("\n# rebuilt\n");
    std::fs::write(&script, body).unwrap();

    let report = host.rescan(&[]).await;
    assert_eq!(report.restarted, vec!["fake".to_string()]);
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Ready, "{:?}", diags[0].error);
    assert_ne!(
        diags[0].pid,
        Some(pid),
        "a rebuilt plugin must be respawned"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn reload_restarts_one_plugin_only() {
    let dir = temp_dir("reload-one");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let pid = host.diagnostics()[0].pid.expect("pid");

    let diag = host
        .reload("fake", &[plugin("fake", "fake")])
        .await
        .unwrap();
    assert_eq!(diag.status, PluginStatus::Ready);
    assert_ne!(diag.pid, Some(pid));
    assert_eq!(host.tools().len(), 12);

    let err = host
        .reload("nope", &[plugin("fake", "fake")])
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");

    // Reloading a plugin that was disabled at runtime rebuilds the slot but
    // must not start it: the operator's last action still wins.
    host.set_enabled("fake", false).await.unwrap();
    let diag = host
        .reload("fake", &[plugin("fake", "fake")])
        .await
        .unwrap();
    assert_eq!(diag.status, PluginStatus::Disabled);
    assert!(diag.pid.is_none());
    assert!(host.tools().is_empty());

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_runtime_disable_survives_a_rescan() {
    let dir = temp_dir("rescan-keep-disabled");
    let cfg = plugin("fake", "fake");
    let mut host = PluginHost::discover(std::slice::from_ref(&cfg), &dir);
    host.start_all().await;
    host.set_enabled("fake", false).await.unwrap();

    let mut changed = cfg;
    changed.env.insert("FAKE_PLUGIN_EXTRA".into(), "1".into());
    let report = host.rescan(&[changed]).await;
    assert_eq!(report.restarted, vec!["fake".to_string()]);
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Disabled);
    assert!(diags[0].pid.is_none());
    assert!(host.tools().is_empty());

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn source_stamp_moves_only_when_the_sources_move() {
    let dir = temp_dir("stamp");
    let host = PluginHost::discover(&[], &dir);
    let before = host.source_stamp();
    assert_eq!(
        before,
        host.source_stamp(),
        "the stamp must be stable at rest"
    );

    write_manifest(&dir, "fresh", "fresh", "");
    assert_ne!(
        before,
        host.source_stamp(),
        "a new manifest must move the stamp the watcher polls"
    );

    // A rebuilt command file must move it too. This is the case the *cached*
    // fingerprint cannot see, and the common one: `cargo build` rewrites the
    // binary without touching any config.
    let script = dir.join("plugin.sh");
    std::fs::copy(FIXTURE, &script).unwrap();
    let mut cfg = plugin("fake", "fake");
    cfg.command = vec!["sh".into(), script.display().to_string()];
    let mut host = PluginHost::discover(std::slice::from_ref(&cfg), &dir);
    let before = host.source_stamp();
    assert_eq!(before, host.source_stamp());
    let mut body = std::fs::read_to_string(&script).unwrap();
    body.push_str("\n# rebuilt\n");
    std::fs::write(&script, body).unwrap();
    assert_ne!(
        before,
        host.source_stamp(),
        "a rebuilt command file must move the stamp"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// Plugin-raised notifications (`notify`) and the `on_notify` hook
// ---------------------------------------------------------------------------

/// The whole point of `notify`: a plugin says something, and the *host* turns it
/// into a validated notice the server can publish. A plugin has no other way to
/// reach a browser.
#[tokio::test]
async fn a_plugin_can_raise_a_notification() {
    let dir = temp_dir("notify-good");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    // Subscribed before start_all: a plugin may notify during its own startup.
    let mut notices = host.listen_notifications();
    host.start_all().await;
    let tools = host.tools();

    let outcome = call(
        find(&tools, "plugin__fake__notify_good"),
        &dir,
        serde_json::json!({"kind": "needs_input", "title": "Deploy needs a yes"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");

    let notice = tokio::time::timeout(Duration::from_secs(5), notices.recv())
        .await
        .expect("a notice within 5s")
        .expect("channel open");
    assert_eq!(notice.plugin, "fake");
    assert_eq!(notice.kind, kc_core::notify::NotifyKind::NeedsInput);
    assert_eq!(notice.title, "Deploy needs a yes");
    assert_eq!(notice.body, "the fixture asked for attention");
    assert!(notice.sticky, "the fixture asked to stay on screen");
    // The session comes from the call context the host forwards.
    assert_eq!(notice.session_id, "s_test");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Every rejection rule, in one test: a malformed frame is dropped rather than
/// becoming a half-valid notification, and it must not disturb the plugin.
#[tokio::test]
async fn malformed_notifications_are_dropped_without_disturbing_the_plugin() {
    let dir = temp_dir("notify-bad");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    let mut notices = host.listen_notifications();
    host.start_all().await;
    let tools = host.tools();

    for (tool, args) in [
        ("plugin__fake__notify_bad_kind", serde_json::json!({})),
        ("plugin__fake__notify_no_title", serde_json::json!({})),
        ("plugin__fake__notify_junk", serde_json::json!({})),
    ] {
        let outcome = call(find(&tools, tool), &dir, args).await;
        // The tool itself succeeded: sending a bad notification is the *plugin's*
        // mistake, and refusing it must not look like a tool failure.
        assert!(outcome.ok, "{tool} should still answer: {outcome:?}");
    }

    // Nothing reached the channel...
    assert!(
        tokio::time::timeout(Duration::from_millis(150), notices.recv())
            .await
            .is_err(),
        "no malformed notification may reach the server"
    );
    // ...and the plugin is still healthy and answering.
    let outcome = call(
        find(&tools, "plugin__fake__echo"),
        &dir,
        serde_json::json!({"text": "still alive"}),
    )
    .await;
    assert!(outcome.ok);
    assert_eq!(outcome.content, "echo: still alive");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// A plugin in a loop must not be able to flood someone's desktop: the burst
/// allowance is spent and the rest are dropped.
#[tokio::test]
async fn a_notification_flood_is_rate_limited() {
    let dir = temp_dir("notify-flood");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    let mut notices = host.listen_notifications();
    host.start_all().await;
    let tools = host.tools();

    let outcome = call(
        find(&tools, "plugin__fake__notify_flood"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(outcome.ok, "a flood must not fail the call: {outcome:?}");

    let mut received = 0;
    while tokio::time::timeout(Duration::from_millis(150), notices.recv())
        .await
        .is_ok()
    {
        received += 1;
        assert!(received <= 10, "the fixture only sent 10");
    }
    // Ten were sent; the burst allowance is five. The exact number depends on how
    // much wall clock the calls took, so the assertion is the safety property:
    // some were dropped, and not all ten came through.
    assert!(
        received > 0 && received < 10,
        "the flood must be partially dropped, got {received} of 10"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// The "out" half: a plugin that advertises `on_notify` hears about notifications
/// the runtime emits, which is how a plugin bridges karucode to an external
/// notifier without karucode growing that integration.
#[tokio::test]
async fn on_notify_reaches_a_plugin_that_advertises_it() {
    let dir = temp_dir("notify-hook");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;

    let observer = {
        // `PluginHost` implements `Hooks`; the observer is what the runtime uses.
        let hooks: Arc<dyn Hooks> = Arc::new(host.hooks_handle());
        hooks
            .notifier()
            .expect("the plugin host observes notifications")
    };
    // The fixture echoes the hook to stderr and answers non-null, so a completed
    // round trip is the proof it arrived.
    observer
        .on_notify(serde_json::json!({
            "session_id": "s_test",
            "run_id": "r_test",
            "kind": "run_finished",
            "title": "Run finished",
            "body": "all done",
            "source": "runtime",
        }))
        .await;

    // No assertion on the return value: `on_notify` is fire-and-forget by design,
    // and the call above must simply not hang or panic. The plugin answering is
    // what `fan_out` waits for within its 5s budget.
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Without a listener, a `notify` frame is dropped silently: the host must work
/// in an embedding that does not care about notifications.
#[tokio::test]
async fn notify_without_a_listener_is_harmless() {
    let dir = temp_dir("notify-nolistener");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let tools = host.tools();

    let outcome = call(
        find(&tools, "plugin__fake__notify_good"),
        &dir,
        serde_json::json!({"title": "nobody is listening"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");

    // Still usable afterwards.
    let outcome = call(
        find(&tools, "plugin__fake__echo"),
        &dir,
        serde_json::json!({"text": "fine"}),
    )
    .await;
    assert!(outcome.ok);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}
