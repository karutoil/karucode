//! End-to-end HTTP tests: the real router, middlewares, SQLite store and agent
//! runtime, with a scripted provider and a minimal approval tool. No network.

mod support;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;
use support::{
    collect_events_until, events_within, text, tool_call, ApprovalTool, HarnessBuilder, ModelsMode,
    Script,
};

fn event_types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event["type"].as_str())
        .collect()
}

fn assert_security_headers(headers: &axum::http::HeaderMap) {
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert!(
        headers.get("access-control-allow-origin").is_none(),
        "the API must not emit CORS headers by default"
    );
}

#[tokio::test]
async fn health_and_config_roundtrip() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let health = h.get("/api/health").await;
    assert_eq!(health.status, StatusCode::OK);
    assert_security_headers(&health.headers);
    let body = health.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], kc_server::VERSION);
    assert_eq!(body["auth_required"], false);
    assert_eq!(body["provider_configured"], true);
    assert_eq!(body["db"], "ok");
    assert!(body["uptime_secs"].is_u64());
    assert_eq!(
        body["workspace"].as_str().unwrap(),
        h.workspace_path().canonicalize().unwrap().to_string_lossy()
    );

    let config = h.get("/api/config").await;
    assert_eq!(config.status, StatusCode::OK);
    let body = config.json();
    assert_eq!(body["settings_locked"], false);
    assert_eq!(
        body["workspace"].as_str().unwrap(),
        h.workspace_path().canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(body["model"]["model"], "test-model");
    assert_eq!(body["model"]["api_key_set"], false);
    assert_eq!(body["limits"]["max_iterations"], 32);
    assert_eq!(body["approvals"]["mode"], "ask");
    assert_eq!(body["subagents"]["max_concurrent"], 3);
    assert!(body["plugins"].as_array().unwrap().is_empty());
    assert!(body["mcp_servers"].as_array().unwrap().is_empty());
    assert_eq!(body["version"], kc_server::VERSION);

    let stats = h.get("/api/stats").await;
    assert_eq!(stats.status, StatusCode::OK);
    assert_eq!(stats.json()["sessions"], 0);
    assert!(stats.json()["db_path"]
        .as_str()
        .unwrap()
        .ends_with("karucode.db"));

    let unknown = h.get("/api/does-not-exist").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error_code(), "not_found");
    assert!(unknown.headers["content-type"]
        .to_str()
        .unwrap()
        .contains("application/json"));
}

#[tokio::test]
async fn sessions_crud() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let created = h
        .post("/api/sessions", json!({ "title": "Fix login bug" }))
        .await;
    assert_eq!(created.status, StatusCode::OK);
    let session = created.json();
    let id = session["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("s_"));
    assert_eq!(session["title"], "Fix login bug");
    assert_eq!(
        session["workspace"].as_str().unwrap(),
        h.workspace_path().canonicalize().unwrap().to_string_lossy()
    );

    let list = h.get("/api/sessions").await.json();
    let sessions = list["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["id"], id);
    assert_eq!(sessions[0]["message_count"], 0);
    assert_eq!(sessions[0]["running"], false);

    let detail = h.get(&format!("/api/sessions/{id}")).await;
    assert_eq!(detail.status, StatusCode::OK);
    let detail = detail.json();
    assert_eq!(detail["session"]["id"], id);
    assert!(detail["messages"].as_array().unwrap().is_empty());
    assert!(detail["runs"].as_array().unwrap().is_empty());
    assert_eq!(detail["running"], false);

    let renamed = h
        .patch(
            &format!("/api/sessions/{id}"),
            json!({ "title": "Renamed" }),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK);
    assert_eq!(renamed.json()["title"], "Renamed");

    let empty_title = h
        .patch(&format!("/api/sessions/{id}"), json!({ "title": "   " }))
        .await;
    assert_eq!(empty_title.status, StatusCode::OK);
    assert_eq!(empty_title.json()["title"], "New session");

    let bad_patch = h
        .patch(&format!("/api/sessions/{id}"), json!({ "nope": 1 }))
        .await;
    assert_eq!(bad_patch.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad_patch.error_code(), "bad_request");

    let deleted = h.delete(&format!("/api/sessions/{id}")).await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(deleted.json()["deleted"], true);

    for (method, path, body) in [
        (Method::GET, format!("/api/sessions/{id}"), None),
        (
            Method::PATCH,
            format!("/api/sessions/{id}"),
            Some(json!({"title": "x"})),
        ),
        (Method::DELETE, format!("/api/sessions/{id}"), None),
        (
            Method::POST,
            format!("/api/sessions/{id}/messages"),
            Some(json!({"content": "hi"})),
        ),
        (
            Method::POST,
            format!("/api/sessions/{id}/cancel"),
            Some(json!({})),
        ),
    ] {
        let response = h.send(method.clone(), &path, body, &[]).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(response.error_code(), "not_found");
    }
}

#[tokio::test]
async fn session_lifecycle_events_reach_the_stream() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    // session_created
    let (status, headers, body) = h.open_events("", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    let id = h.create_session(Some("streamed")).await;
    let events = collect_events_until(body, "session_created").await;
    let created = events
        .iter()
        .find(|event| event["type"] == "session_created")
        .unwrap();
    assert_eq!(created["session_id"], id);
    assert_eq!(created["title"], "streamed");

    // session_renamed
    let (_, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    let renamed = h
        .patch(&format!("/api/sessions/{id}"), json!({"title": "renamed"}))
        .await;
    assert_eq!(renamed.status, StatusCode::OK);
    let events = collect_events_until(body, "session_renamed").await;
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "session_renamed")
            .unwrap()["title"],
        "renamed"
    );

    // session_deleted
    let (_, _, body) = h.open_events("", &[]).await;
    let deleted = h.delete(&format!("/api/sessions/{id}")).await;
    assert_eq!(deleted.status, StatusCode::OK);
    let events = collect_events_until(body, "session_deleted").await;
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "session_deleted")
            .unwrap()["session_id"],
        id
    );
}

