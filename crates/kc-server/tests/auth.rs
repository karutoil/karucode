//! Web-UI sign-in, the Access settings and the tunnel/domain wiring, driven
//! through the real router.
//!
//! These tests are the ones that would have caught a lockout: every case here is
//! about *who gets in*. A passkey ceremony cannot be driven from a `oneshot`
//! request (it needs a real authenticator), so the verification itself is covered
//! by `crates/kc-server/src/webauthn.rs` and the store by `web_auth.rs`; what is
//! proved here is the HTTP contract around them.

mod support;

use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use support::{Harness, HarnessBuilder};

async fn harness() -> Harness {
    HarnessBuilder::new(Vec::new()).build().await
}

/// The session cookie from a response, as a `cookie:` request header value.
fn cookie_of(response: &support::TestResponse) -> String {
    response
        .headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("karucode_session="))
        .map(|value| value.split(';').next().unwrap_or_default().to_string())
        .expect("a session cookie")
}

#[tokio::test]
async fn status_is_public_and_reports_a_fresh_install() {
    let h = harness().await;
    let response = h.get("/api/auth/status").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body = response.json();
    assert_eq!(body["enabled"], json!(false));
    assert_eq!(body["password_set"], json!(false));
    assert_eq!(body["passkey_count"], json!(0));
    assert_eq!(body["needs_setup"], json!(true));
    assert_eq!(body["signed_in"], json!(false));
    assert_eq!(body["session_days"], json!(30));
    assert_eq!(body["tunnel"], json!("none"));
    assert_eq!(body["cloudflare_proxies"], json!(false));
    // The wizard shows the floor, so form and server cannot disagree.
    assert_eq!(body["min_password_len"], json!(10));
}

#[tokio::test]
async fn setting_a_password_turns_sign_in_on_and_the_cookie_authenticates() {
    let h = harness().await;
    // Without a credential the API is open (the historical loopback server).
    assert_eq!(h.get("/api/config").await.status, StatusCode::OK);

    let response = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body = response.json();
    assert_eq!(body["enabled_now"], json!(true), "{}", response.text());
    let cookie = cookie_of(&response);

    // Sign-in is now required...
    let denied = h.get("/api/config").await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    assert_eq!(denied.error_code(), "unauthorized");

    // ...and the cookie the password POST handed back works.
    let allowed = h
        .send_with(
            Method::GET,
            "/api/config",
            None,
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.text());
    assert_eq!(allowed.json()["access"]["enabled"], json!(true));

    // The sign-in screen can say the credential exists.
    let status = h.get("/api/auth/status").await;
    assert_eq!(status.json()["password_set"], json!(true));
    assert_eq!(status.json()["enabled"], json!(true));
}

