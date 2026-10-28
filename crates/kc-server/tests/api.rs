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
async fn projects_scope_sessions_workspace_and_runs() {
    let h = HarnessBuilder::new(vec![
        tool_call(
            "write_file",
            &json!({ "path": "made-by-project-run.txt", "content": "hello from two" }).to_string(),
        ),
        text("done"),
    ])
    .build()
    .await;

    // The bootstrap-shaped harness seeds one project for its workspace.
    let listed = h.get("/api/projects").await;
    assert_eq!(listed.status, StatusCode::OK);
    let body = listed.json();
    let projects = body["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1, "one default project: {body}");
    let default_id = projects[0]["id"].as_str().unwrap().to_string();
    assert_eq!(projects[0]["session_count"], 0);

    // A second project, a sibling of the default workspace.
    let second_dir = h.workspace_path().parent().unwrap().join("project-two");
    std::fs::create_dir_all(&second_dir).unwrap();
    let second_path = second_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let created = h
        .post(
            "/api/projects",
            json!({ "name": "Second", "path": second_path }),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let project = created.json()["project"].clone();
    let second_id = project["id"].as_str().unwrap().to_string();
    assert_eq!(project["name"], "Second");
    assert_eq!(project["path"], second_path);

    // Path validation: duplicates, relative paths and missing directories are
    // rejected with a message naming the problem, never silently created.
    let duplicate = h
        .post("/api/projects", json!({ "path": second_path }))
        .await;
    assert_eq!(duplicate.status, StatusCode::BAD_REQUEST);
    assert!(duplicate.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already the project"));
    let relative = h
        .post("/api/projects", json!({ "path": "relative/dir" }))
        .await;
    assert_eq!(relative.status, StatusCode::BAD_REQUEST);
    assert!(relative.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("absolute"));
    let missing = h
        .post("/api/projects", json!({ "path": "/definitely/not/here" }))
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);

    // A session in the second project records that directory.
    let session = h
        .post(
            "/api/sessions",
            json!({ "title": "Two", "project": second_id }),
        )
        .await;
    assert_eq!(session.status, StatusCode::OK, "{}", session.text());
    let session = session.json();
    let session_id = session["id"].as_str().unwrap().to_string();
    assert_eq!(session["project_id"], second_id);
    assert_eq!(session["workspace"], second_path);

    // An unknown project is a 404 rather than a silently defaulted session.
    let unknown = h
        .post("/api/sessions", json!({ "project": "p_nope" }))
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);

    // Session listing can be scoped to one project.
    let scoped = h.get(&format!("/api/sessions?project={second_id}")).await;
    assert_eq!(scoped.status, StatusCode::OK);
    assert_eq!(scoped.json()["sessions"].as_array().unwrap().len(), 1);
    let unknown_filter = h.get("/api/sessions?project=p_nope").await;
    assert_eq!(unknown_filter.status, StatusCode::NOT_FOUND);
    assert_eq!(
        h.get("/api/projects").await.json()["projects"][1]["session_count"],
        1
    );

    // Auto approvals so the tool call runs without an approval round trip.
    let patched = h
        .patch("/api/config", json!({ "approvals": { "mode": "auto" } }))
        .await;
    assert_eq!(patched.status, StatusCode::OK);

    let (status, _, stream) = h.open_events(&format!("?session={session_id}"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    let started = h
        .post(
            &format!("/api/sessions/{session_id}/messages"),
            json!({ "content": "write the file" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);
    let events = collect_events_until(stream, "run_finished").await;
    let finished = events
        .iter()
        .find(|event| event["type"] == "run_finished")
        .expect("run finished");
    assert_eq!(finished["status"], "completed", "events: {events:?}");

    // The tool ran inside the project's directory, not the default workspace.
    let written = second_dir.join("made-by-project-run.txt");
    assert!(written.exists(), "the run must write into its project");
    assert!(
        !h.workspace_path().join("made-by-project-run.txt").exists(),
        "the run must not write into the default workspace"
    );

    // Workspace and git endpoints follow ?project=.
    let tree = h
        .get(&format!("/api/workspace/tree?path=.&project={second_id}"))
        .await;
    assert_eq!(tree.status, StatusCode::OK);
    let tree_body = tree.json();
    let names: Vec<&str> = tree_body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"made-by-project-run.txt"), "{names:?}");

    let tree_default = h
        .get(&format!("/api/workspace/tree?path=.&project={default_id}"))
        .await;
    assert_eq!(tree_default.status, StatusCode::OK);
    let tree_default_body = tree_default.json();
    let names: Vec<&str> = tree_default_body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"made-by-project-run.txt"), "{names:?}");

    // Deleting a project with sessions needs force=, and says so.
    let refused = h.delete(&format!("/api/projects/{second_id}")).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("force=true"));
    let deleted = h
        .delete(&format!("/api/projects/{second_id}?force=true"))
        .await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(deleted.json()["deleted_sessions"], 1);
    assert!(h.get(&format!("/api/sessions/{session_id}")).await.status == StatusCode::NOT_FOUND);
    assert_eq!(
        h.get("/api/projects").await.json()["projects"]
            .as_array()
            .unwrap()
            .len(),
        1
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
async fn provider_profiles_can_be_saved_selected_and_probed() {
    let h = HarnessBuilder::new(Vec::new())
        .models(ModelsMode::Ok(vec!["model-a".into(), "model-b".into()]))
        .build()
        .await;
    let secret = "sk-profile-secret-123";

    let created = h.post("/api/config", json!({})).await;
    // POST is not the config verb; the patch below is the real path. (Keeps the
    // test honest about the documented method.)
    assert_eq!(created.status, StatusCode::METHOD_NOT_ALLOWED);

    let response = h
        .patch(
            "/api/config",
            json!({
                "providers": { "upsert": [{
                    "id": "gateway",
                    "name": "My gateway",
                    "base_url": "https://gw.example/v1",
                    "api_key": secret,
                    "models": [
                        {"id": "model-a", "name": "Model A", "context_window": 128000, "tool_call": true},
                        {"id": "model-b"}
                    ]
                }] },
                "active": { "provider": "gateway", "model": "model-b" }
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let text = response.text();
    assert!(!text.contains(secret), "the key leaked into /api/config");
    let body: Value = serde_json::from_str(&text).unwrap();
    let profile = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "gateway")
        .expect("saved profile");
    assert_eq!(profile["name"], "My gateway");
    assert_eq!(profile["api_key_set"], true);
    assert_eq!(profile["models"][0]["context_window"], 128000);
    assert_eq!(body["active"]["provider"], "gateway");
    assert_eq!(body["active"]["model"], "model-b");
    // The flat view follows the active profile.
    assert_eq!(body["model"]["base_url"], "https://gw.example/v1");
    assert_eq!(body["model"]["model"], "model-b");
    assert_eq!(body["model"]["api_key_set"], true);

    // Probing a saved profile by id and a draft endpoint both work.
    let probe = h
        .post("/api/provider/test", json!({ "profile": "gateway" }))
        .await;
    assert_eq!(probe.status, StatusCode::OK);
    assert_eq!(probe.json()["ok"], true);
    assert_eq!(probe.json()["models"], json!(["model-a", "model-b"]));

    let draft = h
        .post(
            "/api/provider/test",
            json!({ "base_url": "http://127.0.0.1:9/v1", "model": "draft-model" }),
        )
        .await;
    assert_eq!(draft.status, StatusCode::OK);
    assert_eq!(draft.json()["ok"], true, "the scripted factory accepts it");

    let bad_url = h
        .post(
            "/api/provider/test",
            json!({ "base_url": "ftp://nope", "model": "m" }),
        )
        .await;
    assert_eq!(bad_url.status, StatusCode::OK);
    assert_eq!(bad_url.json()["ok"], false);
    assert!(bad_url.json()["error"]
        .as_str()
        .unwrap()
        .contains("http://"));

    let missing = h
        .post("/api/provider/test", json!({ "profile": "nope" }))
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.error_code(), "not_found");

    // An explicit but unknown model is rejected, not silently rewritten.
    let bad_selection = h
        .patch(
            "/api/config",
            json!({ "active": { "provider": "gateway", "model": "not-in-list" } }),
        )
        .await;
    assert_eq!(bad_selection.status, StatusCode::BAD_REQUEST);
    assert!(bad_selection.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("active.model"));

    // Deleting the active profile re-selects a remaining one.
    let deleted = h
        .patch(
            "/api/config",
            json!({ "providers": { "delete": ["gateway"] } }),
        )
        .await;
    assert_eq!(deleted.status, StatusCode::OK);
    let body = deleted.json();
    assert!(!body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "gateway"));
    assert_ne!(body["active"]["provider"], "gateway");

    // The selection and profiles survive a restart (persisted rows).
    let config = h.get("/api/config").await.json();
    assert!(config["providers"].as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn models_catalog_lists_providers_and_filters_one() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let catalog = kc_server::catalog::parse_catalog(&json!({
        "openai": {
            "id": "openai", "name": "OpenAI",
            "models": {
                "gpt-a": {
                    "id": "gpt-a", "name": "GPT A", "tool_call": true,
                    "limit": {"context": 128000, "output": 16384},
                    "cost": {"input": 0.25, "output": 1.5}
                }
            }
        },
        "local": {"id": "local", "name": "Local", "models": {"tiny": {"id": "tiny"}}}
    }))
    .unwrap();
    h.state.catalog().seed(catalog);

    let list = h.get("/api/models/catalog").await;
    assert_eq!(list.status, StatusCode::OK);
    let body = list.json();
    assert_eq!(body["ok"], true);
    assert_eq!(body["stale"], false);
    let ids: Vec<&str> = body["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"openai") && ids.contains(&"local"));
    assert!(body["models"].as_array().unwrap().is_empty());

    let one = h.get("/api/models/catalog?provider=openai").await;
    assert_eq!(one.status, StatusCode::OK);
    let body = one.json();
    assert_eq!(body["provider"]["name"], "OpenAI");
    assert_eq!(body["models"].as_array().unwrap().len(), 1);
    let model = &body["models"][0];
    assert_eq!(model["context_window"], 128000);
    assert_eq!(model["max_output_tokens"], 16384);
    assert_eq!(model["tool_call"], true);
    assert_eq!(model["cost_input"], 0.25);

    let unknown = h.get("/api/models/catalog?provider=nope").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error_code(), "not_found");

    let bad_query = h.get("/api/models/catalog?refresh=maybe").await;
    assert_eq!(bad_query.status, StatusCode::BAD_REQUEST);
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

// ---------------------------------------------------------------------------
// DNS-rebinding (`Host` allowlist) and settings bounds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rebound_host_with_matching_origin_is_rejected() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    // The reproduced attack: a rebound attacker domain agrees with itself in
    // both `Host` and `Origin`, so an Origin==Host comparison alone accepts it.
    let rebound = h
        .send(
            Method::PATCH,
            "/api/config",
            Some(json!({ "approvals": { "mode": "auto" } })),
            &[("host", "evil.example"), ("origin", "http://evil.example")],
        )
        .await;
    assert_eq!(rebound.status, StatusCode::FORBIDDEN);
    assert_eq!(rebound.error_code(), "permission_denied");
    let message = rebound.json()["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for expected in ["127.0.0.1", "localhost", "::1", "KARUCODE_ALLOWED_HOSTS"] {
        assert!(
            message.contains(expected),
            "the 403 must name the allowed hosts, got: {message}"
        );
    }

    // Nothing was written: approval mode is still the default.
    let config = h.get("/api/config").await.json();
    assert_eq!(config["approvals"]["mode"], "ask");

    // Reads are guarded too: a rebound page must not read the config.
    let read = h.get_with("/api/config", &[("host", "evil.example")]).await;
    assert_eq!(read.status, StatusCode::FORBIDDEN);
    assert_eq!(read.error_code(), "permission_denied");

    // Loopback-looking-but-not-loopback names and foreign names are rejected.
    for host in [
        "127.0.0.1.evil.example",
        "localhost.evil.example",
        "evil.example:8787",
        "0.0.0.0:8787",
    ] {
        let response = h.get_with("/api/health", &[("host", host)]).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "host {host}");
        assert_eq!(response.error_code(), "permission_denied", "host {host}");
    }
}

#[tokio::test]
async fn loopback_hosts_are_served_without_a_token() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    for host in [
        "127.0.0.1:8787",
        "127.0.0.1",
        "localhost:8787",
        "LOCALHOST:8787",
        "[::1]:8787",
        "::1",
    ] {
        let response = h.get_with("/api/health", &[("host", host)]).await;
        assert_eq!(response.status, StatusCode::OK, "host {host}");
        assert_eq!(response.json()["status"], "ok");
    }

    // A mutation from a loopback page still needs the CSRF header and a matching
    // origin, and now passes the Host check.
    let patched = h
        .send(
            Method::PATCH,
            "/api/config",
            Some(json!({ "limits": { "max_iterations": 40 } })),
            &[
                ("host", "127.0.0.1:8787"),
                ("origin", "http://127.0.0.1:8787"),
            ],
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    assert_eq!(patched.json()["limits"]["max_iterations"], 40);
}

#[tokio::test]
async fn allowed_hosts_env_permits_a_configured_proxy_host() {
    // The bootstrap reads KARUCODE_ALLOWED_HOSTS once at startup; the window in
    // which this test mutates the process environment is therefore tiny. Other
    // tests in the binary use loopback hosts, which stay allowed either way.
    std::env::set_var(
        kc_server::host::ALLOWED_HOSTS_ENV,
        "karucode.internal.test, *.proxy.test:8443",
    );
    let h = HarnessBuilder::new(Vec::new()).build().await;
    std::env::remove_var(kc_server::host::ALLOWED_HOSTS_ENV);

    for host in ["karucode.internal.test", "karucode.internal.test:9443"] {
        let response = h.get_with("/api/health", &[("host", host)]).await;
        assert_eq!(response.status, StatusCode::OK, "host {host}");
    }
    let subdomain = h
        .get_with("/api/health", &[("host", "app.proxy.test:8443")])
        .await;
    assert_eq!(subdomain.status, StatusCode::OK);

    // The wildcard does not imply the apex, and ports must match when given.
    for host in [
        "proxy.test:8443",
        "app.proxy.test:9443",
        "evil.example",
        "karucode.internal.test.evil.example",
    ] {
        let response = h.get_with("/api/health", &[("host", host)]).await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "host {host}");
    }
}

#[tokio::test]
async fn a_bearer_token_disables_the_host_restriction() {
    let token = "0123456789abcdef0123456789abcdef";
    let h = HarnessBuilder::new(Vec::new())
        .auth_token(token)
        .build()
        .await;
    let auth = format!("Bearer {token}");

    // Without the token the request stops at authentication, not at the guard.
    let unauthenticated = h
        .get_with("/api/health", &[("host", "karucode.example.com:8787")])
        .await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);

    // With it, any Host is served as long as Origin matches (reverse proxies are
    // free to rewrite Host; the token is the credential).
    let patched = h
        .send(
            Method::PATCH,
            "/api/config",
            Some(json!({ "limits": { "max_iterations": 41 } })),
            &[
                ("host", "karucode.example.com:8787"),
                ("origin", "http://karucode.example.com:8787"),
                ("authorization", auth.as_str()),
            ],
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());

    // A mismatched Origin is still refused with a token configured.
    let cross_origin = h
        .send(
            Method::PATCH,
            "/api/config",
            Some(json!({ "limits": { "max_iterations": 42 } })),
            &[
                ("host", "karucode.example.com:8787"),
                ("origin", "http://evil.example"),
                ("authorization", auth.as_str()),
            ],
        )
        .await;
    assert_eq!(cross_origin.status, StatusCode::FORBIDDEN);
    assert_eq!(cross_origin.error_code(), "permission_denied");
}

