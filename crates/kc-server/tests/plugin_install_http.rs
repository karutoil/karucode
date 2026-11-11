//! HTTP routes for `karucode plugin install`: the web UI's half. Preview, the
//! pollable job, auto-rescan on success, and the admin gate.
//!
//! The repository on disk is the same layout the CLI tests use (an
//! `index.toml` collection); the routes are exercised through the real router
//! with the real plugin host, so "the job's rescan made the plugin appear" is
//! an end-to-end assertion, not a mock.

mod support;

use axum::http::Method;
use serde_json::{json, Value};
use support::{Harness, HarnessBuilder, Script};

use std::path::{Path, PathBuf};
use std::time::Duration;

fn tmp_tag(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-plugin-install-route-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A plugin collection: an index.toml naming one built plugin and one script
/// plugin. The Rust one is a real cargo project so the job does real work.
fn write_collection(root: &Path) {
    std::fs::write(
        root.join("index.toml"),
        "[[plugins]]\nname = \"wordy\"\npath = \"wordy\"\ndescription = \"Counts words\"\n\n\
         [[plugins]]\nname = \"pytools\"\npath = \"pytools\"\ndescription = \"Python demo tools\"\n",
    )
    .unwrap();
    let wordy = root.join("wordy");
    std::fs::create_dir_all(wordy.join("src")).unwrap();
    std::fs::write(
        wordy.join("Cargo.toml"),
        "[package]\nname = \"wordy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n",
    )
    .unwrap();
    // A minimal but real SDK-free plugin: protocol v1 over stdin/stdout.
    std::fs::write(
        wordy.join("src").join("main.rs"),
        r#"fn main() {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let id = value.get("id").cloned().unwrap_or_default();
        let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let result = match method {
            "initialize" => serde_json::json!({
                "name": "wordy",
                "protocol": "1",
                "version": "0.1.0",
                "capabilities": ["tools"],
                "tools": [],
                "hooks": [],
                "panels": [],
                "ui": [],
            }),
            "tools/list" => serde_json::json!([]),
            _ => serde_json::Value::Null,
        };
        let response = serde_json::json!({ "id": id, "jsonrpc": "2.0", "result": result });
        let _ = writeln!(out, "{response}");
        let _ = out.flush();
        if method == "shutdown" {
            break;
        }
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        wordy.join("plugin.toml"),
        "name = \"wordy\"\ncommand = [\"target/release/wordy\"]\npermissions = [\"files.read\"]\n\n\
         [install]\nbuild = [\"cargo\", \"build\", \"--release\"]\n",
    )
    .unwrap();

    let pytools = root.join("pytools");
    std::fs::create_dir_all(&pytools).unwrap();
    std::fs::write(
        pytools.join("plugin.toml"),
        "name = \"pytools\"\ncommand = [\"python3\", \"tool.py\"]\npermissions = [\"files.read\"]\n",
    )
    .unwrap();
    std::fs::write(pytools.join("tool.py"), "# demo python plugin\n").unwrap();
}

async fn harness() -> Harness {
    HarnessBuilder::new(vec![Script::Text(vec!["ok".into()])])
        .no_ai_titles()
        .build()
        .await
}

/// Poll a job until it is terminal or the timeout hits. Returns the last
/// snapshot.
async fn wait_terminal(harness: &Harness, id: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let response = harness
            .send(
                Method::GET,
                &format!("/api/plugins/install/{id}"),
                None,
                &[],
            )
            .await;
        assert_eq!(
            response.status,
            200,
            "poll failed: {}",
            response.error_message()
        );
        let last = response.json();
        let status = last["status"].as_str().unwrap_or_default().to_string();
        if status != "running" {
            return last;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "job never finished; last snapshot: {last}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_returns_the_offered_plugins_without_installing() {
    let harness = harness().await;
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = harness.root.join("data");

    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install/preview",
            Some(json!({
                "source": collection.to_string_lossy(),
            })),
            &[],
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.error_message());
    let preview = response.json();
    let names: Vec<&str> = preview["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["wordy", "pytools"]);
    assert_eq!(preview["kind"], "local");
    // The review surface: permissions, builds, already-installed.
    let wordy = &preview["plugins"][0];
    assert_eq!(wordy["permissions"], json!(["files.read"]));
    assert_eq!(wordy["builds"], true);
    assert_eq!(wordy["already_installed"], false);
    // A preview installs nothing.
    assert!(!data_dir.join("plugins").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_refuses_junk_sources_and_bad_bodies() {
    let harness = harness().await;

    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install/preview",
            Some(json!({ "source": "not-a-source" })),
            &[],
        )
        .await;
    assert_eq!(response.status, 400, "{}", response.error_message());

    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install/preview",
            Some(json!({ "nope": 1 })),
            &[],
        )
        .await;
    assert_eq!(response.status, 400, "unknown fields are rejected");

    let response = harness
        .send(Method::POST, "/api/plugins/install/preview", None, &[])
        .await;
    assert_eq!(response.status, 400, "a body is required");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_install_job_runs_builds_and_rescans_the_host() {
    let harness = harness().await;
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = harness.root.join("data");

    // Start the job.
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install",
            Some(json!({
                "source": collection.to_string_lossy(),
                "names": ["wordy"],
            })),
            &[],
        )
        .await;
    assert_eq!(response.status, 200, "{}", response.error_message());
    let started = response.json();
    let id = started["id"].as_str().expect("job id").to_string();

    // Terminal state, with the install recorded.
    let final_state = wait_terminal(&harness, &id).await;
    assert_eq!(final_state["status"], "completed", "{final_state}");
    let events = final_state["events"].as_array().unwrap();
    let event_kinds: Vec<&str> = events.iter().filter_map(|e| e["kind"].as_str()).collect();
    assert!(
        event_kinds.contains(&"building"),
        "the build ran; events: {events:?}"
    );
    assert!(
        event_kinds.contains(&"installed"),
        "the install landed; events: {events:?}"
    );
    let result = &final_state["result"];
    assert_eq!(result["installed"], json!(["wordy"]));

    // The auto-rescan: the plugin host discovered the new disk manifest and
    // the result carries the report.
    assert_eq!(result["rescan"]["added"], json!(["wordy"]));

    // The plugin is installed on disk where the host found it.
    let manifest = std::fs::read_to_string(data_dir.join("plugins/wordy/plugin.toml")).unwrap();
    assert!(manifest.contains("command = [\""));
    let command_path = manifest
        .lines()
        .find(|l| l.starts_with("command"))
        .unwrap()
        .trim_start_matches("command = [\"")
        .trim_end_matches("\"]");
    assert!(Path::new(command_path).is_file(), "the built binary exists");

    // The running host reports it: ready or failed, but *known*.
    let response = harness.send(Method::GET, "/api/plugins", None, &[]).await;
    let plugins = response.json()["plugins"].as_array().unwrap().clone();
    let wordy = plugins
        .iter()
        .find(|p| p["name"] == "wordy")
        .expect("wordy is registered");
    assert_eq!(wordy["status"], "ready", "the installed plugin handshakes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_job_reports_the_error_and_installs_nothing() {
    let harness = harness().await;
    let collection = tmp_tag("broken");
    // A plugin whose command points outside its directory: refused by the
    // relocation rules, so the job fails without writing to the data dir.
    let plugin = collection.join("wordy");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        "name = \"wordy\"\ncommand = [\"/usr/local/bin/not-a-real-plugin\"]\n",
    )
    .unwrap();

    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install",
            Some(json!({ "source": collection.to_string_lossy() })),
            &[],
        )
        .await;
    assert_eq!(response.status, 200);
    let id = response.json()["id"].as_str().unwrap().to_string();
    let final_state = wait_terminal(&harness, &id).await;
    assert_eq!(final_state["status"], "failed", "{final_state}");
    let error = final_state["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("not relocatable") || error.contains("could not install"),
        "the refusal names the problem: {error}"
    );
    assert!(final_state["result"].is_null(), "no result on failure");
    assert!(
        !harness.root.join("data/plugins/wordy").exists(),
        "a failed job installs no plugin"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_existing_install_is_refused_without_force_over_http() {
    let harness = harness().await;
    let collection = tmp_tag("collection");
    write_collection(&collection);

    // Install once.
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install",
            Some(json!({
                "source": collection.to_string_lossy(),
                "names": ["pytools"],
            })),
            &[],
        )
        .await;
    let id = response.json()["id"].as_str().unwrap().to_string();
    let done = wait_terminal(&harness, &id).await;
    assert_eq!(done["status"], "completed", "{done}");

    // Again without force: the job fails with the --force hint.
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install",
            Some(json!({
                "source": collection.to_string_lossy(),
                "names": ["pytools"],
            })),
            &[],
        )
        .await;
    let id = response.json()["id"].as_str().unwrap().to_string();
    let final_state = wait_terminal(&harness, &id).await;
    assert_eq!(final_state["status"], "failed", "{final_state}");
    assert!(
        final_state["error"]
            .as_str()
            .unwrap_or_default()
            .contains("--force"),
        "the refusal names the fix"
    );

    // With force: replaces.
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install",
            Some(json!({
                "source": collection.to_string_lossy(),
                "names": ["pytools"],
                "force": true,
            })),
            &[],
        )
        .await;
    let id = response.json()["id"].as_str().unwrap().to_string();
    let final_state = wait_terminal(&harness, &id).await;
    assert_eq!(final_state["status"], "completed", "{final_state}");
}

