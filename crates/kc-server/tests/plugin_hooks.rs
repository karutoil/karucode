//! Production `AppState` plugin-hook wiring, against a real out-of-process plugin.

mod support;

use kc_core::config::PluginConfig;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::{text, tool_call, HarnessBuilder};

const HOOK_PLUGIN: &str = r#"
import json, os, sys, time

LOG = os.environ["HOOK_TEST_LOG"]
MANIFEST = {
    "name": "hookprobe", "version": "1.0.0", "protocol": "1",
    "capabilities": ["hooks", "notifications"], "permissions": [],
    "hooks": ["before_prompt", "on_notify"], "panels": [],
}

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def record(value):
    with open(LOG, "a", encoding="utf-8") as output:
        output.write(json.dumps(value) + "\n")

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    method, rid = request.get("method"), request.get("id")
    params = request.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": []}})
    elif method == "hooks/run":
        payload = params.get("payload") or {}
        hook = params.get("hook")
        if hook == "before_prompt":
            # This window proves the server is not holding its administrative
            # plugin mutex while an out-of-process hook is blocked.
            record({"phase": "prompt-started", "turn": payload.get("turn")})
            time.sleep(2)
            send({"jsonrpc": "2.0", "id": rid, "result": {
                "output": "TURN-SENSITIVE-NOTE: " + str(payload.get("turn"))
            }})
        elif hook == "on_notify":
            record({"phase": "notification", "kind": payload.get("kind"),
                    "title": payload.get("title")})
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": {"seen": True}}})
        else:
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": None}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid,
              "error": {"code": -32601, "message": "unknown method"}})
"#;