#[tokio::test]
async fn out_of_range_settings_are_rejected_with_the_bounds() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let cases: [(&str, Value); 14] = [
        (
            "limits.max_file_read_bytes",
            json!({ "limits": { "max_file_read_bytes": 4_000_000_000u64 } }),
        ),
        (
            "limits.max_tool_output_bytes",
            json!({ "limits": { "max_tool_output_bytes": 100_000_000 } }),
        ),
        (
            "limits.max_command_output_bytes",
            json!({ "limits": { "max_command_output_bytes": 255 } }),
        ),
        (
            "limits.context_max_chars",
            json!({ "limits": { "context_max_chars": 9_999 } }),
        ),
        (
            "limits.max_iterations",
            json!({ "limits": { "max_iterations": 501 } }),
        ),
        (
            "limits.max_run_secs",
            json!({ "limits": { "max_run_secs": 9 } }),
        ),
        (
            "limits.command_timeout_secs",
            json!({ "limits": { "command_timeout_secs": 0 } }),
        ),
        (
            "limits.max_command_timeout_secs",
            json!({ "limits": { "max_command_timeout_secs": 3_601 } }),
        ),
        (
            "limits.search_max_results",
            json!({ "limits": { "search_max_results": 2_001 } }),
        ),
        (
            "limits.max_tool_calls_per_turn",
            json!({ "limits": { "max_tool_calls_per_turn": 33 } }),
        ),
        (
            "subagents.max_concurrent",
            json!({ "subagents": { "max_concurrent": 17 } }),
        ),
        (
            "subagents.max_iterations",
            json!({ "subagents": { "max_iterations": 101 } }),
        ),
        (
            "subagents.max_depth",
            json!({ "subagents": { "max_depth": 5 } }),
        ),
        (
            "model.temperature",
            json!({ "model": { "temperature": 2.5 } }),
        ),
    ];
    for (field, patch) in cases {
        let response = h.patch("/api/config", patch.clone()).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "patch {patch} should be rejected"
        );
        assert_eq!(response.error_code(), "bad_request");
        let message = response.json()["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(field),
            "the 400 for {patch} must name {field}, got: {message}"
        );
        assert!(
            message.contains("between"),
            "the 400 for {patch} must state the bounds, got: {message}"
        );
    }

    // Rejections are no-ops: the OOM-enabling value never took effect.
    let config = h.get("/api/config").await.json();
    assert_eq!(config["limits"]["max_file_read_bytes"], 200_000);
    assert_eq!(config["limits"]["max_iterations"], 32);
    assert_eq!(config["subagents"]["max_depth"], 1);

    // The bounds themselves are inclusive.
    let boundary = h
        .patch(
            "/api/config",
            json!({
                "model": { "temperature": 2.0 },
                "limits": {
                    "max_file_read_bytes": 5_000_000,
                    "max_tool_output_bytes": 1_000_000,
                    "max_command_output_bytes": 1_000_000,
                    "context_max_chars": 10_000,
                    "max_iterations": 500,
                    "max_run_secs": 86_400,
                    "command_timeout_secs": 3_600,
                    "max_command_timeout_secs": 3_600,
                    "search_max_results": 2_000,
                    "max_tool_calls_per_turn": 32
                },
                "subagents": { "max_concurrent": 16, "max_iterations": 100, "max_depth": 0 }
            }),
        )
        .await;
    assert_eq!(boundary.status, StatusCode::OK, "{}", boundary.text());
    let body = boundary.json();
    assert_eq!(body["limits"]["max_file_read_bytes"], 5_000_000);
    assert_eq!(body["subagents"]["max_depth"], 0);
}