#[tokio::test]
async fn a_message_streams_text_and_finishes() {
    let h = HarnessBuilder::new(vec![Script::Text(vec![
        "Hello".to_string(),
        ", world".to_string(),
    ])])
    .build()
    .await;
    let id = h.create_session(Some("chat")).await;

    let (status, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    assert_eq!(status, StatusCode::OK);

    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "say hi" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    let started_body = started.json();
    let run_id = started_body["run_id"].as_str().unwrap().to_string();
    assert_eq!(started_body["session_id"], id);

    let events = collect_events_until(body, "run_finished").await;
    let types = event_types(&events);
    assert!(types.contains(&"run_started"), "types: {types:?}");
    assert!(types.contains(&"text_delta"), "types: {types:?}");

    let mut text = String::new();
    for event in &events {
        if event["type"] == "text_delta" {
            text.push_str(event["delta"].as_str().unwrap());
        }
    }
    assert_eq!(text, "Hello, world");

    let finished = events
        .iter()
        .find(|event| event["type"] == "run_finished")
        .unwrap();
    assert_eq!(finished["run_id"], run_id);
    assert_eq!(finished["status"], "completed");
    assert_eq!(finished["error"], Value::Null);

    // Every envelope carries the session and a strictly increasing seq.
    let mut last_seq = 0u64;
    for event in &events {
        assert_eq!(event["session_id"], id);
        let seq = event["seq"].as_u64().unwrap();
        assert!(seq > last_seq, "seq must increase: {seq} after {last_seq}");
        last_seq = seq;
        assert!(event["ts_ms"].as_i64().unwrap() > 0);
    }

    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let messages = detail["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "say hi");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"], "Hello, world");
    assert_eq!(detail["runs"][0]["status"], "completed");
    assert_eq!(detail["running"], false);
}