#[tokio::test(flavor = "multi_thread")]
async fn polls_and_cancels_answer_for_unknown_jobs() {
    let harness = harness().await;
    let response = harness
        .send(Method::GET, "/api/plugins/install/nope", None, &[])
        .await;
    assert_eq!(response.status, 404, "{}", response.error_message());

    let response = harness
        .send(Method::POST, "/api/plugins/install/nope/cancel", None, &[])
        .await;
    assert_eq!(response.status, 404);
}

/// The admin gate: with `admin_token` set, the install routes demand it before
/// anything is fetched or built.
#[tokio::test(flavor = "multi_thread")]
async fn install_routes_require_the_admin_token_when_one_is_configured() {
    let harness = HarnessBuilder::new(vec![Script::Text(vec!["ok".into()])])
        .no_ai_titles()
        .admin_token("a-test-admin-token-0123456789")
        .build()
        .await;

    // No header: refused, and *before* anything is fetched (the source does
    // not need to exist for this assertion).
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install/preview",
            Some(json!({ "source": "karucode/anything" })),
            &[],
        )
        .await;
    assert_eq!(response.status, 403, "{}", response.error_message());
    assert!(response.error_message().contains("admin credential"));

    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install",
            Some(json!({ "source": "karucode/anything" })),
            &[],
        )
        .await;
    assert_eq!(response.status, 403, "the job start is gated the same way");

    // Wrong token: refused.
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install/preview",
            Some(json!({ "source": "karucode/anything" })),
            &[("x-karucode-admin-token", "wrong")],
        )
        .await;
    assert_eq!(response.status, 403);

    // Right token: the gate passes (the request itself may still fail for
    // other reasons — a nonexistent repository — but not with 403).
    let response = harness
        .send(
            Method::POST,
            "/api/plugins/install/preview",
            Some(json!({ "source": "this-owner-99887766/nope" })),
            &[("x-karucode-admin-token", "a-test-admin-token-0123456789")],
        )
        .await;
    assert_ne!(response.status, 403, "the right token passes the gate");
}