#[tokio::test]
async fn signing_in_again_with_the_password_issues_a_new_cookie() {
    let h = harness().await;
    h.post(
        "/api/auth/password",
        json!({ "password": "correct horse battery" }),
    )
    .await;
    let response = h
        .post(
            "/api/auth/login",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["method"], json!("password"));
    let cookie = cookie_of(&response);
    assert_eq!(
        h.send_with(
            Method::GET,
            "/api/health",
            None,
            &[("cookie", &cookie)],
            true
        )
        .await
        .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_wrong_password_is_refused_and_never_names_the_real_one() {
    let h = harness().await;
    h.post(
        "/api/auth/password",
        json!({ "password": "correct horse battery" }),
    )
    .await;
    let response = h
        .post(
            "/api/auth/login",
            json!({ "password": "wrong horse battery" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.error_code(), "unauthorized");
    assert!(
        !response.text().contains("correct horse battery"),
        "the rejection must not echo a credential"
    );
}

#[tokio::test]
async fn repeated_failures_lock_the_client_out() {
    let h = harness().await;
    h.post(
        "/api/auth/password",
        json!({ "password": "correct horse battery" }),
    )
    .await;
    let mut last = StatusCode::UNAUTHORIZED;
    for _ in 0..12 {
        last = h
            .post("/api/auth/login", json!({ "password": "nope nope nope" }))
            .await
            .status;
        if last == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(
        last,
        StatusCode::TOO_MANY_REQUESTS,
        "a password brute force must be rate limited"
    );
    // Even the right password is refused while the lockout stands: otherwise the
    // limit only slows an attacker down without stopping the guess.
    let correct = h
        .post(
            "/api/auth/login",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    assert_eq!(correct.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn the_bearer_token_still_works_alongside_a_password() {
    let h = HarnessBuilder::new(Vec::new())
        .auth_token("a-32-character-bearer-token-value")
        .build()
        .await;
    // With a token configured, the API is gated from the start.
    assert_eq!(h.get("/api/config").await.status, StatusCode::UNAUTHORIZED);
    let with_token = h
        .send_with(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", "Bearer a-32-character-bearer-token-value")],
            true,
        )
        .await;
    assert_eq!(with_token.status, StatusCode::OK, "{}", with_token.text());

    // Adding a password does not invalidate the token. The password POST itself
    // is a protected route on a token deployment, so it carries the token.
    let response = h
        .send_with(
            Method::POST,
            "/api/auth/password",
            Some(json!({ "password": "correct horse battery" })),
            &[("authorization", "Bearer a-32-character-bearer-token-value")],
            true,
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let with_token = h
        .send_with(
            Method::GET,
            "/api/config",
            None,
            &[("authorization", "Bearer a-32-character-bearer-token-value")],
            true,
        )
        .await;
    assert_eq!(with_token.status, StatusCode::OK);
    assert_eq!(
        h.get("/api/auth/status").await.json()["token_required"],
        json!(true)
    );
}

#[tokio::test]
async fn signing_out_invalidates_the_cookie() {
    let h = harness().await;
    let response = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let cookie = cookie_of(&response);
    let out = h.post("/api/auth/logout", json!({})).await;
    let _ = out;
    // The logout above ran without the cookie (the harness cannot carry one on a
    // fresh POST body), so sign out explicitly with it.
    let out = h
        .send_with(
            Method::POST,
            "/api/auth/logout",
            Some(json!({})),
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(out.status, StatusCode::OK, "{}", out.text());
    assert_eq!(
        h.send_with(
            Method::GET,
            "/api/health",
            None,
            &[("cookie", &cookie)],
            true
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED,
        "a signed-out cookie must stop working"
    );
}

#[tokio::test]
async fn a_password_change_requires_the_current_one_and_signs_browsers_out() {
    let h = harness().await;
    let response = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let cookie = cookie_of(&response);

    let without = h
        .send_with(
            Method::POST,
            "/api/auth/password",
            Some(json!({ "password": "a-new-long-password" })),
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(without.status, StatusCode::FORBIDDEN, "{}", without.text());

    let with = h
        .send_with(
            Method::POST,
            "/api/auth/password",
            Some(json!({
                "password": "a-new-long-password",
                "current_password": "correct horse battery",
            })),
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(with.status, StatusCode::OK, "{}", with.text());
    // The old cookie is dead: a password change is what you do after a leak.
    assert_eq!(
        h.send_with(
            Method::GET,
            "/api/health",
            None,
            &[("cookie", &cookie)],
            true
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    // The new password is the live one.
    let login = h
        .post(
            "/api/auth/login",
            json!({ "password": "a-new-long-password" }),
        )
        .await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
}

#[tokio::test]
async fn removing_the_last_credential_turns_sign_in_off() {
    let h = harness().await;
    let response = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let cookie = cookie_of(&response);
    let removed = h
        .send_with(
            Method::DELETE,
            "/api/auth/password",
            Some(json!({ "current_password": "correct horse battery" })),
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.text());
    assert_eq!(removed.json()["enabled"], json!(false));
    // With no credential left, the UI is not locked behind a screen nobody can
    // pass.
    assert_eq!(h.get("/api/config").await.status, StatusCode::OK);
}

#[tokio::test]
async fn enabling_access_with_no_credential_is_refused() {
    let h = harness().await;
    let response = h
        .patch("/api/config", json!({ "access": { "enabled": true } }))
        .await;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.text()
    );
    let message = response.error_message();
    assert!(message.contains("lock you out"), "{message}");
    assert_eq!(
        h.get("/api/config").await.json()["access"]["enabled"],
        json!(false)
    );
}

#[tokio::test]
async fn the_access_section_round_trips_through_patch_and_get() {
    let h = harness().await;
    let response = h
        .patch(
            "/api/config",
            json!({
                "access": {
                    "public_url": "https://karucode.example.com/",
                    "session_days": 7,
                    "tunnel": "cloudflare",
                    "trusted_proxies": ["10.0.0.0/8", "172.17.0.1"],
                }
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let access = response.json()["access"].clone();
    assert_eq!(access["public_url"], json!("https://karucode.example.com"));
    assert_eq!(access["session_days"], json!(7));
    assert_eq!(access["tunnel"], json!("cloudflare"));
    assert_eq!(access["cloudflare_ranges"], json!(true));
    assert_eq!(access["trusts_forwarded_headers"], json!(true));
    assert_eq!(access["public_authority"], json!("karucode.example.com"));
    assert_eq!(
        access["trusted_proxies"],
        json!(["10.0.0.0/8", "172.17.0.1"])
    );
    // And it survives to the next read, because it is persisted, not in memory.
    let reread = h.get("/api/config").await.json();
    assert_eq!(reread["access"]["tunnel"], json!("cloudflare"));
}

#[tokio::test]
async fn an_invalid_proxy_or_url_is_a_400_naming_the_field() {
    let h = harness().await;
    let bad_proxy = h
        .patch(
            "/api/config",
            json!({ "access": { "trusted_proxies": ["proxy.local"] } }),
        )
        .await;
    assert_eq!(bad_proxy.status, StatusCode::BAD_REQUEST);
    assert!(bad_proxy.error_message().contains("trusted_proxies"));

    let bad_url = h
        .patch(
            "/api/config",
            json!({ "access": { "public_url": "karucode.example.com/ui" } }),
        )
        .await;
    assert_eq!(bad_url.status, StatusCode::BAD_REQUEST);
    assert!(bad_url.error_message().contains("public_url"));

    let bad_tunnel = h
        .patch("/api/config", json!({ "access": { "tunnel": "nginx" } }))
        .await;
    assert_eq!(bad_tunnel.status, StatusCode::BAD_REQUEST);
    assert!(bad_tunnel.error_message().contains("tunnel"));
}

#[tokio::test]
async fn the_configured_public_url_is_accepted_by_the_host_guard() {
    // A loopback bind with no bearer token enforces the Host allowlist. Before
    // `[access] public_url` existed, the only way to add a proxy's name was
    // `KARUCODE_ALLOWED_HOSTS`.
    let h = harness().await;
    let rejected = h
        .send_with(
            Method::GET,
            "/api/health",
            None,
            &[("host", "karucode.example.com")],
            true,
        )
        .await;
    assert_eq!(
        rejected.status,
        StatusCode::FORBIDDEN,
        "{}",
        rejected.text()
    );

    h.patch(
        "/api/config",
        json!({ "access": { "public_url": "https://karucode.example.com" } }),
    )
    .await;

    let allowed = h
        .send_with(
            Method::GET,
            "/api/health",
            None,
            &[("host", "karucode.example.com")],
            true,
        )
        .await;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.text());

    // A look-alike name is still refused.
    let lookalike = h
        .send_with(
            Method::GET,
            "/api/health",
            None,
            &[("host", "karucode.example.com.evil.test")],
            true,
        )
        .await;
    assert_eq!(lookalike.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_public_url_targets_the_session_cookie_at_that_origin() {
    let h = harness().await;
    let response = h
        .patch(
            "/api/config",
            json!({
                "access": {
                    "enabled": true,
                    "public_url": "https://karucode.example.com",
                }
            }),
        )
        .await;
    // Refused: enabling sign-in needs a credential first.
    assert_eq!(response.status, StatusCode::BAD_REQUEST);

    let created = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let cookie = cookie_of(&created);
    // Setting the public URL now marks the cookie `Secure`, because the browser
    // reaches this server over TLS.
    let patched = h
        .send_with(
            Method::PATCH,
            "/api/config",
            Some(json!({
                "access": {
                    "public_url": "https://karucode.example.com",
                }
            })),
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());

    // Signed in *at the public address*: the cookie is `Secure`, because that is
    // how a browser reaches this name.
    let login = h
        .send_with(
            Method::POST,
            "/api/auth/login",
            Some(json!({ "password": "correct horse battery" })),
            &[("host", "karucode.example.com")],
            true,
        )
        .await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.text());
    let set_cookie = login
        .headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("karucode_session="))
        .expect("cookie")
        .to_string();
    assert!(set_cookie.contains("Secure"), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");

    // Signed in on `http://127.0.0.1`, the cookie is **not** `Secure`: a browser
    // drops a Secure cookie on a plain-http origin, so claiming it here would
    // sign the operator out on their own machine.
    let local = h
        .post(
            "/api/auth/login",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let local_cookie = local
        .headers
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("karucode_session="))
        .expect("cookie")
        .to_string();
    assert!(!local_cookie.contains("Secure"), "{local_cookie}");
}

#[tokio::test]
async fn sessions_are_listed_and_can_be_revoked_one_by_one() {
    let h = harness().await;
    let created = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let first = cookie_of(&created);
    let second_login = h
        .post(
            "/api/auth/login",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let second = cookie_of(&second_login);

    let list = h
        .send_with(
            Method::GET,
            "/api/auth/sessions",
            None,
            &[("cookie", &first)],
            true,
        )
        .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.text());
    let sessions = list.json()["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        sessions.len() >= 2,
        "expected two sessions, got {sessions:?}"
    );
    // The ids are short, non-reversible handles - never the cookie itself.
    let ids: Vec<String> = sessions
        .iter()
        .map(|session| session["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(ids.iter().all(|id| id.len() == 12 && !id.contains(&first)));

    let current = list.json()["current"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let other = ids
        .iter()
        .find(|id| **id != current)
        .expect("another session")
        .clone();
    let revoked = h
        .send_with(
            Method::DELETE,
            &format!("/api/auth/sessions/{other}"),
            None,
            &[("cookie", &first)],
            true,
        )
        .await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.text());
    assert_eq!(revoked.json()["removed"], json!(true));
    // One of the two cookies still works and the other does not.
    let first_ok = h
        .send_with(
            Method::GET,
            "/api/auth/me",
            None,
            &[("cookie", &first)],
            true,
        )
        .await;
    let second_ok = h
        .send_with(
            Method::GET,
            "/api/auth/me",
            None,
            &[("cookie", &second)],
            true,
        )
        .await;
    assert_ne!(first_ok.status.is_success(), second_ok.status.is_success());
}

#[tokio::test]
async fn revoking_every_session_signs_this_browser_out_too() {
    let h = harness().await;
    let created = h
        .post(
            "/api/auth/password",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let cookie = cookie_of(&created);
    let response = h
        .send_with(
            Method::POST,
            "/api/auth/sessions/revoke-all",
            Some(json!({})),
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(
        h.send_with(
            Method::GET,
            "/api/health",
            None,
            &[("cookie", &cookie)],
            true
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn passkey_sign_in_is_refused_when_no_passkey_is_registered() {
    let h = harness().await;
    let options = h.post("/api/auth/passkey/options", json!({})).await;
    assert_eq!(options.status, StatusCode::OK, "{}", options.text());
    let body = options.json();
    assert!(!body["challenge"].as_str().unwrap_or_default().is_empty());
    assert_eq!(body["rp_id"], json!("127.0.0.1"));
    assert_eq!(body["allow"], json!([]));

    let finish = h
        .post(
            "/api/auth/passkey/login",
            json!({
                "challenge_id": body["challenge_id"],
                "credential_id": "unknown",
                "client_data_json": "",
                "authenticator_data": "",
                "signature": "",
            }),
        )
        .await;
    assert_eq!(finish.status, StatusCode::UNAUTHORIZED, "{}", finish.text());
    assert!(finish.error_message().contains("not registered"));
}

#[tokio::test]
async fn a_passkey_challenge_cannot_be_replayed() {
    let h = harness().await;
    let options = h.post("/api/auth/passkey/options", json!({})).await.json();
    let challenge_id = options["challenge_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let first = h
        .post(
            "/api/auth/passkey/login",
            json!({
                "challenge_id": challenge_id,
                "credential_id": "unknown",
                "client_data_json": "",
                "authenticator_data": "",
                "signature": "",
            }),
        )
        .await;
    assert_eq!(first.status, StatusCode::UNAUTHORIZED);
    let second = h
        .post(
            "/api/auth/passkey/login",
            json!({
                "challenge_id": challenge_id,
                "credential_id": "unknown",
                "client_data_json": "",
                "authenticator_data": "",
                "signature": "",
            }),
        )
        .await;
    assert_eq!(second.status, StatusCode::BAD_REQUEST);
    assert!(second.error_message().contains("expired"));
}

#[tokio::test]
async fn the_passkey_list_is_empty_and_public_key_free() {
    let h = harness().await;
    let response = h.get("/api/auth/passkeys").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["passkeys"], json!([]));
    assert_eq!(response.json()["password_set"], json!(false));
}

#[tokio::test]
async fn onboarding_is_needed_on_a_fresh_install_and_remembered_after_completion() {
    let h = harness().await;
    let state = h.get("/api/onboarding").await;
    assert_eq!(state.status, StatusCode::OK, "{}", state.text());
    let body = state.json();
    assert_eq!(body["needed"], json!(true), "{}", state.text());
    assert_eq!(body["completed"], json!(false));
    assert_eq!(body["has_credential"], json!(false));

    let done = h
        .post("/api/onboarding/complete", json!({ "skipped": true }))
        .await;
    assert_eq!(done.status, StatusCode::OK, "{}", done.text());
    assert_eq!(done.json()["skipped"], json!(true));

    let again = h.get("/api/onboarding").await.json();
    assert_eq!(again["needed"], json!(false));
    assert_eq!(again["completed"], json!(true));
}

#[tokio::test]
async fn a_configured_provider_means_onboarding_is_not_needed() {
    let h = harness().await;
    h.patch(
        "/api/config",
        json!({ "model": { "api_key": "sk-test-key-value", "model": "test-model" } }),
    )
    .await;
    let body = h.get("/api/onboarding").await.json();
    assert_eq!(body["provider_configured"], json!(true));
    assert_eq!(
        body["needed"],
        json!(false),
        "an install that already has a provider must not be greeted with setup"
    );
}

#[tokio::test]
async fn onboarding_reports_the_access_state_the_wizard_has_to_show() {
    let h = harness().await;
    let body = h.get("/api/onboarding").await.json();
    assert_eq!(body["access"]["enabled"], json!(false));
    assert_eq!(body["access"]["password_set"], json!(false));
    assert_eq!(body["access"]["tunnel"], json!("none"));
    assert_eq!(body["access"]["passkeys"], json!([]));
    assert!(!body["version"].as_str().unwrap_or_default().is_empty());
}

#[tokio::test]
async fn a_signed_out_browser_still_gets_the_shell_and_the_sign_in_route() {
    let h = harness().await;
    h.post(
        "/api/auth/password",
        json!({ "password": "correct horse battery" }),
    )
    .await;
    // The SPA shell has no credential on it and is served anyway - otherwise the
    // sign-in screen could never render.
    let shell = h.get("/").await;
    assert_ne!(shell.status, StatusCode::UNAUTHORIZED);
    // And the status route is public.
    assert_eq!(h.get("/api/auth/status").await.status, StatusCode::OK);
    // Everything else is not.
    assert_eq!(
        h.get("/api/sessions").await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_settings_lock_still_wins_over_an_access_patch() {
    let h = HarnessBuilder::new(Vec::new())
        .lock_settings()
        .build()
        .await;
    let response = h
        .patch("/api/config", json!({ "access": { "session_days": 7 } }))
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_unknown_access_field_is_rejected_rather_than_ignored() {
    let h = harness().await;
    let response = h
        .patch("/api/config", json!({ "access": { "pasword": "typo" } }))
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_login_screen_body_never_contains_a_hash() {
    let h = harness().await;
    h.post(
        "/api/auth/password",
        json!({ "password": "correct horse battery" }),
    )
    .await;
    let status = h.get("/api/auth/status").await;
    let body: Value = status.json();
    let text = serde_json::to_string(&body).unwrap();
    assert!(!text.contains("argon2"), "{text}");
    assert!(!text.contains("correct horse battery"), "{text}");
    // Even a signed-in request never returns key material.
    let login = h
        .post(
            "/api/auth/login",
            json!({ "password": "correct horse battery" }),
        )
        .await;
    let cookie = cookie_of(&login);
    let me = h
        .send_with(
            Method::GET,
            "/api/auth/me",
            None,
            &[("cookie", &cookie)],
            true,
        )
        .await;
    assert_eq!(me.json()["signed_in"], json!(true));
    assert_eq!(me.json()["principal"], json!("password"));
    assert!(!serde_json::to_string(&me.json())
        .unwrap()
        .contains("argon2"));
}