#[tokio::test]
async fn sse_stream_can_be_filtered_to_one_session() {
    let h = HarnessBuilder::new(vec![text("for A")]).build().await;
    let session_a = h.create_session(Some("A")).await;
    let session_b = h.create_session(Some("B")).await;

    let (_, _, body_b) = h.open_events(&format!("?session={session_b}"), &[]).await;
    let (_, _, body_a) = h.open_events(&format!("?session={session_a}"), &[]).await;

    let started = h
        .post(
            &format!("/api/sessions/{session_a}/messages"),
            json!({ "content": "only for A" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);

    let events_a = collect_events_until(body_a, "run_finished").await;
    assert!(events_a
        .iter()
        .all(|event| event["session_id"] == session_a));

    // B's filtered stream must not have seen A's events.
    let events_b = events_within(body_b, Duration::from_millis(250)).await;
    assert!(
        events_b.is_empty(),
        "session-filtered stream leaked events: {events_b:?}"
    );
}

#[tokio::test]
async fn concurrent_run_returns_409_and_cancel_stops_it() {
    let h = HarnessBuilder::new(vec![Script::Hang]).build().await;
    let id = h.create_session(Some("busy")).await;
    let (_, _, body) = h.open_events(&format!("?session={id}"), &[]).await;

    let first = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({"content": "first"}),
        )
        .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);

    let second = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({"content": "second"}),
        )
        .await;
    assert_eq!(second.status, StatusCode::CONFLICT);
    assert_eq!(second.error_code(), "bad_request");
    assert!(second.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already active"));

    let cancel = h
        .post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
    assert_eq!(cancel.status, StatusCode::OK);
    assert_eq!(cancel.json()["cancelled"], true);

    let events = collect_events_until(body, "run_finished").await;
    let finished = events
        .iter()
        .find(|event| event["type"] == "run_finished")
        .unwrap();
    assert_eq!(finished["status"], "cancelled");

    h.wait_until_idle(&id).await;
}

#[tokio::test]
async fn approvals_endpoint_resolves_a_real_approval() {
    let h = HarnessBuilder::new(vec![tool_call("needs_approval", "{}"), text("all done")])
        .tool(std::sync::Arc::new(ApprovalTool))
        .build()
        .await;
    let id = h.create_session(Some("approval")).await;
    let (_, _, body) = h.open_events(&format!("?session={id}"), &[]).await;

    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({"content": "please run it"}),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);

    let approval = h.wait_for_approval(&id).await;
    let approval_id = approval["id"].as_str().unwrap().to_string();
    assert_eq!(approval["kind"], "shell_command");
    assert_eq!(approval["status"], "pending");
    assert_eq!(approval["payload"]["command"], "echo hi");

    let resolved = h
        .post(
            &format!("/api/approvals/{approval_id}"),
            json!({ "decision": "allow_once" }),
        )
        .await;
    assert_eq!(resolved.status, StatusCode::OK);
    assert_eq!(resolved.json()["resolved"], true);
    assert_eq!(resolved.json()["decision"], "allow_once");

    let events = collect_events_until(body, "run_finished").await;
    let types = event_types(&events);
    assert!(types.contains(&"approval_requested"), "types: {types:?}");
    assert!(types.contains(&"approval_resolved"), "types: {types:?}");
    let tool_finished = events
        .iter()
        .find(|event| event["type"] == "tool_call_finished")
        .expect("tool_call_finished");
    assert_eq!(tool_finished["ok"], true);
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "run_finished")
            .unwrap()["status"],
        "completed"
    );

    // The approval row is no longer pending and a second resolve is a 404.
    let pending = h.get(&format!("/api/approvals?session={id}")).await.json();
    assert!(pending["approvals"].as_array().unwrap().is_empty());
    let again = h
        .post(
            &format!("/api/approvals/{approval_id}"),
            json!({ "decision": "deny" }),
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    let invalid = h
        .post("/api/approvals/whatever", json!({ "decision": "maybe" }))
        .await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn workspace_explorer_confines_paths() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    std::fs::create_dir_all(h.workspace_path().join("src")).unwrap();
    std::fs::write(h.workspace_path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(h.workspace_path().join("README.md"), "# hi\n").unwrap();

    let tree = h.get("/api/workspace/tree?path=.&depth=2").await;
    assert_eq!(tree.status, StatusCode::OK);
    let tree = tree.json();
    let entries = tree["entries"].as_array().unwrap();
    assert_eq!(entries[0]["kind"], "dir");
    assert!(entries.iter().any(|entry| entry["path"] == "src/main.rs"));
    assert!(entries.iter().any(|entry| entry["path"] == "README.md"));

    let file = h.get("/api/workspace/file?path=src/main.rs").await;
    assert_eq!(file.status, StatusCode::OK);
    let file = file.json();
    assert_eq!(file["path"], "src/main.rs");
    assert_eq!(file["content"], "fn main() {}\n");
    assert_eq!(file["language"], "rust");
    assert_eq!(file["binary"], false);

    let escape = h.get("/api/workspace/file?path=../../etc/passwd").await;
    assert_eq!(escape.status, StatusCode::BAD_REQUEST);
    assert_eq!(escape.error_code(), "path_escape");

    let escape_tree = h.get("/api/workspace/tree?path=..%2F..%2Fetc").await;
    assert_eq!(escape_tree.status, StatusCode::BAD_REQUEST);
    assert_eq!(escape_tree.error_code(), "path_escape");

    let missing = h.get("/api/workspace/file?path=missing.txt").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.error_code(), "not_found");

    let no_path = h.get("/api/workspace/file").await;
    assert_eq!(no_path.status, StatusCode::BAD_REQUEST);

    let bad_depth = h.get("/api/workspace/tree?path=.&depth=x").await;
    assert_eq!(bad_depth.status, StatusCode::BAD_REQUEST);

    let git_status = h.get("/api/git/status").await;
    assert_eq!(git_status.status, StatusCode::OK);
    // The temp workspace is not a repository; that is a payload, not an error.
    assert_eq!(git_status.json()["is_repo"], false);
}