/// Write an executable marker script (used to prove git helpers do not run).
#[cfg(unix)]
fn write_marker_script(path: &std::path::Path, marker: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display());
    std::fs::write(path, script).expect("write marker script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod marker script");
}

#[cfg(unix)]
fn git_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .output()
        .expect("run git")
}

#[cfg(unix)]
#[tokio::test]
async fn git_endpoints_do_not_run_repository_helpers() {
    let git_available = std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    if !git_available {
        eprintln!("skipping: git is not installed");
        return;
    }

    let h = HarnessBuilder::new(Vec::new()).build().await;
    let ws = h.workspace_path().to_path_buf();
    let fsmonitor_marker = ws.join("fsmonitor-ran");
    let diff_marker = ws.join("diffexternal-ran");
    let fsmonitor_script = ws.join("fsmonitor.sh");
    let diff_script = ws.join("external-diff.sh");
    write_marker_script(&fsmonitor_script, &fsmonitor_marker);
    write_marker_script(&diff_script, &diff_marker);

    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "tracked.txt"],
        vec!["commit", "-q", "-m", "init"],
    ] {
        if args[0] == "add" {
            std::fs::write(ws.join("tracked.txt"), "one\n").unwrap();
        }
        let output = git_in(&ws, &args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (key, value) in [
        ("core.fsmonitor", fsmonitor_script.display().to_string()),
        ("diff.external", diff_script.display().to_string()),
    ] {
        let output = git_in(&ws, &["config", key, &value]);
        assert!(output.status.success(), "git config {key} failed");
    }
    // An unstaged modification so `git diff` actually has to produce a patch.
    std::fs::write(ws.join("tracked.txt"), "one\ntwo\n").unwrap();

    // Sanity: without the hardening these helpers really would run. This is
    // informational (hook invocation differs between git versions); the
    // assertions below are the guarantee.
    let _ = git_in(&ws, &["status", "--porcelain"]);
    let _ = git_in(&ws, &["diff"]);
    let fixture_live = fsmonitor_marker.exists() || diff_marker.exists();
    for marker in [&fsmonitor_marker, &diff_marker] {
        std::fs::remove_file(marker).ok();
    }
    if !fixture_live {
        eprintln!(
            "note: this git build did not invoke the configured helpers; the \
             'must not run' assertions below are still the guarantee"
        );
    }

    let status = h.get("/api/git/status").await;
    assert_eq!(status.status, StatusCode::OK);
    let status = status.json();
    assert_eq!(status["is_repo"], true);
    assert_eq!(status["branch"], "main");
    assert!(
        !fsmonitor_marker.exists(),
        "`core.fsmonitor` must not be executed by /api/git/status"
    );

    let diff = h.get("/api/git/diff").await;
    assert_eq!(diff.status, StatusCode::OK, "{}", diff.text());
    let diff = diff.json();
    assert!(
        diff["diff"].as_str().unwrap_or_default().contains("+two"),
        "the hardened diff must still produce a patch: {diff}"
    );
    assert!(
        !diff_marker.exists(),
        "`diff.external` must not be executed by /api/git/diff"
    );
}

#[tokio::test]
async fn only_supported_model_providers_can_be_saved() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    // The UI selector is driven by this additive list.
    let config = h.get("/api/config").await.json();
    assert_eq!(config["available_providers"], json!(["openai_compatible"]));
    assert_eq!(config["model"]["provider"], "openai_compatible");

    let unknown = h
        .patch(
            "/api/config",
            json!({ "model": { "provider": "anthropic" } }),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    assert_eq!(unknown.error_code(), "bad_request");
    let message = unknown.json()["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(message.contains("model.provider"), "{message}");
    assert!(message.contains("openai_compatible"), "{message}");
    // The lie was not persisted.
    assert_eq!(
        h.get("/api/config").await.json()["model"]["provider"],
        "openai_compatible"
    );

    // A supported id is accepted (and stored trimmed).
    let accepted = h
        .patch(
            "/api/config",
            json!({ "model": { "provider": " openai_compatible " } }),
        )
        .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.text());
    assert_eq!(accepted.json()["model"]["provider"], "openai_compatible");
}

#[tokio::test]
async fn a_corrupt_persisted_row_cannot_prevent_startup() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let configured_base_url = h.state.settings().snapshot().model.base_url;
    assert_eq!(configured_base_url, "http://127.0.0.1:9/v1");

    // Simulate an older build (or a hand-edited database) leaving a value that
    // today's validation rejects, plus one row that is still fine.
    let store = h.state.store();
    store
        .set_setting("model.base_url", "ftp://not-a-url")
        .await
        .unwrap();
    store
        .set_setting("limits.max_iterations", "99999")
        .await
        .unwrap();
    store
        .set_setting("limits.search_max_results", "7")
        .await
        .unwrap();

    // A fresh handle over the same database is exactly what a restart does: it
    // must succeed and fall back to the file/env value for the bad keys.
    let restarted = kc_server::SettingsHandle::new(h.state.settings().snapshot(), store.clone())
        .await
        .expect("a persisted row must never prevent startup");
    let config = restarted.snapshot();
    assert_eq!(
        config.model.base_url, configured_base_url,
        "the file/env base_url must stay in force"
    );
    assert_eq!(config.limits.max_iterations, 32, "invalid row ignored");
    assert_eq!(config.limits.search_max_results, 7, "valid row applied");
    // (The skip is logged at WARN with the key; `apply_rows_resilient`'s unit
    // test covers the policy itself, this covers the startup path.)

    // The operational (file/env) layer is still validated loudly. That layer
    // feeds the provider profile, so break the profile the way a bad
    // KARUCODE_BASE_URL would.
    let mut broken = restarted.snapshot();
    broken.providers[0].base_url = "ftp://still-not-an-url".into();
    broken.normalize_providers();
    assert!(
        kc_server::SettingsHandle::new(broken, store).await.is_err(),
        "a bad file/env value must abort startup"
    );
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

#[tokio::test]
async fn first_message_titles_an_untitled_session() {
    let h = HarnessBuilder::new(vec![text("ok")]).build().await;
    let id = h.create_session(None).await;
    assert_eq!(
        h.get(&format!("/api/sessions/{id}")).await.json()["session"]["title"],
        "New session"
    );

    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "  Fix   the\n\nlogin\tbug  " }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);

    // The title is persisted before the run starts, so it is already visible.
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(detail["session"]["title"], "Fix the login bug");
    let list = h.get("/api/sessions").await.json();
    let session = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == id)
        .expect("session in list");
    assert_eq!(session["title"], "Fix the login bug");

    h.wait_until_idle(&id).await;
}

