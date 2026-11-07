//! Production `AppState` plugin-hook wiring, against a real out-of-process plugin.

mod support;

use kc_core::config::PluginConfig;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use support::{text, HarnessBuilder};

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
