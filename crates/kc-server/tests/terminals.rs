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
        .patch(&format!("/api/terminals/{id}"), json!({"title":"build watcher"}))
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

    wait_for("the previous shell's group to exit", || old_terminal.is_dead()).await;
    h.delete(&format!("/api/terminals/{id}")).await;
}

/// The cap is enforced against the rows and names the limit, rather than
/// silently replacing a running shell.
#[tokio::test]
async fn the_terminal_budget_is_enforced() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    // The harness reads the default config, so the budget is `max_terminals`.
    let budget = h
        .get("/api/terminals")
        .await
        .json()["max_terminals"]
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

    let ok = h
        .post("/api/terminals", json!({"cwd":"sub"}))
        .await;
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