#[tokio::test]
async fn mutations_require_the_csrf_header_and_a_matching_origin() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let missing_header = h
        .send_with(Method::POST, "/api/sessions", Some(json!({})), &[], false)
        .await;
    assert_eq!(missing_header.status, StatusCode::FORBIDDEN);
    assert_eq!(missing_header.error_code(), "permission_denied");

    let bad_origin = h
        .send(
            Method::POST,
            "/api/sessions",
            Some(json!({})),
            &[("origin", "http://evil.example")],
        )
        .await;
    assert_eq!(bad_origin.status, StatusCode::FORBIDDEN);

    let mismatched_port = h
        .send(
            Method::POST,
            "/api/sessions",
            Some(json!({})),
            &[("origin", "http://127.0.0.1:9999")],
        )
        .await;
    assert_eq!(mismatched_port.status, StatusCode::FORBIDDEN);

    let matching = h
        .send(
            Method::POST,
            "/api/sessions",
            Some(json!({})),
            &[("origin", "http://127.0.0.1:8787")],
        )
        .await;
    assert_eq!(matching.status, StatusCode::OK);

    // GET is not affected.
    let get = h.get("/api/sessions").await;
    assert_eq!(get.status, StatusCode::OK);
}

#[tokio::test]
async fn bearer_auth_is_enforced_and_query_tokens_are_sse_only() {
    let token = "0123456789abcdef0123456789abcdef";
    let h = HarnessBuilder::new(Vec::new())
        .auth_token(token)
        .build()
        .await;

    let unauthenticated = h.get("/api/health").await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unauthenticated.error_code(), "unauthorized");

    let wrong = h
        .get_with("/api/health", &[("authorization", "Bearer nope")])
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    let authorized = h
        .get_with(
            "/api/health",
            &[("authorization", &format!("Bearer {token}"))],
        )
        .await;
    assert_eq!(authorized.status, StatusCode::OK);
    assert_eq!(authorized.json()["auth_required"], true);

    // `?token=` is accepted only for the event stream.
    let query_on_api = h.get(&format!("/api/health?token={token}")).await;
    assert_eq!(query_on_api.status, StatusCode::UNAUTHORIZED);

    let (sse_status, sse_headers, sse_body) = h.open_events(&format!("?token={token}"), &[]).await;
    assert_eq!(sse_status, StatusCode::OK);
    assert!(sse_headers["content-type"]
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    drop(sse_body);

    let (header_status, _, header_body) = h
        .open_events("", &[("authorization", &format!("Bearer {token}"))])
        .await;
    assert_eq!(header_status, StatusCode::OK);
    drop(header_body);
}

