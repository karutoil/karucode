//! Terminal HTTP surface, end to end through the real router: create, list,
//! rename, restart and delete, with real shells spawned by the server.
//!
//! The websocket itself needs a live socket, so it is covered by the unit tests
//! in `kc_server::terminals` (a real PTY, real output, real resize) and by the
//! browser check; here the surface and the process lifecycle are exercised.

mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;
use support::HarnessBuilder;

async fn wait_for<T: Fn() -> bool>(what: &str, check: T) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

fn ids(list: &Value) -> Vec<String> {
    list["terminals"]
        .as_array()
        .expect("terminals array")
        .iter()
        .map(|terminal| terminal["id"].as_str().expect("id").to_string())
        .collect()
}

#[tokio::test]
async fn terminals_are_created_listed_renamed_and_deleted_over_http() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let empty = h.get("/api/terminals").await;
    assert_eq!(empty.status, StatusCode::OK);
    let body = empty.json();
    assert_eq!(body["enabled"], true);
    assert!(body["terminals"].as_array().unwrap().is_empty());
    assert_eq!(body["max_terminals"], 16);
    assert!(body["shell"].as_str().unwrap().len() > 1);

    let created = h.post("/api/terminals", json!({})).await;
    assert_eq!(
        created.status,
        StatusCode::OK,
        "create failed: {}",
        created.text()
    );
    let terminal = &created.json()["terminal"];
    let id = terminal["id"].as_str().expect("id").to_string();
    assert!(id.starts_with("t_"), "ids are prefixed for the API shape");
    assert_eq!(terminal["running"], true, "the shell is spawned eagerly");
    assert!(terminal["pid"].as_u64().unwrap() > 0);
    assert_eq!(terminal["position"], 1);
    // The default title names the directory, so a tab strip is readable.
    assert!(
        terminal["title"].as_str().unwrap().ends_with("-1"),
        "unexpected title: {}",
        terminal["title"]
    );
    assert_eq!(
        terminal["cwd"].as_str().unwrap(),
        h.workspace_path().canonicalize().unwrap().to_string_lossy()
    );

    let listed = h.get("/api/terminals").await.json();
    assert_eq!(ids(&listed), vec![id.clone()]);

    // Rename, then reset to the derived default.
    let renamed = h
        .patch(
            &format!("/api/terminals/{id}"),
            json!({"title":"build watcher"}),
        )
        .await;
    assert_eq!(renamed.status, StatusCode::OK);
    assert_eq!(renamed.json()["terminal"]["title"], "build watcher");
    let reset = h
        .patch(&format!("/api/terminals/{id}"), json!({"title":null}))
        .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert!(reset.json()["terminal"]["title"]
        .as_str()
        .unwrap()
        .ends_with("-1"));
    // A PATCH with no `title` key changes nothing at all.
    let noop = h.patch(&format!("/api/terminals/{id}"), json!({})).await;
    assert_eq!(noop.status, StatusCode::OK);

    h.delete(&format!("/api/terminals/{id}")).await;
    assert!(h.get("/api/terminals").await.json()["terminals"]
        .as_array()
        .unwrap()
        .is_empty());
    // Deleting twice is a 404, not a silent success.
    let gone = h.delete(&format!("/api/terminals/{id}")).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    assert_eq!(gone.error_code(), "not_found");
}

/// A restart keeps the id, the title and the position but replaces the process.
#[tokio::test]
async fn restarting_a_terminal_replaces_the_shell_and_keeps_the_tab() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let created = h
        .post("/api/terminals", json!({"title":"watcher"}))
        .await
        .json();
    let id = created["terminal"]["id"].as_str().unwrap().to_string();
    let first_pid = created["terminal"]["pid"].as_u64().unwrap();
    assert!(first_pid > 0);

    // The old shell really goes away: the registry reaps the group.
    let old_terminal = h.state.terminals().get(&id).expect("live shell");
    let restarted = h
        .post(&format!("/api/terminals/{id}/restart"), json!({}))
        .await;
    assert_eq!(
        restarted.status,
        StatusCode::OK,
        "restart failed: {}",
        restarted.text()
    );
    let terminal = &restarted.json()["terminal"];
    assert_eq!(terminal["id"], id.as_str(), "the tab keeps its id");
    assert_eq!(terminal["title"], "watcher", "the tab keeps its name");
    assert_eq!(terminal["position"], 1);
    let new_pid = terminal["pid"].as_u64().unwrap();
    assert_ne!(new_pid, first_pid, "the shell is a new process");
    assert_eq!(terminal["running"], true);

    wait_for("the previous shell's group to exit", || {
        old_terminal.is_dead()
    })
    .await;
    h.delete(&format!("/api/terminals/{id}")).await;
}

