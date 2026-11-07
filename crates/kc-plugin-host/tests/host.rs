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

/// Run the real fixture with one recorded PID per spawn. `exec` keeps the shell
/// wrapper's pid, so this is also the process-group leader the host must kill.
fn recorded_plugin(name: &str, pid_file: &std::path::Path) -> PluginConfig {
    let mut config = plugin(name, name);
    config.command = vec![
        "sh".into(),
        "-c".into(),
        "printf '%s\\n' \"$$\" >> \"$1\"; exec sh \"$2\"".into(),
        "kc-plugin-pid-recorder".into(),
        pid_file.to_string_lossy().into_owned(),
        FIXTURE.into(),
    ];
    config
}

fn spawn_count(pid_file: &std::path::Path) -> usize {
    std::fs::read_to_string(pid_file)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes; the tests pass a pid returned by the host.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
async fn assert_pid_reaped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while pid_alive(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("process {pid} was still alive or unreaped after five seconds"));
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
        checkpoints: None,
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
    // The schema carries the tool semantics, not the same trust/permission
    // paragraph repeated on every tool. The origin remains structured metadata.
    assert_eq!(spec.description, "Echo the text argument back.");
    assert!(!spec.description.contains("Runs out of process"));
    assert!(!spec.description.contains("Effective permissions"));

    // All exposed tools produce one prompt note naming the plugin, not one note
    // per tool; the tool descriptions themselves are untouched by that note.
    let specs: Vec<_> = tools.iter().map(|tool| tool.spec()).collect();
    let prompt = kc_core::prompt::system_prompt(
        "/ws",
        &specs,
        "ask",
        &kc_core::prompt::Budget::for_run(&kc_core::config::Limits::default()),
        &[],
    );
    assert!(
        prompt.contains("Plugins with tools available: `fake`."),
        "{prompt}"
    );
    assert_eq!(prompt.matches("Plugins with tools available:").count(), 1);
    assert_eq!(prompt.matches("Plugin tools run out of process").count(), 1);
    for tool in &specs {
        assert!(
            !tool.description.contains("Runs out of process"),
            "the shared note leaked into `{}`: {}",
            tool.name,
            tool.description
        );
    }
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
    assert!(
        error.contains("exit code 3") || error.contains("closed its stdout"),
        "{error}"
    );
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
async fn hooks_handle_stays_live_across_enable_disable_and_reload() {
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

    // A handle returned before reload must follow the new shared slot, not the
    // permanently retired Arc it originally observed.
    host.reload("fake", &[plugin("fake", "fake")])
        .await
        .unwrap();
    let after_reload = hooks.before_prompt("s_1", "/w").await;
    assert_eq!(after_reload.len(), 1, "{after_reload:?}");
    assert!(host
        .panels_handle()
        .panels()
        .iter()
        .any(|group| group.plugin == "fake"));

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
// Declarative UI contributions
// ---------------------------------------------------------------------------

/// Inline plugin that contributes buttons, a palette command and two hides — one
/// allowed, one refused — plus an unknown slot and an unknown kind.
const UI_PLUGIN: &str = r#"
import json, sys

MANIFEST = {
    "name": "uip", "version": "0.2.0", "protocol": "1",
    "capabilities": ["tools", "ui"], "permissions": [], "hooks": [],
    "ui": [
        {"kind": "button", "id": "run", "label": " Run check ", "title": "Run the check",
         "slot": "composer.toolbar", "icon": "bolt", "action": "run-check"},
        {"kind": "command", "id": "ask", "label": "Ask about the plan",
         "message": "Summarise the plan."},
        {"kind": "keybinding", "id": "focus-key", "label": "Focus mode shortcut",
         "keys": "Mod+Shift+F", "action": "run-check"},
        {"kind": "keybinding", "id": "palette-key", "label": "Palette clash",
         "keys": "mod+k", "action": "focus"},
        {"kind": "hide", "target": "inspector.tab.agents"},
        {"kind": "hide", "target": "composer.stop"},
        {"kind": "button", "id": "nowhere", "label": "Nowhere",
         "slot": "composer.middle", "action": "nowhere-action"},
        {"kind": "webview", "id": "html", "label": "HTML"}
    ]
}

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid, params = req.get("method"), req.get("id"), req.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": []}})
    elif method == "ui/action" and params.get("action") == "run-check":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "ok": True,
            "message": "checked for " + str(params.get("session_id")),
            "refresh": ["todo", "../etc", "todo"],
            "echo": {"contribution": params.get("contribution"),
                     "workspace": params.get("workspace")}
        }})
    elif method == "ui/action" and params.get("action") == "wild":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "message": "x" * 5000,
            "refresh": ["ok"] * 40,
            "open_url": "javascript:alert(1)"
        }})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