#[tokio::test]
async fn provider_test_reports_success_and_redacted_failure() {
    let ok = HarnessBuilder::new(Vec::new())
        .models(ModelsMode::Ok(vec!["a".into(), "b".into()]))
        .build()
        .await;
    let response = ok.post("/api/provider/test", json!({})).await;
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["models"], json!(["a", "b"]));
    assert_eq!(body["error"], Value::Null);
    assert!(body["latency_ms"].is_u64());

    let secret = "sk-test-secret-value";
    let failing = HarnessBuilder::new(vec![Script::Fail(format!("chat failed with {secret}"))])
        .models(ModelsMode::Fail(format!("listing failed with {secret}")))
        .build()
        .await;
    let patched = failing
        .patch("/api/config", json!({ "model": { "api_key": secret } }))
        .await;
    assert_eq!(patched.status, StatusCode::OK);

    let response = failing.post("/api/provider/test", json!({})).await;
    assert_eq!(response.status, StatusCode::OK);
    let text = response.text();
    assert!(!text.contains(secret), "secret leaked: {text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["models"], json!([]));
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("chat probe failed"));
    assert!(body["error"].as_str().unwrap().contains("***"));
}

#[tokio::test]
async fn settings_patch_validates_persists_and_hides_the_key() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let secret = "sk-secret-value-123";

    let response = h
        .patch(
            "/api/config",
            json!({
                "model": {
                    "model": "coder",
                    "temperature": 0.5,
                    "api_key": secret,
                    "max_tokens": null
                },
                "limits": { "max_iterations": 40 },
                "approvals": { "mode": "ask_commands" },
                "subagents": { "max_concurrent": 2 }
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let text = response.text();
    assert!(!text.contains(secret), "the API key must never be echoed");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["model"]["model"], "coder");
    assert_eq!(body["model"]["temperature"], 0.5);
    assert_eq!(body["model"]["api_key_set"], true);
    assert_eq!(body["limits"]["max_iterations"], 40);
    assert_eq!(body["approvals"]["mode"], "ask_commands");
    assert_eq!(body["subagents"]["max_concurrent"], 2);

    // Persisted in SQLite ...
    assert_eq!(
        h.state
            .store()
            .get_setting("model.api_key")
            .await
            .unwrap()
            .as_deref(),
        Some(secret)
    );
    assert_eq!(
        h.state
            .store()
            .get_setting("limits.max_iterations")
            .await
            .unwrap()
            .as_deref(),
        Some("40")
    );
    // ... and readable back through GET.
    let config = h.get("/api/config").await.json();
    assert_eq!(config["model"]["api_key_set"], true);
    assert_eq!(config["limits"]["max_iterations"], 40);

    // A fresh handle over the same database (i.e. a restart) sees the values.
    let reloaded = kc_server::SettingsHandle::new(h.state.settings().snapshot(), h.state.store())
        .await
        .expect("reload settings");
    let reloaded_config = reloaded.snapshot();
    assert_eq!(reloaded_config.model.model, "coder");
    assert_eq!(reloaded_config.limits.max_iterations, 40);
    assert_eq!(reloaded_config.model.api_key.as_deref(), Some(secret));
    assert_eq!(
        reloaded_config.approvals.mode,
        kc_core::config::ApprovalMode::AskCommands
    );

    // `clear_api_key` erases it (including any file/env key).
    let cleared = h
        .patch("/api/config", json!({ "model": { "clear_api_key": true } }))
        .await;
    assert_eq!(cleared.status, StatusCode::OK);
    assert_eq!(cleared.json()["model"]["api_key_set"], false);
    assert_eq!(
        h.state
            .store()
            .get_setting("model.api_key")
            .await
            .unwrap()
            .as_deref(),
        Some("")
    );

    // Validation failures are 400s with a clear message and change nothing.
    let cases = [
        json!({ "model": { "temperature": 3.0 } }),
        json!({ "model": { "temperature": -0.1 } }),
        json!({ "model": { "base_url": "ftp://example.com/v1" } }),
        json!({ "model": { "base_url": "" } }),
        json!({ "model": { "max_tokens": 0 } }),
        json!({ "model": { "top_p": 2.0 } }),
        json!({ "model": { "provider": "" } }),
        json!({ "approvals": { "mode": "sometimes" } }),
        json!({ "subagents": { "max_concurrent": 0 } }),
        json!({ "limits": { "max_iterations": 0 } }),
        json!({ "model": { "typo_field": true } }),
    ];
    for patch in cases {
        let response = h.patch("/api/config", patch.clone()).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "patch {patch} should be rejected"
        );
        assert_eq!(response.error_code(), "bad_request");
    }
    let config = h.get("/api/config").await.json();
    assert_eq!(config["model"]["model"], "coder");
    assert_eq!(config["limits"]["max_iterations"], 40);
}