/// The cap is enforced against the rows and names the limit, rather than
/// silently replacing a running shell.
#[tokio::test]
async fn the_terminal_budget_is_enforced() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    // The harness reads the default config, so the budget is `max_terminals`.
    let budget = h.get("/api/terminals").await.json()["max_terminals"]
        .as_u64()
        .unwrap() as usize;
    let mut created = Vec::new();
    for _ in 0..budget {
        let response = h.post("/api/terminals", json!({})).await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());
        created.push(
            response.json()["terminal"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let over = h.post("/api/terminals", json!({})).await;
    assert_eq!(over.status, StatusCode::BAD_REQUEST);
    assert_eq!(over.error_code(), "bad_request");
    assert!(
        over.error_message().contains("terminals.max_terminals"),
        "the error must name the setting: {}",
        over.error_message()
    );
    // Closing one frees a slot.
    h.delete(&format!("/api/terminals/{}", created[0])).await;
    assert_eq!(
        h.post("/api/terminals", json!({})).await.status,
        StatusCode::OK
    );
    for id in h.get("/api/terminals").await.json()["terminals"]
        .as_array()
        .unwrap()
    {
        h.delete(&format!("/api/terminals/{}", id["id"].as_str().unwrap()))
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_creates_respect_the_row_budget() {
    let h = HarnessBuilder::new(Vec::new())
        .terminals(kc_core::config::TerminalConfig {
            max_terminals: 1,
            ..Default::default()
        })
        .build()
        .await;
    let (a, b, c, d) = tokio::join!(
        h.post("/api/terminals", json!({})),
        h.post("/api/terminals", json!({})),
        h.post("/api/terminals", json!({})),
        h.post("/api/terminals", json!({})),
    );
    let responses = [a, b, c, d];
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.status == StatusCode::BAD_REQUEST)
            .count(),
        3
    );
    let list = h.get("/api/terminals").await.json();
    let ids = ids(&list);
    assert_eq!(ids.len(), 1);
    h.delete(&format!("/api/terminals/{}", ids[0])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_ensure_live_returns_one_shell() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let created = h.post("/api/terminals", json!({})).await.json();
    let id = created["terminal"]["id"].as_str().unwrap();
    let old = h.state.terminals().remove(id).unwrap();
    old.shutdown();
    let row = h.state.store().get_terminal(id).await.unwrap().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let terminals = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let state = &h.state;
                let row = &row;
                scope.spawn(move || {
                    barrier.wait();
                    kc_server::terminals::ensure_live(state, row, 80, 24).unwrap()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(terminals
        .iter()
        .all(|t| std::sync::Arc::ptr_eq(t, &terminals[0])));
    assert_eq!(h.state.terminals().len(), 1);
    h.delete(&format!("/api/terminals/{id}")).await;
}

#[tokio::test]
async fn json_ping_gets_a_private_text_pong_not_broadcast_pty_bytes() {
    use futures_util::{SinkExt, StreamExt};

    let h = HarnessBuilder::new(Vec::new()).build().await;
    let id = h.post("/api/terminals", json!({})).await.json()["terminal"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = h.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let url = format!("ws://{address}/api/terminals/{id}/socket");
    let (mut first, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut second, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    for socket in [&mut first, &mut second] {
        let ready = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            ready.is_text(),
            "ready control must be JSON text: {ready:?}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(ready.to_text().unwrap()).unwrap()["type"],
            "ready"
        );
    }

    first
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"ping"}"#.to_string(),
        ))
        .await
        .unwrap();
    // The shell is still printing its prompt, so binary PTY frames can arrive on
    // either socket at any time. Read past them: the first socket must get the
    // private text pong, and the second must never get that pong at all.
    let pong = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let message = first
                .next()
                .await
                .expect("first socket closed")
                .expect("first socket failed");
            if message.is_text() {
                let value: Value =
                    serde_json::from_str(message.to_text().unwrap()).unwrap_or(Value::Null);
                if value["type"] == "pong" {
                    return value;
                }
            }
        }
    })
    .await
    .expect("a ping must be answered with a pong within two seconds");
    assert_eq!(pong, json!({"type":"pong"}));
    // Any frame the second socket receives in the next moment must be the shell's
    // own output (binary), never the other browser's pong.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(150);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, second.next()).await {
            Err(_) | Ok(None) => break,
            Ok(Some(Err(error))) => panic!("second socket failed: {error}"),
            Ok(Some(Ok(message))) => assert!(
                !message.is_text(),
                "one browser's ping must not be broadcast to another browser: {message:?}"
            ),
        }
    }

    drop(first);
    drop(second);
    server.abort();
    h.delete(&format!("/api/terminals/{id}")).await;
}

