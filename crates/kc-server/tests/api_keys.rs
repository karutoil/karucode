//! Application API keys: minting, scopes, expiry, rotation and revocation,
//! driven through the real router.
//!
//! The store's own behaviour (hash-only storage, retention, last-use flush) is
//! covered by the unit tests in `crates/kc-server/src/api_keys.rs`; these tests
//! prove the HTTP contract and, most importantly, that the auth middleware
//! actually enforces a key's scope instead of trusting the route.

mod support;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use support::{Harness, HarnessBuilder};

const MASTER: &str = "test-auth-token-0123456789";

/// A server that already requires a credential, which is the only state in which
/// creating a key is allowed.
async fn harness() -> Harness {
    HarnessBuilder::new(Vec::new())
        .auth_token(MASTER)
        .build()
        .await
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn create(h: &Harness, body: Value) -> Value {
    let auth = bearer(MASTER);
    let response = h
        .send(
            Method::POST,
            "/api/auth/api-keys",
            Some(body),
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()
}

#[tokio::test]
async fn a_read_write_key_works_as_a_bearer_and_is_enforced_by_scope() {
    let h = harness().await;
    let body = create(
        &h,
        json!({"name":"ci-runner","scopes":["read","write"],"expires_in_days":30}),
    )
    .await;
    let token = body["token"].as_str().expect("token").to_string();
    assert!(token.starts_with("kck_"));
    assert_eq!(body["api_key"]["name"], json!("ci-runner"));
    assert_eq!(body["api_key"]["status"], json!("active"));
    assert_eq!(body["api_key"]["scopes"], json!(["read", "write"]));
    assert!(body["warning"].is_null());

    let auth = bearer(&token);
    // Read.
    let config = h
        .send(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(config.status, StatusCode::OK, "{}", config.text());
    // Write, with the CSRF header the harness adds for mutations.
    let session = h
        .send(
            Method::POST,
            "/api/sessions",
            Some(json!({"title":"from a key"})),
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(session.status, StatusCode::OK, "{}", session.text());

    // Key management needs admin, which this key does not have.
    let keys = h
        .send(
            Method::GET,
            "/api/auth/api-keys",
            None,
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(keys.status, StatusCode::FORBIDDEN, "{}", keys.text());
    assert_eq!(keys.error_code(), "permission_denied");
    assert!(keys.error_message().contains("admin"));

    // A read-only key is refused a mutation, and the message names the scope.
    let body = create(&h, json!({"name":"monitor","scopes":["read"]})).await;
    let read_only = body["token"].as_str().expect("token").to_string();
    let auth = bearer(&read_only);
    let read = h
        .send(
            Method::GET,
            "/api/sessions",
            None,
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text());
    let write = h
        .send(
            Method::POST,
            "/api/sessions",
            Some(json!({})),
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(write.status, StatusCode::FORBIDDEN, "{}", write.text());
    assert!(write.error_message().contains("write scope"));
}

#[tokio::test]
async fn the_token_is_returned_once_and_the_list_never_contains_it() {
    let h = harness().await;
    let body = create(&h, json!({"name":"needs-a-lifetime"})).await;
    let token = body["token"].as_str().expect("token").to_string();
    assert!(body["warning"]
        .as_str()
        .unwrap_or("")
        .contains("never expires"));

    let auth = bearer(MASTER);
    let list = h
        .send(
            Method::GET,
            "/api/auth/api-keys",
            None,
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text());
    let list_body = list.json();
    assert_eq!(list_body["keys"].as_array().map(Vec::len), Some(1));
    assert_eq!(list_body["max_keys"], json!(100));
    assert!(!list.text().contains(&token));
    // Only a recognisable prefix and the last four characters are shown.
    let key = &list_body["keys"][0];
    assert!(key["prefix"].as_str().unwrap_or("").starts_with("kck_"));
    assert_eq!(key["last4"].as_str().map(str::len), Some(4));
}

#[tokio::test]
async fn revocation_is_immediate_and_rotation_replaces_the_secret() {
    let h = harness().await;
    let master = bearer(MASTER);
    let body = create(&h, json!({"name":"leaked","scopes":["read"]})).await;
    let id = body["api_key"]["id"].as_str().expect("id").to_string();
    let token = body["token"].as_str().expect("token").to_string();

    let revoked = h
        .send(
            Method::DELETE,
            &format!("/api/auth/api-keys/{id}"),
            None,
            &[("authorization", &master)],
        )
        .await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.text());
    let auth = bearer(&token);
    let denied = h
        .send(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    assert!(denied.error_message().contains("revoked"));
    assert!(!denied.error_message().contains(&token));

    // A fresh key rotates: the old token dies and the new one has the same id.
    let body = create(&h, json!({"name":"rotating","scopes":["read"]})).await;
    let id = body["api_key"]["id"].as_str().expect("id").to_string();
    let old = body["token"].as_str().expect("token").to_string();
    let rotated = h
        .send(
            Method::POST,
            &format!("/api/auth/api-keys/{id}/rotate"),
            Some(json!({})),
            &[("authorization", &master)],
        )
        .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.text());
    let rotated_body = rotated.json();
    let new = rotated_body["token"].as_str().expect("token").to_string();
    assert_ne!(old, new);
    assert_eq!(rotated_body["api_key"]["id"], json!(id));
    let stale = h
        .send(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", &bearer(&old))],
        )
        .await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    let fresh = h
        .send(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", &bearer(&new))],
        )
        .await;
    assert_eq!(fresh.status, StatusCode::OK, "{}", fresh.text());
}

#[tokio::test]
async fn an_expired_key_is_refused_with_the_reason() {
    let h = harness().await;
    // The smallest expiry the API accepts is one second in the future.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    let body = create(
        &h,
        json!({"name":"short-lived","scopes":["read"],"expires_at": now + 3}),
    )
    .await;
    let token = body["token"].as_str().expect("token").to_string();
    tokio::time::sleep(std::time::Duration::from_millis(3_500)).await;
    let denied = h
        .send(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", &bearer(&token))],
        )
        .await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    assert!(denied.error_message().contains("expired"));
}

#[tokio::test]
async fn a_key_cannot_be_created_while_the_server_is_open() {
    // The historical default: loopback, no token, no sign-in. A key would add no
    // protection, so creation names what to set first instead of pretending.
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let response = h
        .post("/api/auth/api-keys", json!({"name":"pointless"}))
        .await;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.text()
    );
    assert!(response.error_message().contains("requires no credential"));
}

#[tokio::test]
async fn me_identifies_an_api_key_and_its_scopes() {
    let h = harness().await;
    let body = create(
        &h,
        json!({"name":"identified","scopes":["read","admin"],"expires_in_days":1}),
    )
    .await;
    let id = body["api_key"]["id"].as_str().expect("id").to_string();
    let token = body["token"].as_str().expect("token").to_string();
    let me = h
        .send(
            Method::GET,
            "/api/auth/me",
            None,
            &[("authorization", &bearer(&token))],
        )
        .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.text());
    let me = me.json();
    assert_eq!(me["principal"], json!("api_key"));
    assert_eq!(me["label"], json!("identified"));
    assert_eq!(me["api_key_id"], json!(id));
    assert_eq!(me["scopes"], json!(["read", "admin"]));
    assert_eq!(me["signed_in"], json!(false));
}

#[tokio::test]
async fn an_admin_scoped_key_reaches_key_management_but_not_the_admin_gate_it_does_not_have() {
    let h = harness().await;
    let body = create(
        &h,
        json!({"name":"operator","scopes":["admin"],"expires_in_days":1}),
    )
    .await;
    let token = body["token"].as_str().expect("token").to_string();
    let auth = bearer(&token);
    // Key management itself is an admin surface.
    let list = h
        .send(
            Method::GET,
            "/api/auth/api-keys",
            None,
            &[("authorization", &auth)],
        )
        .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text());
    // Approvals are the human gate: an admin key may answer them, a write key
    // may not (covered above by the scope matrix). A missing approval is a 404,
    // which proves the request reached the handler rather than being scoped out.
    let approval = h
        .send(
            Method::POST,
            "/api/approvals/does-not-exist",
            Some(json!({"decision":"approve"})),
            &[("authorization", &auth)],
        )
        .await;
    assert_ne!(
        approval.status,
        StatusCode::FORBIDDEN,
        "{}",
        approval.text()
    );
}

#[tokio::test]
async fn a_write_key_cannot_answer_an_approval() {
    let h = harness().await;
    let body = create(&h, json!({"name":"automation","scopes":["write"]})).await;
    let token = body["token"].as_str().expect("token").to_string();
    let response = h
        .send(
            Method::POST,
            "/api/approvals/does-not-exist",
            Some(json!({"decision":"approve"})),
            &[("authorization", &bearer(&token))],
        )
        .await;
    assert_eq!(
        response.status,
        StatusCode::FORBIDDEN,
        "{}",
        response.text()
    );
    assert!(response.error_message().contains("admin scope"));
}