#[tokio::test]
async fn locked_settings_reject_patches() {
    let h = HarnessBuilder::new(Vec::new())
        .lock_settings()
        .build()
        .await;
    let config = h.get("/api/config").await.json();
    assert_eq!(config["settings_locked"], true);

    let response = h
        .patch("/api/config", json!({ "model": { "model": "other" } }))
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.error_code(), "permission_denied");
    assert!(response.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("locked"));
}

fn unique_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-server-{name}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn static_assets_and_spa_fallback() {
    let web = unique_dir("web");
    std::fs::write(
        web.join("index.html"),
        "<html><body>APP-SHELL</body></html>",
    )
    .unwrap();
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("assets/app-1234abcd.js"), "console.log('hashed')").unwrap();
    std::fs::write(web.join("assets/plain.js"), "console.log('plain')").unwrap();

    let h = HarnessBuilder::new(Vec::new())
        .web_dir(web.clone())
        .build()
        .await;

    let index = h.get("/").await;
    assert_eq!(index.status, StatusCode::OK);
    assert!(index.text().contains("APP-SHELL"));
    assert!(index.headers["content-type"]
        .to_str()
        .unwrap()
        .contains("text/html"));
    assert_eq!(index.headers["cache-control"], "no-cache");
    assert_security_headers(&index.headers);

    let deep_link = h.get("/sessions/s_123").await;
    assert_eq!(deep_link.status, StatusCode::OK);
    assert!(deep_link.text().contains("APP-SHELL"));

    let hashed = h.get("/assets/app-1234abcd.js").await;
    assert_eq!(hashed.status, StatusCode::OK);
    assert!(hashed.headers["content-type"]
        .to_str()
        .unwrap()
        .contains("javascript"));
    assert_eq!(
        hashed.headers["cache-control"],
        "public, max-age=31536000, immutable"
    );

    let plain = h.get("/assets/plain.js").await;
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(plain.headers["cache-control"], "no-cache");

    // Unknown API paths stay JSON even with a SPA installed.
    let api_miss = h.get("/api/nope").await;
    assert_eq!(api_miss.status, StatusCode::NOT_FOUND);
    assert_eq!(api_miss.error_code(), "not_found");

    // Traversal cannot read outside the web directory.
    let traversal = h.get("/../Cargo.toml").await;
    assert_eq!(traversal.status, StatusCode::OK);
    assert!(traversal.text().contains("APP-SHELL"));

    std::fs::remove_dir_all(web).ok();
}

#[tokio::test]
async fn broken_plugin_and_mcp_servers_are_isolated() {
    use kc_core::config::{McpServerConfig, PluginConfig};

    let plugin = PluginConfig {
        name: "broken".into(),
        enabled: true,
        command: vec!["/definitely/not/a/karucode-plugin".into()],
        env: Default::default(),
        permissions: Vec::new(),
        settings: Default::default(),
    };
    let mcp_config = McpServerConfig {
        name: "broken-mcp".into(),
        transport: "stdio".into(),
        command: vec!["/definitely/not/an-mcp-server".into()],
        url: None,
        env: Default::default(),
        headers: Default::default(),
        enabled: true,
        require_approval: true,
    };

    let h = HarnessBuilder::new(vec![text("still works")])
        .plugin(plugin)
        .mcp(mcp_config)
        .build()
        .await;

    let plugins = h.get("/api/plugins").await;
    assert_eq!(plugins.status, StatusCode::OK);
    let plugins = plugins.json();
    assert_eq!(plugins["plugins"][0]["name"], "broken");
    assert_eq!(plugins["plugins"][0]["status"], "failed");
    assert!(plugins["plugins"][0]["error"].as_str().is_some());

    let mcp = h.get("/api/mcp").await;
    assert_eq!(mcp.status, StatusCode::OK);
    let mcp = mcp.json();
    assert_eq!(mcp["servers"][0]["name"], "broken-mcp");
    assert_eq!(mcp["servers"][0]["status"], "failed");
    assert!(mcp["servers"][0]["error"].as_str().is_some());

    let config = h.get("/api/config").await.json();
    assert_eq!(config["plugins"][0]["status"], "failed");
    assert_eq!(config["mcp_servers"][0]["status"], "failed");
    assert!(config["plugins"][0]["tools"].as_array().unwrap().is_empty());

    // Retrying enable returns diagnostics (still failed), not a 500.
    let enabled = h
        .post("/api/plugins/broken/enable", json!({ "enabled": true }))
        .await;
    assert_eq!(enabled.status, StatusCode::OK);
    assert_eq!(enabled.json()["status"], "failed");

    // `/api/config` reads the live host/manager, not the startup config: the
    // entry was configured as enabled, but the live status is `failed`.
    let config_after_enable = h.get("/api/config").await.json();
    assert_eq!(config_after_enable["plugins"][0]["status"], "failed");
    assert_eq!(config_after_enable["mcp_servers"][0]["status"], "failed");

    let unknown = h
        .post("/api/plugins/nope/enable", json!({ "enabled": true }))
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let missing_body = h.post("/api/plugins/broken/enable", json!({})).await;
    assert_eq!(missing_body.status, StatusCode::BAD_REQUEST);

    // The agent still runs with the broken externals absent from the registry.
    let id = h.create_session(Some("isolated")).await;
    let (_, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "hi" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    let events = collect_events_until(body, "run_finished").await;
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "run_finished")
            .unwrap()["status"],
        "completed"
    );
}

