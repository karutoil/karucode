//! Per-plugin tool-output budgets over the real router.
//!
//! The properties here are the ones the feature exists for: an unanswered request
//! grants nothing, an approved value is what the runtime applies and is on the
//! record with who/why/when, a refusal falls back to the global default and
//! cannot be re-asked, a value can lower as well as raise, and nothing a plugin
//! or a config file carries is enough on its own.

mod support;

use axum::http::Method;
use serde_json::{json, Value};
use support::{Harness, HarnessBuilder};

use kc_core::config::Limits;

fn plugin(name: &str) -> kc_core::config::PluginConfig {
    kc_core::config::PluginConfig {
        name: name.to_string(),
        enabled: true,
        command: vec!["sh".into(), "-c".into(), "exit 0".into()],
        env: Default::default(),
        permissions: Vec::new(),
        settings: Default::default(),
        tool_output_bytes: None,
    }
}

fn harness(admin: Option<&str>, config: kc_core::config::PluginConfig) -> HarnessBuilder {
    let mut builder = HarnessBuilder::new(vec![support::text("ok")]).plugin(config);
    if let Some(admin) = admin {
        builder = builder.admin_token(admin);
    }
    builder
}

async fn post(h: &Harness, path: &str, body: Value, headers: &[(&str, &str)]) -> (u16, Value) {
    let response = h.send(Method::POST, path, Some(body), headers).await;
    (response.status.as_u16(), response.json())
}