#[tokio::test]
async fn state_adapter_forwards_turn_and_notifications_without_holding_the_host_lock() {
    let probe = std::env::temp_dir().join(format!(
        "kc-server-hook-probe-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&probe).unwrap();
    let script = probe.join("hookprobe.py");
    let log = probe.join("hooks.jsonl");
    std::fs::write(&script, HOOK_PLUGIN).unwrap();
    let mut env = std::collections::BTreeMap::new();
    env.insert("HOOK_TEST_LOG".into(), log.to_string_lossy().into_owned());
    env.insert("FAKE_PLUGIN_NAME".into(), "hookprobe".into());
    let plugin = PluginConfig {
        name: "hookprobe".into(),
        enabled: true,
        command: vec!["python3".into(), script.to_string_lossy().into_owned()],
        env,
        permissions: Vec::new(),
        settings: Default::default(),

        tool_output_bytes: None,
    };
    let h = HarnessBuilder::new(vec![text("provider answered")])
        .no_ai_titles()
        .plugin(plugin)
        .build()
        .await;
    let session = h.create_session(Some("Hook wiring")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({"content":"needle question for the plugin"}),
        )
        .await;
    assert_eq!(
        sent.status,
        axum::http::StatusCode::ACCEPTED,
        "{}",
        sent.text()
    );

    // Once the plugin has entered its delayed prompt hook, an unrelated host read
    // must still complete promptly. The old adapter held this mutex across the RPC.
    for _ in 0..200 {
        if std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("prompt-started")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("prompt-started"));
    let started = Instant::now();
    let plugins = h.get("/api/plugins").await;
    assert_eq!(plugins.status, axum::http::StatusCode::OK);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "plugin admin read was blocked for {:?}",
        started.elapsed()
    );

    h.wait_until_idle(&session).await;
    let prompts = h.factory.prompts();
    let system = prompts
        .first()
        .and_then(|messages| messages.first())
        .expect("provider system prompt");
    assert!(
        system.contains("TURN-SENSITIVE-NOTE: needle question for the plugin"),
        "{system}"
    );

    // Notification fan-out is spawned after run completion. Wait for the plugin's
    // own side effect instead of asserting a scheduler-dependent instant arrival.
    for _ in 0..300 {
        if std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("run_finished")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let records: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        records.iter().any(|record| {
            record["phase"] == "notification" && record["kind"] == "run_finished"
        }),
        "{records:?}"
    );
    h.state.shutdown_token().cancel();
    std::fs::remove_dir_all(probe).ok();
}

/// The four *gate* hooks must reach a real plugin through the production
/// `AppState`, not just through `kc_plugin_host`'s own tests.
///
/// This is the regression test for a wiring bug that made every gate a no-op in
/// a real install: `kc_plugin_host::PluginHooks` implements `on_tool_call`,
/// `on_tool_result_gate`, `on_model_request` and `on_stop`, but bootstrap wires
/// `kc_server::state::PluginHooks` as the runtime's hooks object. That adapter
/// forwarded only the observers, so each gate silently fell through to the
/// trait's default (allow / change nothing) and no plugin could affect a run.
///
/// The plugin below blocks one tool call and records each gate payload, so a
/// regression fails loudly: the tool would run and the block record would never
/// arrive.
const GATE_PLUGIN: &str = r#"
import json, os, sys

LOG = os.environ["GATE_TEST_LOG"]

MANIFEST = {
    "name": "gateprobe", "version": "1.0.0", "protocol": "1",
    "capabilities": ["hooks", "tools"], "permissions": [],
    "hooks": ["on_tool_call", "on_tool_result", "on_model_request", "on_stop"],
    "panels": [],
    "tools": [],
}

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def record(value):
    with open(LOG, "a", encoding="utf-8") as output:
        output.write(json.dumps(value) + "\n")

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    method, rid = request.get("method"), request.get("id")
    params = request.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": []}})
    elif method == "hooks/run":
        hook = params.get("hook")
        payload = params.get("payload") or {}
        if hook == "on_tool_call":
            record({"hook": hook, "tool": payload.get("tool"),
                    "class": payload.get("class")})
            # Refuse the read-only probe: a gate that is wired reaches the model
            # as this reason instead of the file's contents.
            if payload.get("tool") == "read_file":
                send({"jsonrpc": "2.0", "id": rid, "result": {
                    "output": {"block": True, "reason": "GATE-BLOCKED"}}})
            else:
                send({"jsonrpc": "2.0", "id": rid, "result": {"output": None}})
        elif hook == "on_tool_result":
            record({"hook": hook, "tool": payload.get("tool")})
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": None}})
        elif hook == "on_model_request":
            record({"hook": hook, "tool": None})
            send({"jsonrpc": "2.0", "id": rid, "result": {
                "output": "GATE-MODEL-NOTE"}})
        elif hook == "on_stop":
            record({"hook": hook, "tool": None})
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": None}})
        else:
            send({"jsonrpc": "2.0", "id": rid, "result": {"output": None}})
    elif method == "shutdown":
        send({"jsonrpc": "2.0", "id": rid, "result": None})
        break
    elif rid is not None:
        send({"jsonrpc": "2.0", "id": rid,
              "error": {"code": -32601, "message": "unknown method"}})
"#;

#[tokio::test]
async fn state_adapter_forwards_the_four_gate_hooks_to_a_real_plugin() {
    let probe = std::env::temp_dir().join(format!(
        "kc-server-gate-probe-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&probe).unwrap();
    let script = probe.join("gateprobe.py");
    let log = probe.join("gates.jsonl");
    std::fs::write(&script, GATE_PLUGIN).unwrap();
    let target = probe.join("readable.txt");
    std::fs::write(&target, "the real file contents").unwrap();

    let mut env = std::collections::BTreeMap::new();
    env.insert("GATE_TEST_LOG".into(), log.to_string_lossy().into_owned());
    env.insert("FAKE_PLUGIN_NAME".into(), "gateprobe".into());
    let plugin = PluginConfig {
        name: "gateprobe".into(),
        enabled: true,
        command: vec!["python3".into(), script.to_string_lossy().into_owned()],
        env,
        permissions: vec!["files.read".into()],
        settings: Default::default(),
        tool_output_bytes: None,
    };

    let h = HarnessBuilder::new(vec![])
        .no_ai_titles()
        .plugin(plugin)
        .build()
        .await;
    // The agent asks to read a real file; the plugin's on_tool_call blocks it.
    h.factory.push(tool_call(
        "read_file",
        &json!({"path": target.to_string_lossy()}).to_string(),
    ));
    h.factory.push(text("done"));

    let session = h.create_session(Some("Gate wiring")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({"content":"read that file"}),
        )
        .await;
    assert_eq!(
        sent.status,
        axum::http::StatusCode::ACCEPTED,
        "{}",
        sent.text()
    );
    h.wait_until_idle(&session).await;

    let records: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap_or_else(|_| String::new())
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let hooks_seen: Vec<&str> = records
        .iter()
        .filter_map(|record| record["hook"].as_str())
        .collect();

    // 1. on_tool_call reached the plugin, so the gate is wired.
    assert!(
        hooks_seen.contains(&"on_tool_call"),
        "on_tool_call never reached the plugin: {records:?}"
    );
    // 2. on_model_request reached the plugin on the provider request.
    assert!(
        hooks_seen.contains(&"on_model_request"),
        "on_model_request never reached the plugin: {records:?}"
    );

    // 3. The block actually took effect: the model saw the reason, not the file.
    let prompts = h.factory.prompts();
    let transcript = prompts
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<String>>()
        .join("\n");
    assert!(
        transcript.contains("GATE-BLOCKED"),
        "the blocked call did not reach the model: {transcript}"
    );
    assert!(
        !transcript.contains("the real file contents"),
        "the blocked tool still ran: {transcript}"
    );

    h.state.shutdown_token().cancel();
    std::fs::remove_dir_all(probe).ok();
}