/// Configured non-secret variables reach the shell, but the same passthrough list
/// cannot expose a credential-shaped name or invent an absent value.
#[tokio::test]
async fn terminal_socket_uses_sanitized_live_command_env_passthrough() {
    use futures_util::{SinkExt, StreamExt};

    const VALUE: &str = "KC_SERVER_TERMINAL_TEST_VALUE";
    const SECRET: &str = "KC_SERVER_TERMINAL_TEST_TOKEN";
    const MISSING: &str = "KC_SERVER_TERMINAL_TEST_MISSING";
    std::env::set_var(VALUE, "passed-through");
    std::env::set_var(SECRET, "must-not-appear");
    std::env::remove_var(MISSING);
    let limits = kc_core::config::Limits {
        command_env_passthrough: vec![VALUE.into(), SECRET.into(), MISSING.into()],
        ..Default::default()
    };
    let h = HarnessBuilder::new(Vec::new()).limits(limits).build().await;
    let created = h.post("/api/terminals", json!({})).await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let id = created.json()["terminal"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = h.router.clone();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let url = format!("ws://{address}/api/terminals/{id}/socket");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(ready.to_text().unwrap()).unwrap()["type"],
        "ready"
    );
    let command = "printf 'KC_ENV_RESULT value=<%s> secret=<%s> missing=<%s>\\n' \"$(printenv KC_SERVER_TERMINAL_TEST_VALUE || printf ABSENT)\" \"$(printenv KC_SERVER_TERMINAL_TEST_TOKEN || printf ABSENT)\" \"$(printenv KC_SERVER_TERMINAL_TEST_MISSING || printf ABSENT)\"\n";
    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            command.as_bytes().to_vec(),
        ))
        .await
        .unwrap();
    let mut output = Vec::new();
    for _ in 0..200 {
        let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("terminal output timeout")
            .unwrap()
            .unwrap();
        if frame.is_binary() {
            output.extend_from_slice(&frame.into_data());
        }
        let text = String::from_utf8_lossy(&output);
        if text.contains("KC_ENV_RESULT") && text.contains("missing=<ABSENT>") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("value=<passed-through>"), "{text:?}");
    assert!(text.contains("secret=<ABSENT>"), "{text:?}");
    assert!(text.contains("missing=<ABSENT>"), "{text:?}");
    assert!(!text.contains("must-not-appear"), "{text:?}");

    drop(socket);
    server.abort();
    h.delete(&format!("/api/terminals/{id}")).await;
    std::env::remove_var(VALUE);
    std::env::remove_var(SECRET);
}

/// `terminals.enabled = false` removes the whole surface: no list, no create.
#[tokio::test]
async fn a_disabled_terminal_refuses_every_route() {
    let h = HarnessBuilder::new(Vec::new())
        .terminals(kc_core::config::TerminalConfig {
            enabled: false,
            ..kc_core::config::TerminalConfig::default()
        })
        .build()
        .await;

    let listed = h.get("/api/terminals").await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["enabled"], false);
    assert!(listed.json()["terminals"].as_array().unwrap().is_empty());

    let created = h.post("/api/terminals", json!({})).await;
    assert_eq!(created.status, StatusCode::FORBIDDEN);
    assert!(created.error_message().contains("disabled"));
}