#[tokio::test]
async fn an_unanswered_request_grants_nothing_and_an_answer_is_honoured_and_audited() {
    let h = harness(None, plugin("bundle")).build().await;

    // The ask: 400,000 bytes for a plugin whose tools return whole features.
    let (status, asked) = post(
        &h,
        "/api/plugins/bundle/tool-output/request",
        json!({"bytes": 400_000, "reason": "returns whole-feature bundles"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "{asked}");
    assert_eq!(
        asked["effective_bytes"], 24_000,
        "a request is not a grant: the global default still applies"
    );
    assert_eq!(asked["granted"], false);
    assert_eq!(asked["pending"]["bytes"], 400_000);

    // The listing says the same thing, because it is what the model will see.
    let listed = h.get("/api/plugins").await.json();
    let entry = listed["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "bundle")
        .expect("the plugin is listed");
    assert_eq!(entry["tool_output"]["effective_bytes"], 24_000);
    assert_eq!(entry["tool_output"]["granted"], false);

    // The operator answers, in the browser.
    let (status, decided) = post(
        &h,
        "/api/plugins/bundle/tool-output/decision",
        json!({"decision": "grant"}),
        &[("x-karucode-client", "web")],
    )
    .await;
    assert_eq!(status, 200, "{decided}");
    assert_eq!(decided["effective_bytes"], 400_000);
    assert_eq!(decided["granted"], true);
    let record = &decided["decision"];
    assert_eq!(record["decided_by"], "operator");
    assert_eq!(record["channel"], "web");
    assert_eq!(record["bytes"], 400_000);
    assert_eq!(record["reason"], "returns whole-feature bundles");
    assert!(record["decided_at"].as_i64().unwrap() > 0, "a timestamp");

    // The audit trail: asked, then answered.
    let kinds: Vec<&str> = decided["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["requested", "granted"]);

    // And the listing reflects the grant, with who and when.
    let listed = h.get("/api/plugins").await.json();
    let entry = listed["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "bundle")
        .unwrap();
    assert_eq!(entry["tool_output"]["effective_bytes"], 400_000);
    assert_eq!(entry["tool_output"]["granted"], true);
    assert_eq!(entry["tool_output"]["decided_by"], "operator");
    assert_eq!(entry["tool_output"]["granted_bytes"], 400_000);
}

#[tokio::test]
async fn a_refusal_falls_back_to_the_global_default_and_cannot_be_re_asked() {
    let h = harness(None, plugin("chatter")).build().await;

    let (status, _) = post(
        &h,
        "/api/plugins/chatter/tool-output/request",
        json!({"bytes": 120_000, "reason": "long transcripts"}),
        &[],
    )
    .await;
    assert_eq!(status, 200);

    let (status, refused) = post(
        &h,
        "/api/plugins/chatter/tool-output/decision",
        json!({"decision": "refuse"}),
        &[("x-karucode-client", "web")],
    )
    .await;
    assert_eq!(status, 200, "{refused}");
    assert_eq!(
        refused["effective_bytes"], 24_000,
        "a refusal leaves the global default in force"
    );
    assert_eq!(refused["granted"], false);
    assert_eq!(refused["decision"]["decision"], "refused");

    // One ask is one ask: the same request cannot be filed again.
    let (status, body) = post(
        &h,
        "/api/plugins/chatter/tool-output/request",
        json!({"bytes": 120_000, "reason": "long transcripts"}),
        &[],
    )
    .await;
    assert_eq!(status, 409, "{body}");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("already answered"),
        "the refusal explains itself: {message}"
    );

    // Only the operator can reopen it.
    let response = h
        .send(
            Method::DELETE,
            "/api/plugins/chatter/tool-output",
            None,
            &[("x-karucode-client", "web")],
        )
        .await;
    assert_eq!(response.status.as_u16(), 200);
    let cleared = response.json();
    assert_eq!(cleared["effective_bytes"], 24_000);
    assert!(cleared["decision"].is_null(), "the answer is gone");
    let (status, _) = post(
        &h,
        "/api/plugins/chatter/tool-output/request",
        json!({"bytes": 120_000, "reason": "long transcripts"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "after the operator clears it, asking works");
}

#[tokio::test]
async fn an_operator_answer_needs_the_admin_credential_when_one_is_configured() {
    let h = harness(Some("s3cret"), plugin("guarded")).build().await;
    let (status, _) = post(
        &h,
        "/api/plugins/guarded/tool-output/request",
        json!({"bytes": 200_000, "reason": "documents"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "asking needs no credential");

    let (status, body) = post(
        &h,
        "/api/plugins/guarded/tool-output/decision",
        json!({"decision": "grant"}),
        &[("x-karucode-client", "web")],
    )
    .await;
    assert_eq!(
        status, 403,
        "answering does: a shell cannot grant it on a hardened install: {body}"
    );
    let view = h.get("/api/plugins/guarded/tool-output").await.json();
    assert_eq!(
        view["effective_bytes"], 24_000,
        "the refused attempt changed nothing"
    );

    let (status, granted) = post(
        &h,
        "/api/plugins/guarded/tool-output/decision",
        json!({"decision": "grant"}),
        &[
            ("x-karucode-client", "web"),
            ("x-karucode-admin-token", "s3cret"),
        ],
    )
    .await;
    assert_eq!(status, 200, "{granted}");
    assert_eq!(granted["effective_bytes"], 200_000);
}

#[tokio::test]
async fn nothing_a_plugin_or_a_config_key_carries_is_enough_on_its_own() {
    // The operator's own `[[plugins]] tool_output_bytes`, with no decision on
    // record. It may only *narrow* a grant, so on its own it does nothing - which
    // is what stops an agent from writing a config key instead of asking.
    let mut config = plugin("sneaky");
    config.tool_output_bytes = Some(600_000);
    let h = harness(None, config).build().await;

    let view = h.get("/api/plugins/sneaky/tool-output").await.json();
    assert_eq!(
        view["effective_bytes"], 24_000,
        "a config value alone is not a grant: {view}"
    );
    assert_eq!(view["granted"], false);
    assert_eq!(view["declared_bytes"], 600_000);

    // With a real decision behind it, the same key narrows the grant.
    let (_, _) = post(
        &h,
        "/api/plugins/sneaky/tool-output/request",
        json!({"bytes": 400_000, "reason": "bundles"}),
        &[],
    )
    .await;
    let (_, decided) = post(
        &h,
        "/api/plugins/sneaky/tool-output/decision",
        json!({"decision": "grant", "bytes": 400_000}),
        &[("x-karucode-client", "web")],
    )
    .await;
    assert_eq!(
        decided["effective_bytes"], 400_000,
        "600,000 declared cannot widen a 400,000 grant: {decided}"
    );
}

#[tokio::test]
async fn a_value_outside_the_accepted_range_is_rejected() {
    let limits = Limits {
        max_plugin_tool_output_bytes: 100_000,
        ..Default::default()
    };
    let h = harness(None, plugin("bounded"))
        .limits(limits)
        .build()
        .await;

    let (status, body) = post(
        &h,
        "/api/plugins/bounded/tool-output/request",
        json!({"bytes": 500, "reason": "tiny"}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("2000"));

    let (status, body) = post(
        &h,
        "/api/plugins/bounded/tool-output/request",
        json!({"bytes": 400_000, "reason": "over the ceiling"}),
        &[],
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("100000"),
        "it names the ceiling: {message}"
    );

    // And nothing was filed by either attempt.
    let view = h.get("/api/plugins/bounded/tool-output").await.json();
    assert!(view["pending"].is_null(), "{view}");
}

#[tokio::test]
async fn a_grant_can_lower_the_budget_as_well_as_raise_it() {
    let h = harness(None, plugin("quiet")).build().await;
    let (_, _) = post(
        &h,
        "/api/plugins/quiet/tool-output/request",
        json!({"bytes": 4_000, "reason": "the summary is all the model needs"}),
        &[],
    )
    .await;
    let (status, decided) = post(
        &h,
        "/api/plugins/quiet/tool-output/decision",
        json!({"decision": "grant", "bytes": 4_000}),
        &[("x-karucode-client", "web")],
    )
    .await;
    assert_eq!(status, 200, "{decided}");
    assert_eq!(
        decided["effective_bytes"], 4_000,
        "a grant below the global default lowers the ceiling"
    );
}

#[tokio::test]
async fn the_silence_that_runs_out_a_wait_can_only_ever_refuse() {
    let h = harness(None, plugin("patient")).build().await;
    let (_, _) = post(
        &h,
        "/api/plugins/patient/tool-output/request",
        json!({"bytes": 400_000, "reason": "bundles"}),
        &[],
    )
    .await;

    // The CLI records the timeout as the answer it is. Even if a caller sends
    // `grant` with that channel, the service refuses: a wait nobody answered is
    // never a yes.
    let (status, decided) = post(
        &h,
        "/api/plugins/patient/tool-output/decision",
        json!({"decision": "grant", "channel": "timeout"}),
        &[],
    )
    .await;
    assert_eq!(status, 200, "{decided}");
    assert_eq!(decided["effective_bytes"], 24_000);
    assert_eq!(decided["decision"]["decision"], "refused");
    assert_eq!(decided["decision"]["decided_by"], "no answer");
}

#[tokio::test]
async fn an_unknown_plugin_is_a_404_rather_than_a_silent_no_op() {
    let h = harness(None, plugin("real")).build().await;
    let response = h.get("/api/plugins/imaginary/tool-output").await;
    assert_eq!(response.status.as_u16(), 404);
    let (status, _) = post(
        &h,
        "/api/plugins/imaginary/tool-output/request",
        json!({"bytes": 100_000, "reason": "typo"}),
        &[],
    )
    .await;
    assert_eq!(status, 404);
}