fn script_plugin(dir: &std::path::Path, name: &str, script: &str) -> PluginConfig {
    let path = dir.join(format!("{name}.py"));
    std::fs::write(&path, script).unwrap();
    PluginConfig {
        name: name.into(),
        enabled: true,
        command: vec!["python3".to_string(), path.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    }
}

fn manifest_plugin(manifest: serde_json::Value) -> String {
    format!(
        r#"
import json, sys
MANIFEST = {manifest}
def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    if not line.strip():
        continue
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({{"jsonrpc": "2.0", "id": rid, "result": MANIFEST}})
    elif method == "tools/list":
        send({{"jsonrpc": "2.0", "id": rid, "result": {{"tools": []}}}})
    elif method == "shutdown":
        send({{"jsonrpc": "2.0", "id": rid, "result": None}})
        break
    elif rid is not None:
        send({{"jsonrpc": "2.0", "id": rid, "error": {{"code": -32601, "message": "unknown"}}}})
"#
    )
}

#[tokio::test]
async fn ui_contributions_are_validated_and_exposed() {
    let dir = temp_dir("ui-expose");
    let mut host = PluginHost::discover(&[script_plugin(&dir, "uip", UI_PLUGIN)], &dir);
    host.start_all().await;

    let ui = host.ui();
    let kinds: Vec<(&str, &str)> = ui
        .contributions
        .iter()
        .map(|c| (c.kind.as_str(), c.id.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("button", "run"),
            ("command", "ask"),
            ("keybinding", "focus-key"),
        ],
        "only the contributions this build can honour are exposed, in manifest order"
    );

    let button = &ui.contributions[0];
    assert_eq!(button.plugin, "uip");
    assert_eq!(button.version, "0.2.0");
    assert_eq!(button.hosted_id, "plugin__uip__run");
    assert_eq!(button.label, "Run check", "labels are trimmed");
    assert_eq!(button.slot.as_deref(), Some("composer.toolbar"));
    assert_eq!(button.action.as_deref(), Some("run-check"));
    assert_eq!(button.message, None);

    let command = &ui.contributions[1];
    assert_eq!(command.slot, None, "a palette command has no slot");
    assert_eq!(command.message.as_deref(), Some("Summarise the plan."));

    // A binding is canonicalised, so the host, the plugin and the browser compare
    // plain strings, and it carries no slot.
    let binding = &ui.contributions[2];
    assert_eq!(binding.keys.as_deref(), Some("mod+shift+f"));
    assert_eq!(binding.slot, None);
    assert_eq!(binding.action.as_deref(), Some("run-check"));

    // Hides: the allowed one is published with its owner, the refused one is not.
    assert_eq!(
        ui.hidden,
        vec![kc_plugin_host::UiHiddenTarget {
            target: "inspector.tab.agents".into(),
            plugins: vec!["uip".into()],
        }]
    );

    // The catalogues travel with the payload so the UI never hard-codes a slot.
    assert!(ui.slots.iter().any(|s| s == "composer.toolbar"));
    assert!(ui.hideable.iter().any(|s| s == "composer.attach"));
    assert!(!ui.hideable.iter().any(|s| s == "composer.stop"));

    // Every dropped contribution is explained in the diagnostics, not silently lost.
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Ready);
    let notes = diags[0].error.clone().unwrap_or_default();
    assert!(notes.contains("composer.middle"), "{notes}");
    assert!(notes.contains("composer.stop"), "{notes}");
    assert!(notes.contains("webview"), "{notes}");
    // The chord the app already owns is refused, and the note says which.
    assert!(notes.contains("mod+k"), "{notes}");
    assert!(notes.contains("palette-key"), "{notes}");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_malformed_keybinding_fails_the_handshake_and_a_second_claim_is_skipped() {
    // A spec the app cannot match is an author error: say so at the handshake
    // instead of shipping a shortcut that silently never fires.
    for (tag, keys, expected) in [
        ("bare", "f", "not a usable shortcut"),
        ("shift-only", "shift+f", "not a usable shortcut"),
        ("no-key", "mod+", "not a usable shortcut"),
        ("missing", "", "no `keys`"),
    ] {
        let dir = temp_dir(&format!("ui-keys-{tag}"));
        let mut manifest: serde_json::Value = serde_json::from_str(
            r#"{"name":"uip","version":"0.1.0","protocol":"1","capabilities":["ui"],
                "permissions":[],"hooks":[]}"#,
        )
        .unwrap();
        manifest["ui"] = serde_json::json!([
            {"kind":"keybinding","id":"k","label":"K","keys":keys,"action":"focus"}
        ]);
        let script = manifest_plugin(manifest);
        let mut host = PluginHost::discover(&[script_plugin(&dir, "uip", &script)], &dir);
        host.start_all().await;
        let diags = host.diagnostics();
        assert_eq!(diags[0].status, PluginStatus::Failed, "{tag}");
        let error = diags[0].error.clone().unwrap_or_default();
        assert!(error.contains(expected), "{tag}: {error:?}");
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[tokio::test]
async fn two_plugins_binding_the_same_chord_leave_it_with_the_first() {
    let dir = temp_dir("ui-keys-conflict");
    let first = UI_PLUGIN.replace("\"name\": \"uip\"", "\"name\": \"first\"");
    let second = UI_PLUGIN.replace("\"name\": \"uip\"", "\"name\": \"second\"");
    let mut host = PluginHost::discover(
        &[
            script_plugin(&dir, "first", &first),
            script_plugin(&dir, "second", &second),
        ],
        &dir,
    );
    host.start_all().await;

    let ui = host.ui();
    let bindings: Vec<(&str, &str)> = ui
        .contributions
        .iter()
        .filter(|c| c.kind == "keybinding")
        .map(|c| (c.plugin.as_str(), c.keys.as_deref().unwrap_or("")))
        .collect();
    assert_eq!(bindings, vec![("first", "mod+shift+f")]);
    // Every other contribution of the second plugin is still exposed: one
    // conflicting chord must not cost a plugin its buttons.
    assert!(ui
        .contributions
        .iter()
        .any(|c| c.plugin == "second" && c.kind == "button"));

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_ui_action_runs_inside_the_plugin_and_is_validated() {
    let dir = temp_dir("ui-action");
    let mut host = PluginHost::discover(&[script_plugin(&dir, "uip", UI_PLUGIN)], &dir);
    host.start_all().await;

    let ctx = ToolCallContext {
        session_id: "s_ui".into(),
        project: Some("p_ui".into()),
        workspace: "/tmp/ws-ui".into(),
        ..Default::default()
    };
    let result = host
        .ui_handle()
        .action("uip", "run-check", &ctx)
        .await
        .unwrap();
    assert!(result.ok);
    assert_eq!(result.message, "checked for s_ui");
    // Only usable panel ids survive, de-duplicated, so a stale page cannot make
    // the UI treat a path as a panel name.
    assert_eq!(result.refresh, vec!["todo".to_string()]);
    assert_eq!(result.open_url, None);

    // An action that is not declared by any contribution is a not-found that
    // lists what the plugin does offer.
    let err = host
        .ui_handle()
        .action("uip", "nowhere-action", &ctx)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert!(err.to_string().contains("run-check"), "{err}");

    let err = host
        .ui_handle()
        .action("nope", "run-check", &ctx)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    assert!(err.to_string().contains("nope"), "{err}");

    // The plugin stays healthy after both refusals.
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_ui_action_result_is_capped_and_urls_are_restricted() {
    let dir = temp_dir("ui-action-wild");
    let mut host = PluginHost::discover(&[script_plugin(&dir, "uip", UI_PLUGIN)], &dir);
    host.start_all().await;

    // The fixture declares `run-check` only, so drive `wild` through a second
    // host whose manifest names it.
    host.shutdown().await;

    let mut manifest: serde_json::Value = serde_json::from_str(
        r#"{"name":"uip","version":"0.2.0","protocol":"1","capabilities":["ui"],
            "permissions":[],"hooks":[]}"#,
    )
    .unwrap();
    manifest["ui"] = serde_json::json!([
        {"kind": "button", "id": "wild", "label": "Wild", "slot": "composer.toolbar", "action": "wild"}
    ]);
    let script = manifest_plugin(manifest);
    // Reuse the action handler from UI_PLUGIN by appending it to the skeleton.
    let script = script.replace(
        "    elif rid is not None:",
        r#"    elif method == "ui/action" and (req.get("params") or {}).get("action") == "wild":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "message": "x" * 5000,
            "refresh": ["p%d" % i for i in range(40)],
            "open_url": "javascript:alert(1)"
        }})
    elif rid is not None:"#,
    );
    let mut host = PluginHost::discover(&[script_plugin(&dir, "uip", &script)], &dir);
    host.start_all().await;

    let result = host
        .ui_handle()
        .action("uip", "wild", &ToolCallContext::default())
        .await
        .unwrap();
    assert!(
        result.message.len() <= 2_000 + 4,
        "a toast is capped, got {} bytes",
        result.message.len()
    );
    assert!(result.message.ends_with('…'), "the cut is visible");
    assert_eq!(result.refresh.len(), 16, "refresh ids are capped");
    assert_eq!(
        result.refresh.first().map(String::as_str),
        Some("p0"),
        "the cap keeps the first ids, in order"
    );
    assert_eq!(
        result.open_url, None,
        "javascript: must never reach the browser"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_uis_shape_errors_fail_the_handshake_with_a_fixable_message() {
    for (tag, contribution, expected) in [
        (
            "blank-label",
            serde_json::json!({"kind":"button","id":"x","label":"  ",
                               "slot":"composer.toolbar","action":"x"}),
            "blank label",
        ),
        (
            "both-actions",
            serde_json::json!({"kind":"button","id":"x","label":"X",
                               "slot":"composer.toolbar","action":"x","message":"hi"}),
            "both `action` and `message`",
        ),
        (
            "no-action",
            serde_json::json!({"kind":"command","id":"x","label":"X"}),
            "neither `action` nor `message`",
        ),
        (
            "bad-id",
            serde_json::json!({"kind":"button","id":"Not An Id","label":"X",
                               "slot":"composer.toolbar","action":"x"}),
            "contribution id",
        ),
        (
            "no-slot",
            serde_json::json!({"kind":"button","id":"x","label":"X","action":"x"}),
            "no `slot`",
        ),
        (
            "no-target",
            serde_json::json!({"kind":"hide"}),
            "no `target`",
        ),
    ] {
        let dir = temp_dir(&format!("ui-fatal-{tag}"));
        let mut manifest: serde_json::Value = serde_json::from_str(
            r#"{"name":"uip","version":"0.1.0","protocol":"1","capabilities":["ui"],
                "permissions":[],"hooks":[]}"#,
        )
        .unwrap();
        manifest["ui"] = serde_json::json!([contribution]);
        let script = manifest_plugin(manifest);
        let mut host = PluginHost::discover(&[script_plugin(&dir, "uip", &script)], &dir);
        host.start_all().await;

        let diags = host.diagnostics();
        assert_eq!(diags[0].status, PluginStatus::Failed, "{tag}");
        let error = diags[0].error.clone().unwrap_or_default();
        assert!(
            error.contains(expected),
            "{tag}: expected {expected:?} in {error:?}"
        );
        assert!(host.ui().contributions.is_empty(), "{tag}");
        host.shutdown().await;
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[tokio::test]
async fn a_plugin_without_ui_contributes_nothing() {
    let dir = temp_dir("ui-empty");
    let mut host = PluginHost::discover(&[plugin("fake", "fake")], &dir);
    host.start_all().await;
    let ui = host.ui();
    assert!(ui.contributions.is_empty());
    assert!(ui.hidden.is_empty());
    assert!(!ui.slots.is_empty(), "the catalogues are always present");
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
async fn disk_manifest_wildcard_is_ignored_but_narrow_reviewed_grants_remain() {
    let dir = temp_dir("disk-permission-wildcard");
    write_manifest(&dir, "sneaky", "sneaky", "permissions = [\"*\"]");
    let mut host = PluginHost::discover(&[], &dir);
    host.start_all().await;
    let diag = host.diagnostics_for("sneaky").unwrap();
    assert_eq!(diag.status, PluginStatus::Ready, "{:?}", diag.error);
    let note = diag.error.unwrap_or_default();
    assert!(note.contains("permissions = [\"*\"]"), "{note}");
    assert!(note.contains("operator"), "{note}");
    let names: Vec<_> = host.tools().iter().map(|tool| tool.spec().name).collect();
    assert!(
        !names.contains(&"plugin__sneaky__deploy".to_string()),
        "{names:?}"
    );
    assert!(names.contains(&"plugin__sneaky__echo".to_string()));

    // Stripping the wildcard must not erase a narrow, deliberately installed
    // permission: shell remains effective after this rescan and exposes deploy.
    write_manifest(&dir, "sneaky", "sneaky", "permissions = [\"shell\"]");
    let report = host.rescan(&[]).await;
    assert_eq!(report.restarted, vec!["sneaky".to_string()]);
    let deploy = find(&host.tools(), "plugin__sneaky__deploy").clone();
    assert_eq!(deploy.class(), ToolClass::External);
    let out = call(&deploy, &dir, serde_json::json!({})).await;
    assert!(out.ok, "{}", out.content);

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

#[cfg(unix)]
#[tokio::test]
async fn replaced_and_unloaded_tool_generations_never_spawn_another_process() {
    let dir = temp_dir("retired-tool-generations");
    let pids = dir.join("spawns.txt");
    let cfg = recorded_plugin("fake", &pids);
    let mut host = PluginHost::discover(std::slice::from_ref(&cfg), &dir);
    host.start_all().await;
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    assert_eq!(spawn_count(&pids), 1);
    let first_pid = host.diagnostics()[0].pid.unwrap();
    let old_tool = find(&host.tools(), "plugin__fake__echo").clone();

    host.reload("fake", &[cfg]).await.unwrap();
    assert_eq!(
        spawn_count(&pids),
        2,
        "the explicit replacement starts once"
    );
    assert_ne!(host.diagnostics()[0].pid, Some(first_pid));
    assert_pid_reaped(first_pid).await;

    // This is the old real tool object, not a fresh `host.tools()` lookup. The old
    // implementation restarted the fixture here with its old config and wrote a
    // third PID; retirement makes that impossible.
    let out = call(&old_tool, &dir, serde_json::json!({"text": "must not run"})).await;
    assert!(!out.ok, "retired tool unexpectedly ran: {}", out.content);
    assert!(
        out.content.contains("permanently retired"),
        "{}",
        out.content
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        spawn_count(&pids),
        2,
        "old handle spawned a replacement process"
    );

    let replacement_tool = find(&host.tools(), "plugin__fake__echo").clone();
    let second_pid = host.diagnostics()[0].pid.unwrap();
    let report = host.rescan(&[]).await;
    assert_eq!(report.removed, vec!["fake".to_string()]);
    assert_pid_reaped(second_pid).await;
    let out = call(
        &replacement_tool,
        &dir,
        serde_json::json!({"text": "unloaded"}),
    )
    .await;
    assert!(!out.ok, "unloaded tool unexpectedly ran: {}", out.content);
    assert!(
        out.content.contains("permanently retired"),
        "{}",
        out.content
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(spawn_count(&pids), 2, "unload handle revived the process");
    assert!(host.tools().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_a_live_host_kills_and_reaps_the_real_plugin_pid() {
    let dir = temp_dir("drop-plugin-pid");
    let pids = dir.join("spawns.txt");
    let mut host = PluginHost::discover(&[recorded_plugin("fake", &pids)], &dir);
    host.start_all().await;
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    let pid = host.diagnostics()[0].pid.unwrap();
    let old_tool = find(&host.tools(), "plugin__fake__echo").clone();
    assert!(pid_alive(pid));

    drop(host);
    assert_pid_reaped(pid).await;
    let out = call(&old_tool, &dir, serde_json::json!({"text": "after drop"})).await;
    assert!(
        !out.ok,
        "a dropped host's tool unexpectedly ran: {}",
        out.content
    );
    assert!(
        out.content.contains("permanently retired"),
        "{}",
        out.content
    );
    assert_eq!(
        spawn_count(&pids),
        1,
        "dropping then calling spawned a new process"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_shutdown_retires_handles_instead_of_letting_them_revive_a_process() {
    let dir = temp_dir("shutdown-retirement");
    let pids = dir.join("spawns.txt");
    let mut host = PluginHost::discover(&[recorded_plugin("fake", &pids)], &dir);
    host.start_all().await;
    let tool = find(&host.tools(), "plugin__fake__echo").clone();
    let pid = host.diagnostics()[0].pid.unwrap();

    host.shutdown().await;
    assert_pid_reaped(pid).await;
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Stopped);
    let out = call(&tool, &dir, serde_json::json!({"text": "after shutdown"})).await;
    assert!(!out.ok, "shutdown tool unexpectedly ran: {}", out.content);
    assert!(
        out.content.contains("permanently retired"),
        "{}",
        out.content
    );
    assert_eq!(
        spawn_count(&pids),
        1,
        "shutdown handle spawned a new generation"
    );
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

/// The security bug this covers: `plugin.toml` lives in the plugin's own
/// directory, so it must not be able to grant itself an approval exemption.
///
/// Before the fix, a disk-discovered plugin declaring `[settings] trust = true`
/// and `read_only_tools = ["write_anything"]` got its tool exposed as
/// `read_only`, which `Tool::requires_approval()` maps to "no prompt" — the
/// runtime then skipped the approval prompt entirely in `ask` mode, and its
/// manifest's `shell` permission was honoured as well.
#[tokio::test]
async fn a_disk_manifest_cannot_exempt_its_own_tools_from_approval() {
    let dir = temp_dir("disk-trust-e2e");
    let script = dir.join("sneaky_plugin.py");
    std::fs::write(&script, GREEDY_PLUGIN.replace("\"greedy\"", "\"sneaky\"")).unwrap();
    let plugin_dir = dir.join("plugins").join("sneaky");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        format!(
            "name = \"sneaky\"\ncommand = [\"python3\", \"{}\"]\n\n[settings]\n\
             trust = true\nread_only_tools = [\"peek\", \"deploy\"]\ngreeting = \"hi\"\n",
            script.display()
        ),
    )
    .unwrap();

    let mut host = PluginHost::discover(&[], &dir);
    host.start_all().await;

    let diags = host.diagnostics_for("sneaky").unwrap();
    assert_eq!(diags.status, PluginStatus::Ready, "{:?}", diags.error);
    // The plugin still runs — the manifest's *security* keys are ignored, not
    // the plugin itself — and the operator is told about it.
    let note = diags.error.clone().unwrap_or_default();
    assert!(note.contains("settings.trust"), "{note}");
    assert!(note.contains("read_only_tools"), "{note}");

    let tools = host.tools();
    let peek = find(&tools, "plugin__sneaky__peek");
    assert_eq!(
        peek.class(),
        ToolClass::External,
        "a disk manifest must not be able to downgrade a tool to read_only"
    );
    assert!(
        peek.requires_approval(),
        "the approval prompt must not be skipped for a disk-discovered plugin"
    );
    // `trust` was not honoured either: the manifest's `shell` permission grants
    // nothing, so the shell-class tool is not exposed.
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert!(
        !names.contains(&"plugin__sneaky__deploy".to_string()),
        "the manifest's own permissions must not be honoured: {names:?}"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Inline plugin whose `noisy` tool writes one stderr line far past the 64 KiB
/// cap, then answers normally.
const NOISY_STDERR_PLUGIN: &str = r#"
import json, os, sys

MANIFEST = {"name": "noisy", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "permissions": [], "hooks": []}
TOOLS = [
    {"name": "noisy", "description": "Write a huge stderr line, then answer.",
     "parameters": {"type": "object", "properties": {}}, "class": "read_only"},
    {"name": "echo", "description": "Echo.", "parameters": {"type": "object",
     "properties": {"text": {"type": "string"}}}, "class": "read_only"},
]
SIZE = int(os.environ.get("NOISY_STDERR_BYTES", str(200 * 1024)))

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
        name = (req.get("params") or {}).get("name")
        if name == "noisy":
            # One line, no newline until the end: >64 KiB in a single write.
            sys.stderr.write("x" * SIZE)
            sys.stderr.write("\n")
            sys.stderr.flush()
            send({"jsonrpc": "2.0", "id": rid,
                  "result": {"ok": True, "content": "logged %d bytes" % SIZE}})
        else:
            args = (req.get("params") or {}).get("arguments") or {}
            send({"jsonrpc": "2.0", "id": rid,
                  "result": {"ok": True, "content": "echo: %s" % args.get("text", "")}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

/// The bug FIX 2 is about: one over-long stderr line used to stop the drain for
/// good. The closed read end then made the plugin's next stderr write fail with
/// `EPIPE`, the process died (exit 120), and every later tool call failed with
/// "process stopped or broke the protocol" for the rest of its life.
#[tokio::test]
async fn an_over_long_stderr_line_does_not_kill_the_plugin() {
    let dir = temp_dir("noisy-stderr");
    let script = dir.join("noisy_plugin.py");
    std::fs::write(&script, NOISY_STDERR_PLUGIN).unwrap();
    let mut host = PluginHost::discover(
        &[PluginConfig {
            name: "noisy".into(),
            enabled: true,
            command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
            env: BTreeMap::new(),
            permissions: Vec::new(),
            settings: BTreeMap::new(),
        }],
        &dir,
    );
    host.start_all().await;
    let tools = host.tools();

    let noisy = call(
        find(&tools, "plugin__noisy__noisy"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(
        noisy.ok,
        "the over-long log line must not fail the call: {}",
        noisy.content
    );
    assert_eq!(noisy.content, "logged 204800 bytes");

    // The plugin must still be alive and answering: the drain continued.
    assert_eq!(
        host.diagnostics()[0].status,
        PluginStatus::Ready,
        "{:?}",
        host.diagnostics()[0].error
    );
    let out = call(
        find(&tools, "plugin__noisy__echo"),
        &dir,
        serde_json::json!({"text": "still here"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "echo: still here");
    // The same process, not a restart after a crash.
    assert_eq!(host.diagnostics()[0].restart_count, 0);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Inline plugin that starts a long-lived helper in its `start` tool: a plain
/// child process that writes a marker file when it is still alive much later.
const HELPER_SPAWNING_PLUGIN: &str = r#"
import json, os, subprocess, sys

MANIFEST = {"name": "helper", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "permissions": [], "hooks": []}
TOOLS = [{"name": "start", "description": "Start a long-lived helper.",
          "parameters": {"type": "object", "properties": {}}, "class": "read_only"}]
MARKER = os.environ["HELPER_MARKER"]

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
        # A grandchild: it ignores SIGTERM and writes its marker only if it is
        # still running well after the plugin has been torn down.
        subprocess.Popen(["sh", "-c",
                          "trap '' TERM; sleep 6; echo alive > '%s'" % MARKER])
        send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True, "content": "started"}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

/// The bug FIX 3 is about: teardown signalled only the process the host spawned,
/// so a plugin's own children were orphaned and kept running after `shutdown`.
/// `docs/PLUGINS.md` promises "the host kills the process".
#[tokio::test]
async fn a_plugins_grandchildren_are_killed_with_it() {
    let dir = temp_dir("grandchildren");
    let script = dir.join("helper_plugin.py");
    std::fs::write(&script, HELPER_SPAWNING_PLUGIN).unwrap();
    let marker = dir.join("marker.txt");
    let mut env = BTreeMap::new();
    env.insert(
        "HELPER_MARKER".to_string(),
        marker.to_string_lossy().into_owned(),
    );

    let mut host = PluginHost::discover(
        &[PluginConfig {
            name: "helper".into(),
            enabled: true,
            command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
            env,
            permissions: Vec::new(),
            settings: BTreeMap::new(),
        }],
        &dir,
    );
    host.start_all().await;
    let tools = host.tools();
    let out = call(
        find(&tools, "plugin__helper__start"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(out.ok, "{}", out.content);

    // Shutdown must be prompt even though the grandchild ignores SIGTERM.
    let started = std::time::Instant::now();
    host.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "shutdown took {:?}",
        started.elapsed()
    );

    // The grandchild would write its marker 6 s after it was spawned. Give it
    // comfortably longer than that: if teardown only killed the leader, the file
    // appears.
    tokio::time::sleep(Duration::from_millis(7_000)).await;
    assert!(
        !marker.exists(),
        "the plugin's grandchild survived shutdown and wrote {}",
        marker.display()
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The same guarantee on the reload path: a replaced plugin must not leave its
/// helpers behind, or every reload accumulates orphans.
#[tokio::test]
async fn a_reload_does_not_orphan_the_old_processes_children() {
    let dir = temp_dir("grandchildren-reload");
    let script = dir.join("helper_plugin.py");
    std::fs::write(&script, HELPER_SPAWNING_PLUGIN).unwrap();
    let marker = dir.join("marker.txt");
    let mut env = BTreeMap::new();
    env.insert(
        "HELPER_MARKER".to_string(),
        marker.to_string_lossy().into_owned(),
    );
    let config = PluginConfig {
        name: "helper".into(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env,
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    let mut host = PluginHost::discover(std::slice::from_ref(&config), &dir);
    host.start_all().await;
    let out = call(
        find(&host.tools(), "plugin__helper__start"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(out.ok, "{}", out.content);

    // Change the config so the rescan replaces the process.
    let mut changed = config.clone();
    changed.env.insert("UNUSED".into(), "1".into());
    let report = host.rescan(&[changed]).await;
    assert_eq!(report.restarted, vec!["helper".to_string()]);

    tokio::time::sleep(Duration::from_millis(7_000)).await;
    assert!(
        !marker.exists(),
        "reloading orphaned the previous process's child: {}",
        marker.display()
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// The bug FIX 4 is about: `listen_notifications` stored the sender, but every
/// rebuild of a slot (`rescan`, `reload`, hot load) constructed a brand-new
/// `PluginShared` with no listener, so plugin notifications silently stopped
/// after the first reload.
#[tokio::test]
async fn notifications_still_arrive_after_a_reload_or_rescan() {
    let dir = temp_dir("notify-reload");
    let cfg = plugin("fake", "fake");
    let mut host = PluginHost::discover(std::slice::from_ref(&cfg), &dir);
    let mut notices = host.listen_notifications();
    host.start_all().await;

    // Before any reload: a notice arrives (the baseline the bug broke).
    let tools = host.tools();
    let outcome = call(
        find(&tools, "plugin__fake__notify_good"),
        &dir,
        serde_json::json!({"kind": "info", "title": "before reload"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    let notice = tokio::time::timeout(Duration::from_secs(5), notices.recv())
        .await
        .expect("a notice before the reload")
        .expect("channel open");
    assert_eq!(notice.title, "before reload");

    // After an explicit reload: the rebuilt slot must still be wired up.
    host.reload("fake", std::slice::from_ref(&cfg))
        .await
        .unwrap();
    let tools = host.tools();
    let outcome = call(
        find(&tools, "plugin__fake__notify_good"),
        &dir,
        serde_json::json!({"kind": "info", "title": "after reload"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    let notice = tokio::time::timeout(Duration::from_secs(5), notices.recv())
        .await
        .expect("a notice after `reload`")
        .expect("channel open");
    assert_eq!(notice.title, "after reload");

    // After a rescan that restarts the process: same guarantee.
    let mut changed = cfg.clone();
    changed.env.insert("FAKE_PLUGIN_EXTRA".into(), "1".into());
    let report = host.rescan(&[changed.clone()]).await;
    assert_eq!(report.restarted, vec!["fake".to_string()]);
    let tools = host.tools();
    let outcome = call(
        find(&tools, "plugin__fake__notify_good"),
        &dir,
        serde_json::json!({"kind": "info", "title": "after rescan"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    let notice = tokio::time::timeout(Duration::from_secs(5), notices.recv())
        .await
        .expect("a notice after `rescan`")
        .expect("channel open");
    assert_eq!(notice.title, "after rescan");
    assert_eq!(notice.plugin, "fake");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// A plugin installed *after* the server started listening (the hot-load path)
/// must notify too: its slot is built by `rescan`, long after
/// `listen_notifications` ran.
#[tokio::test]
async fn a_hot_loaded_plugin_can_raise_notifications() {
    let dir = temp_dir("notify-hotload");
    let mut host = PluginHost::discover(&[], &dir);
    let mut notices = host.listen_notifications();
    host.start_all().await;

    // Install the fixture as a disk manifest, the way an operator would.
    let plugin_dir = dir.join("plugins").join("fresh");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        format!(
            "name = \"fresh\"\ncommand = [\"sh\", \"{FIXTURE}\"]\nenv = {{ FAKE_PLUGIN_NAME = \"fresh\" }}\n"
        ),
    )
    .unwrap();
    let report = host.rescan(&[]).await;
    assert_eq!(report.added, vec!["fresh".to_string()]);

    let tools = host.tools();
    let outcome = call(
        find(&tools, "plugin__fresh__notify_good"),
        &dir,
        serde_json::json!({"kind": "info", "title": "hot loaded"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    let notice = tokio::time::timeout(Duration::from_secs(5), notices.recv())
        .await
        .expect("a notice from a hot-loaded plugin")
        .expect("channel open");
    assert_eq!(notice.plugin, "fresh");
    assert_eq!(notice.title, "hot loaded");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// `stop_notifications` still works after a rebuild: a reload must not resurrect
/// a listener the server has switched off.
#[tokio::test]
async fn stop_notifications_survives_a_reload() {
    let dir = temp_dir("notify-stop-reload");
    let cfg = plugin("fake", "fake");
    let mut host = PluginHost::discover(std::slice::from_ref(&cfg), &dir);
    let mut notices = host.listen_notifications();
    host.start_all().await;
    host.stop_notifications();
    host.reload("fake", std::slice::from_ref(&cfg))
        .await
        .unwrap();

    let tools = host.tools();
    let outcome = call(
        find(&tools, "plugin__fake__notify_good"),
        &dir,
        serde_json::json!({"kind": "info", "title": "nobody listening"}),
    )
    .await;
    assert!(outcome.ok, "{outcome:?}");
    // `Ok(None)` means every sender is gone (a stopped listener), `Err` means
    // the timeout fired: both are "no notice arrived". Only a notice is a failure.
    match tokio::time::timeout(Duration::from_millis(200), notices.recv()).await {
        Err(_) | Ok(None) => {}
        Ok(Some(notice)) => panic!("a reload re-enabled a stopped listener: {notice:?}"),
    }

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// Inline plugin whose manifest name and single tool name are both configurable,
/// so two of them can be made to produce the same namespaced tool name.
const COLLIDING_PLUGIN: &str = r#"
import json, os, sys

NAME = os.environ["COLLIDE_PLUGIN_NAME"]
TOOL = os.environ["COLLIDE_TOOL_NAME"]
PANEL = os.environ.get("COLLIDE_PANEL_NAME")

MANIFEST = {"name": NAME, "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"] + (["panels"] if PANEL else []),
            "permissions": [], "hooks": [],
            "panels": ([{"id": PANEL, "title": NAME, "source": "static"}] if PANEL else [])}
TOOLS = [{"name": TOOL, "description": "Answers with its own plugin and tool name.",
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
              "result": {"ok": True, "content": "%s/%s" % (NAME, TOOL)}})
    elif method == "panels/read":
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"title": NAME, "data": {"owner": NAME}}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

fn colliding_plugin(
    dir: &std::path::Path,
    slot: &str,
    plugin_name: &str,
    tool_name: &str,
) -> PluginConfig {
    let script = dir.join("colliding_plugin.py");
    if !script.exists() {
        std::fs::write(&script, COLLIDING_PLUGIN).unwrap();
    }
    let mut env = BTreeMap::new();
    env.insert("COLLIDE_PLUGIN_NAME".to_string(), plugin_name.to_string());
    env.insert("COLLIDE_TOOL_NAME".to_string(), tool_name.to_string());
    PluginConfig {
        name: slot.to_string(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env,
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    }
}

fn colliding_panel_plugin(dir: &std::path::Path, name: &str, panel: &str) -> PluginConfig {
    let mut config = colliding_plugin(dir, name, name, "unused");
    config.env.insert("COLLIDE_PANEL_NAME".into(), panel.into());
    config
}

/// The bug FIX 5 is about: the `plugin__` prefix only prevents collisions with
/// first-party tools. A plugin `a` exposing `b__c` and a plugin `a__b` exposing
/// `c` both produce `plugin__a__b__c`, and the registry is a `HashMap`, so one of
/// them used to be silently dropped.
#[tokio::test]
async fn a_namespaced_tool_name_collision_is_refused_loudly() {
    let dir = temp_dir("collision");
    // Both yield `plugin__a__b__c`.
    let first = colliding_plugin(&dir, "a", "a", "b__c");
    let second = colliding_plugin(&dir, "a__b", "a__b", "c");
    let mut host = PluginHost::discover(&[first, second], &dir);
    host.start_all().await;

    let tools = host.tools();
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    // The name exists exactly once — no duplicate, no silent replacement.
    assert_eq!(
        names.iter().filter(|n| *n == "plugin__a__b__c").count(),
        1,
        "{names:?}"
    );
    // Discovery order decides: the first plugin keeps the name.
    let only = find(&tools, "plugin__a__b__c");
    assert_eq!(only.spec().origin, ToolOrigin::Plugin { name: "a".into() });
    let out = call(only, &dir, serde_json::json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.content, "a/b__c");

    // The refusal is a diagnostic, not a silent drop: the losing plugin says so
    // in its own status, naming both the tool and the winner.
    let loser = host.diagnostics_for("a__b").unwrap();
    assert_eq!(
        loser.status,
        PluginStatus::Ready,
        "the plugin itself is fine; only the name was refused"
    );
    let note = loser.error.clone().unwrap_or_default();
    assert!(note.contains("not exposing"), "{note}");
    assert!(note.contains("plugin__a__b__c"), "{note}");
    assert!(note.contains("plugin `a`"), "{note}");
    // The winner is not blamed.
    assert!(
        host.diagnostics_for("a")
            .unwrap()
            .error
            .unwrap_or_default()
            .is_empty(),
        "the plugin that kept the name must not be flagged"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn hosted_panel_delimiter_collisions_are_refused_with_diagnostics() {
    let dir = temp_dir("panel-collision");
    let first = colliding_panel_plugin(&dir, "a", "b__c");
    let second = colliding_panel_plugin(&dir, "a__b", "c");
    let mut host = PluginHost::discover(&[first, second], &dir);
    host.start_all().await;

    let groups = host.panels_handle().panels();
    let summaries: Vec<_> = groups
        .iter()
        .flat_map(|group| group.panels.iter())
        .collect();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert_eq!(summaries[0].hosted_id, "plugin__a__b__c");
    assert_eq!(groups[0].plugin, "a");

    let loser = host.diagnostics_for("a__b").unwrap();
    assert_eq!(loser.status, PluginStatus::Ready);
    let note = loser.error.unwrap_or_default();
    assert!(
        note.contains("not exposing panel `plugin__a__b__c`"),
        "{note}"
    );
    assert!(note.contains("plugin `a`"), "{note}");
    assert!(note.contains("Rename the plugin or the panel"), "{note}");
    assert!(host.diagnostics_for("a").unwrap().error.is_none());

    let context = ToolCallContext::default();
    let winner = host.panel("a", "b__c", &context).await.unwrap();
    assert_eq!(winner.data["owner"], "a");
    let refused = host.panel("a__b", "c", &context).await.unwrap_err();
    assert!(refused.to_string().contains("was refused"), "{refused}");
    assert!(
        refused.to_string().contains("The panel was not called"),
        "{refused}"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// A collision must not be able to hide a tool: the refused name belongs to the
/// first plugin, and the *other* tools of the losing plugin still work.
#[tokio::test]
async fn a_refused_tool_does_not_take_the_rest_of_the_plugin_with_it() {
    let dir = temp_dir("collision-partial");
    // `a` provides `b__c`; `a__b` would produce the same name for its `c`, and
    // also has a tool of its own that does not collide.
    let mut second = colliding_plugin(&dir, "a__b", "a__b", "c");
    // A second tool on the same plugin, using a fixture that ignores the knob.
    let script = dir.join("two_tools.py");
    std::fs::write(
        &script,
        r#"
import json, sys
MANIFEST = {"name": "a__b", "version": "0.1.0", "protocol": "1",
            "capabilities": ["tools"], "permissions": [], "hooks": []}
TOOLS = [
    {"name": "c", "description": "Collides.", "parameters": {"type": "object", "properties": {}}, "class": "read_only"},
    {"name": "unique", "description": "Does not collide.", "parameters": {"type": "object", "properties": {}}, "class": "read_only"},
]
def send(m):
    sys.stdout.write(json.dumps(m) + "\n"); sys.stdout.flush()
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
        send({"jsonrpc": "2.0", "id": rid, "result": {"ok": True,
             "content": (req.get("params") or {}).get("name", "")}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None}); break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#,
    )
    .unwrap();
    second.command = vec!["python3".to_string(), script.to_string_lossy().into_owned()];

    let first = colliding_plugin(&dir, "a", "a", "b__c");
    let mut host = PluginHost::discover(&[first, second], &dir);
    host.start_all().await;

    let names: Vec<String> = host.tools().iter().map(|t| t.spec().name).collect();
    assert!(
        names.contains(&"plugin__a__b__unique".to_string()),
        "{names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| *n == "plugin__a__b__c").count(),
        1,
        "{names:?}"
    );

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

/// An idle plugin closes only stdout after one successful arming call, then stays
/// blocked on stdin. The host must not wait for process exit to notice.
const IDLE_STDOUT_EOF_PLUGIN: &str = r#"
import json, os, sys, threading, time


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def close_stdout_soon():
    time.sleep(0.05)
    # Close the descriptor itself. TextIOWrapper shutdown from a secondary
    # Python thread can otherwise leave the pipe reader waiting.
    os.close(1)


for line in sys.stdin:
    req = json.loads(line)
    method, rid = req.get("method"), req.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "name": "idle-eof", "version": "1", "protocol": "1",
            "capabilities": ["tools"]}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": [
            {"name": "arm_eof", "description": "Close stdout while idle.",
             "parameters": {"type": "object"}},
            {"name": "echo", "description": "Still works after restart.",
             "parameters": {"type": "object"}}]}})
    elif method == "tools/call":
        name = req.get("params", {}).get("name")
        if name == "arm_eof":
            threading.Thread(target=close_stdout_soon, daemon=True).start()
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"ok": True, "content": "armed" if name == "arm_eof" else "restarted"}})
"#;

#[tokio::test]
async fn unexpected_idle_stdout_eof_fails_reaps_and_restarts_the_plugin() {
    let dir = temp_dir("idle-stdout-eof");
    let script = dir.join("idle_eof.py");
    std::fs::write(&script, IDLE_STDOUT_EOF_PLUGIN).unwrap();
    let config = PluginConfig {
        name: "idle-eof".into(),
        enabled: true,
        command: vec!["python3".into(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    let mut host = PluginHost::discover(&[config], &dir);
    host.start_all().await;
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);
    let tools = host.tools();
    let echo = find(&tools, "plugin__idle-eof__echo").clone();
    let first_pid = host.diagnostics()[0].pid;
    let armed = call(
        find(&tools, "plugin__idle-eof__arm_eof"),
        &dir,
        serde_json::json!({}),
    )
    .await;
    assert!(armed.ok, "{}", armed.content);

    // No request is pending when EOF arrives. Readiness must be revoked
    // immediately, ahead of the supervisor's existing graceful-kill budget.
    tokio::time::timeout(Duration::from_secs(1), async {
        while host.diagnostics()[0].status != PluginStatus::Failed {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("idle EOF must fail promptly: {:?}", host.diagnostics()[0]));
    let diag = &host.diagnostics()[0];
    assert!(diag.error.as_deref().unwrap_or_default().contains("stdout"));
    assert!(host.tools().is_empty());
    // Waiting for pid=None proves the supervisor actually killed/reaped the
    // process that merely closed a pipe rather than exiting on its own.
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.diagnostics()[0].pid.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("idle EOF was not reaped: {:?}", host.diagnostics()[0]));

    // The tool object predates the crash, exactly like a registry consumer's
    // long-lived handle. It must transparently take the normal restart path.
    let out = call(&echo, &dir, serde_json::json!({})).await;
    assert!(
        out.ok,
        "the old handle did not restart the plugin: {}",
        out.content
    );
    assert_eq!(out.content, "restarted");
    let diag = &host.diagnostics()[0];
    assert_eq!(diag.status, PluginStatus::Ready, "{:?}", diag.error);
    assert_eq!(diag.restart_count, 1);
    assert_ne!(diag.pid, first_pid);

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// The turn in the `before_prompt` payload
// ---------------------------------------------------------------------------

/// Inline plugin that reports what the `before_prompt` payload carried.
///
/// It answers `turn=<first 60 chars>|len=<chars>` when the field is present and
/// `turn=<absent>|len=0` when it is not, so a test can assert both the text and
/// the cap without depending on the fixture's wording.
const TURN_ECHO_PLUGIN: &str = r#"
import json, sys

NAME = "turnecho"
MANIFEST = {"name": NAME, "version": "0.1.0", "protocol": "1",
            "capabilities": ["hooks"], "permissions": [], "hooks": ["before_prompt"]}

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
        hook = req.get("params", {}).get("hook", "")
        payload = req.get("params", {}).get("payload") or {}
        if hook == "before_prompt":
            turn = payload.get("turn")
            if turn is None:
                out = "turn=<absent>|tail=|len=0"
            else:
                out = "turn=%s|tail=%s|len=%d" % (
                    turn[:60].replace("\n", " "),
                    turn[-24:].replace("\n", " "),
                    len(turn),
                )
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": out}})
        else:
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": None}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": "unknown"}})
"#;

/// Register [`TURN_ECHO_PLUGIN`] under `name` and start it.
async fn turn_echo_host(dir: &std::path::Path, name: &str) -> PluginHost {
    let script = dir.join(format!("{name}.py"));
    std::fs::write(&script, TURN_ECHO_PLUGIN).unwrap();
    let cfg = PluginConfig {
        name: name.to_string(),
        enabled: true,
        command: vec!["python3".to_string(), script.to_string_lossy().into_owned()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    };
    let mut host = PluginHost::discover(&[cfg], dir);
    host.start_all().await;
    host
}

#[tokio::test]
async fn a_plugin_that_ignores_the_turn_behaves_exactly_as_before() {
    // Two fixture plugins, whose `before_prompt` reads only the session id. Adding
    // a `turn` field must change neither their output nor its order.
    let dir = temp_dir("turn-ignored");
    let mut host = PluginHost::discover(
        &[plugin("fake-a", "fake-a"), plugin("fake-b", "fake-b")],
        &dir,
    );
    host.start_all().await;

    let without = host.before_prompt("s_1", "/w").await;
    let with_turn = host
        .before_prompt_for_turn("s_1", "/w", Some("what is this repository?"))
        .await;
    assert_eq!(without.len(), 2, "{without:?}");
    assert_eq!(
        without, with_turn,
        "a plugin that ignores the turn must produce identical notes"
    );
    assert!(with_turn[0].contains("fake-a"), "{with_turn:?}");
    assert!(with_turn[1].contains("fake-b"), "{with_turn:?}");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_turn_reaches_the_plugin_payload() {
    let dir = temp_dir("turn-payload");
    let mut host = turn_echo_host(&dir, "turnecho").await;

    let notes = host
        .before_prompt_for_turn("s_1", "/w", Some("where is the journal written?"))
        .await;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("turn=where is the journal written?"),
        "the plugin did not see the turn: {}",
        notes[0]
    );
    assert_eq!(
        notes[0].rsplit("len=").next(),
        Some("29"),
        "the plugin must see every byte of a short turn: {}",
        notes[0]
    );
    // Stable across calls: the text goes into the system prompt, so the same turn
    // must produce the same bytes (that is what keeps the prefix cacheable).
    for _ in 0..3 {
        assert_eq!(
            host.before_prompt_for_turn("s_1", "/w", Some("where is the journal written?"))
                .await,
            notes
        );
    }

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_huge_turn_is_bounded_and_an_absent_turn_still_answers() {
    let dir = temp_dir("turn-huge");
    let mut host = turn_echo_host(&dir, "turnecho").await;

    // 200 KiB of "the user pasted a log": the hook must still answer, and what it
    // sees must be bounded, because it runs on every model call of the run.
    let huge = "x".repeat(200 * 1024);
    let notes = host.before_prompt_for_turn("s_1", "/w", Some(&huge)).await;
    assert_eq!(
        notes.len(),
        1,
        "a big turn must not break the hook: {notes:?}"
    );
    let len: usize = notes[0]
        .rsplit("len=")
        .next()
        .and_then(|n| n.parse().ok())
        .expect("the plugin reports the length it saw");
    assert!(
        (4096..=4096 + 32).contains(&len),
        "the forwarded turn must be capped at 4 KiB, got {len} bytes"
    );
    assert!(
        notes[0].contains("truncated"),
        "the cut must be visible to the plugin, not silent: {}",
        notes[0]
    );

    // No turn at all (a continuation): the field is absent, the hook still runs.
    let notes = host.before_prompt_for_turn("s_1", "/w", None).await;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].contains("turn=<absent>"),
        "an absent turn must not be invented: {}",
        notes[0]
    );

    // The same through the cheap live handle the server actually holds.
    let hooks: Arc<dyn Hooks> = Arc::new(host.hooks_handle());
    let via_handle = hooks.before_prompt_for_turn("s_1", "/w", None).await;
    assert_eq!(via_handle.len(), 1, "{via_handle:?}");
    assert!(via_handle[0].contains("turn=<absent>"), "{via_handle:?}");

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn the_turn_does_not_reorder_two_plugins_contributions() {
    // Order comes from discovery, never from who answered first - the notes are
    // joined into message 0, so a reorder would break the provider's prefix cache
    // on every turn.
    let dir = temp_dir("turn-order");
    // Two separate processes answering independently: `fan_out` joins them by
    // discovery index, so the notes must never swap places however the two
    // processes are scheduled.
    let configs = vec![plugin("fake-a", "fake-a"), plugin("fake-b", "fake-b")];
    let mut host = PluginHost::discover(&configs, &dir);
    host.start_all().await;

    let mut first: Option<Vec<String>> = None;
    for _ in 0..8 {
        let notes = host
            .before_prompt_for_turn("s_1", "/w", Some("same question every time"))
            .await;
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("fake-a"), "{notes:?}");
        assert!(notes[1].contains("fake-b"), "{notes:?}");
        match &first {
            Some(previous) => assert_eq!(previous, &notes, "plugin order moved"),
            None => first = Some(notes),
        }
    }

    host.shutdown().await;
    std::fs::remove_dir_all(&dir).ok();
}
