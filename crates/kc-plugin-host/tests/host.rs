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
use kc_core::plugin::PluginStatus;
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
    assert_eq!(manifest.hooks, vec!["before_prompt".to_string()]);
    assert!(diags[0].pid.is_some());
    assert_eq!(diags[0].tools.len(), 7);

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
    assert_eq!(echo.class(), ToolClass::ReadOnly);
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
    assert_eq!(host.tools().len(), 6);

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
    assert!(notes.iter().any(|n| n.contains("fake-a")));
    assert!(notes.iter().any(|n| n.contains("fake-b")));

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
    assert_eq!(host.tools().len(), 6);
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Ready);

    let diag = host.set_enabled("fake", false).await.unwrap();
    assert_eq!(diag.status, PluginStatus::Disabled);
    assert!(diag.pid.is_none());
    assert!(diag.tools.is_empty());
    assert!(host.tools().is_empty());

    let diag = host.set_enabled("fake", true).await.unwrap();
    assert_eq!(diag.status, PluginStatus::Ready);
    assert!(diag.pid.is_some());
    assert_eq!(host.tools().len(), 6);
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

    // Granting the permission in the config exposes it, with the shell class.
    let mut granted = plugin("fake", "fake");
    granted.permissions.push("shell".into());
    let mut host = PluginHost::discover(&[granted], &dir);
    host.start_all().await;
    let tools = host.tools();
    let deploy = find(&tools, "plugin__fake__deploy");
    assert_eq!(deploy.class(), ToolClass::Shell);
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