/// A `cwd` outside every workspace root is refused; one inside it is honoured.
#[tokio::test]
async fn the_start_directory_is_confined_to_the_workspace() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let inside = h.workspace_path().join("sub");
    std::fs::create_dir_all(&inside).expect("subdir");

    let ok = h.post("/api/terminals", json!({"cwd":"sub"})).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.text());
    assert_eq!(
        ok.json()["terminal"]["cwd"].as_str().unwrap(),
        inside.canonicalize().unwrap().to_string_lossy()
    );
    let id = ok.json()["terminal"]["id"].as_str().unwrap().to_string();

    // An absolute path outside the project is a path escape, not a shell.
    let outside = h.post("/api/terminals", json!({"cwd":"/etc"})).await;
    assert!(
        outside.status == StatusCode::BAD_REQUEST || outside.status == StatusCode::NOT_FOUND,
        "`cwd: /etc` was accepted: {} {}",
        outside.status,
        outside.text()
    );
    // `..` cannot climb out either.
    let climbing = h.post("/api/terminals", json!({"cwd":"../../.."})).await;
    assert!(climbing.status.is_client_error());

    h.delete(&format!("/api/terminals/{id}")).await;
}

/// An unknown terminal is a `404` on every route that takes an id, and a bad
/// body is a `400` naming the field.
#[tokio::test]
async fn unknown_ids_and_bad_bodies_are_clear_errors() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let missing = "t_does_not_exist";

    assert_eq!(
        h.patch(&format!("/api/terminals/{missing}"), json!({"title":"x"}))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.delete(&format!("/api/terminals/{missing}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.post(&format!("/api/terminals/{missing}/restart"), json!({}))
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // Unknown fields are rejected, so a typo cannot silently do nothing.
    let typo = h.post("/api/terminals", json!({"titel":"x"})).await;
    assert_eq!(typo.status, StatusCode::BAD_REQUEST);
    assert!(typo.error_message().contains("JSON") || typo.error_message().contains("titel"));
}

/// The terminal route inherits the security headers, and a plain GET of the
/// socket route is not a way to read a shell as JSON.
#[tokio::test]
async fn the_terminal_routes_keep_the_security_posture() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let created = h.post("/api/terminals", json!({})).await;
    let id = created.json()["terminal"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A plain GET of the socket route is not an upgrade: it must not expose the
    // shell as JSON, and must still carry the CSP.
    let probe = h.get(&format!("/api/terminals/{id}/socket")).await;
    assert!(probe.status.is_client_error(), "{}", probe.status);
    assert_eq!(probe.headers["x-content-type-options"], "nosniff");
    assert!(probe.headers["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));

    // Creating a terminal is a mutation, so the CSRF header is required. Without
    // it a cross-site page cannot open a shell.
    let no_csrf = h
        .send_with(
            axum::http::Method::POST,
            "/api/terminals",
            Some(json!({})),
            &[],
            false,
        )
        .await;
    assert_eq!(no_csrf.status, StatusCode::FORBIDDEN);
    assert!(no_csrf.error_message().contains("X-Karucode-Client"));

    h.delete(&format!("/api/terminals/{id}")).await;
}

#[tokio::test]
async fn concurrent_rename_and_delete_are_serialized() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    for title in [json!({}), json!({"title":"renamed"})] {
        for _ in 0..20 {
            let created = h.post("/api/terminals", json!({})).await.json();
            let id = created["terminal"]["id"].as_str().unwrap();
            let path = format!("/api/terminals/{id}");
            let (rename, delete) = tokio::join!(h.patch(&path, title.clone()), h.delete(&path));
            assert!(rename.status == StatusCode::OK || rename.status == StatusCode::NOT_FOUND);
            assert_eq!(delete.status, StatusCode::OK);
        }
    }
}