#[tokio::test]
async fn git_endpoints_report_status_and_diff() {
    let git_available = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !git_available {
        return;
    }

    let h = HarnessBuilder::new(Vec::new()).build().await;
    let ws = h.workspace_path().to_path_buf();
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("run git");
        assert!(
            status.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("tracked.txt"), "one\n").unwrap();
    run(&["add", "tracked.txt"]);
    run(&["commit", "-q", "-m", "init"]);
    std::fs::write(ws.join("tracked.txt"), "two\n").unwrap();
    std::fs::write(ws.join("untracked.txt"), "new\n").unwrap();

    let status = h.get("/api/git/status").await;
    assert_eq!(status.status, StatusCode::OK);
    let status = status.json();
    assert_eq!(status["is_repo"], true);
    assert_eq!(status["branch"], "main");
    assert_eq!(status["error"], Value::Null);
    let files = status["files"].as_array().unwrap();
    let tracked = files
        .iter()
        .find(|file| file["path"] == "tracked.txt")
        .expect("tracked file in status");
    assert_eq!(tracked["status"], "M");
    assert_eq!(tracked["staged"], false);
    let untracked = files
        .iter()
        .find(|file| file["path"] == "untracked.txt")
        .expect("untracked file in status");
    assert_eq!(untracked["status"], "??");

    let diff = h.get("/api/git/diff?path=tracked.txt").await;
    assert_eq!(diff.status, StatusCode::OK);
    let diff = diff.json();
    assert!(diff["diff"].as_str().unwrap().contains("-one"));
    assert!(diff["diff"].as_str().unwrap().contains("+two"));
    assert_eq!(diff["truncated"], false);

    let staged = h.get("/api/git/diff?path=tracked.txt&staged=true").await;
    assert_eq!(staged.status, StatusCode::OK);
    assert_eq!(staged.json()["diff"], "");

    let bad_ref = h.get("/api/git/diff?base=--upload-pack=evil").await;
    assert_eq!(bad_ref.status, StatusCode::BAD_REQUEST);

    let staged_flag = h.get("/api/git/diff?staged=maybe").await;
    assert_eq!(staged_flag.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn invalid_json_and_bad_bodies_are_clear() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let response = h
        .send(
            Method::POST,
            "/api/sessions",
            None,
            &[("content-type", "application/json")],
        )
        .await;
    // An empty body is allowed (defaults to a new untitled session).
    assert_eq!(response.status, StatusCode::OK);

    let response = h
        .send_raw_body(Method::POST, "/api/sessions", b"{not json".to_vec())
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(response.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("JSON"));

    let id = h.create_session(None).await;
    let empty = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "  " }),
        )
        .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);

    let missing = h
        .post(&format!("/api/sessions/{id}/messages"), json!({}))
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
}

impl support::Harness {
    async fn send_raw_body(
        &self,
        method: Method,
        path: &str,
        body: Vec<u8>,
    ) -> support::TestResponse {
        use axum::body::Body;
        use tower::ServiceExt;
        let request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:8787")
            .header("x-karucode-client", "web")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        support::TestResponse {
            status,
            headers,
            body,
        }
    }
}