#[tokio::test]
async fn second_message_keeps_the_derived_title() {
    let h = HarnessBuilder::new(vec![text("one"), text("two")])
        .build()
        .await;
    let id = h.create_session(None).await;

    let first = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "first prompt about parsing" }),
        )
        .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    h.wait_until_idle(&id).await;
    let title = h.get(&format!("/api/sessions/{id}")).await.json()["session"]["title"].clone();
    assert_eq!(title, "first prompt about parsing");

    let second = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "second prompt that must not retitle anything" }),
        )
        .await;
    assert_eq!(second.status, StatusCode::ACCEPTED);
    h.wait_until_idle(&id).await;
    assert_eq!(
        h.get(&format!("/api/sessions/{id}")).await.json()["session"]["title"],
        title
    );
}

#[tokio::test]
async fn an_explicitly_renamed_session_keeps_its_title() {
    let h = HarnessBuilder::new(vec![text("ok")]).build().await;
    let id = h.create_session(None).await;

    let renamed = h
        .patch(
            &format!("/api/sessions/{id}"),
            json!({ "title": "Release checklist" }),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK);

    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "a prompt that would otherwise become the title" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);

    assert_eq!(
        h.get(&format!("/api/sessions/{id}")).await.json()["session"]["title"],
        "Release checklist"
    );
    h.wait_until_idle(&id).await;
}

#[tokio::test]
async fn an_auto_title_is_announced_on_the_event_stream() {
    let h = HarnessBuilder::new(vec![text("ok")]).build().await;
    let id = h.create_session(None).await;
    let (status, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    assert_eq!(status, StatusCode::OK);

    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "Investigate the flaky migration test" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED);

    let events = collect_events_until(body, "session_renamed").await;
    let renamed = events
        .iter()
        .find(|event| event["type"] == "session_renamed")
        .expect("session_renamed event");
    assert_eq!(renamed["session_id"], id);
    assert_eq!(renamed["title"], "Investigate the flaky migration test");
    // The rename is emitted before the run starts.
    assert!(
        !event_types(&events).contains(&"run_started"),
        "session_renamed must precede run_started: {:?}",
        event_types(&events)
    );

    h.wait_until_idle(&id).await;
}
