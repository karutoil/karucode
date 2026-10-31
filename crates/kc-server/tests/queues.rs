//! HTTP tests for the queue routes: queue with attachments, list, recall, and the
//! `queued_messages` frame.
//!
//! These drive the real router and the real runtime, so what is asserted is the
//! JSON a browser actually receives - including that a recalled message comes
//! back with the chips it was queued with, which is the whole point of the
//! feature.

mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;
use support::{events_within, text, HarnessBuilder, Script};

/// The attachment a browser sends for "summarise this file".
fn attachment() -> Value {
    json!({
        "name": "notes.md",
        "path": "notes.md",
        "mode": "inline",
        "size": 812,
        "content": "# Note\n",
    })
}

/// A steer body: the rendered content the model will see, the draft to restore,
/// and the chip to redraw.
fn steer_body() -> Value {
    json!({
        "content": "summarise\n\nAttached 1 file:\n\n### notes.md (inlined, 812 B)\n\n```md\n# Note\n```",
        "draft": "summarise",
        "attachments": [attachment()],
    })
}

/// Start a run that will not finish on its own, so the queue has something to
/// queue against.
async fn running_session() -> (support::Harness, String) {
    let h = HarnessBuilder::new(vec![Script::Hang]).build().await;
    let id = h.create_session(Some("queues")).await;
    let started = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "long job" }),
        )
        .await;
    assert_eq!(started.status, StatusCode::ACCEPTED, "{}", started.text());
    h.wait_until_running(&id).await;
    (h, id)
}

/// A steer queued with an attachment comes back whole from both the session detail
/// and the queues route, and the recall returns it with its draft and chip.
#[tokio::test]
async fn a_queued_steer_keeps_its_attachments_and_can_be_recalled() {
    let (h, id) = running_session().await;

    let queued = h
        .post(
            &format!("/api/sessions/{id}/messages?mode=steer"),
            steer_body(),
        )
        .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.text());
    let body = queued.json();
    assert_eq!(body["mode"], "steer");
    assert_eq!(body["queued"], true);
    let steer_id = body["steer_id"].as_str().expect("steer id").to_string();
    // The acceptance answers with the entry itself, so the panel can draw it
    // without waiting for the event frame.
    assert_eq!(body["queued_message"]["id"], steer_id);
    assert_eq!(body["queued_message"]["draft"], "summarise");
    assert_eq!(body["queued_message"]["attachments"][0]["name"], "notes.md");

    // A reload sees the same message: the session detail carries it.
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let steers = detail["queued_steers"].as_array().expect("steer array");
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0]["id"], steer_id);
    assert_eq!(steers[0]["draft"], "summarise");
    assert_eq!(steers[0]["attachments"][0]["mode"], "inline");
    assert!(steers[0]["content"]
        .as_str()
        .unwrap()
        .contains("### notes.md (inlined, 812 B)"));

    // …and so does the cheap route that carries no transcript.
    let queues = h.get(&format!("/api/sessions/{id}/queues")).await;
    assert_eq!(queues.status, StatusCode::OK);
    let queues = queues.json();
    assert_eq!(queues["session_id"], id);
    assert_eq!(queues["running"], json!(true));
    assert_eq!(queues["steers"][0]["id"], steer_id);
    assert_eq!(queues["steers"][0]["attachments"][0]["path"], "notes.md");
    assert_eq!(queues["follow_ups"], json!([]));

    // Recall: the message is taken out of the queue and handed back.
    let recalled = h
        .delete(&format!("/api/sessions/{id}/queues/{steer_id}"))
        .await;
    assert_eq!(recalled.status, StatusCode::OK, "{}", recalled.text());
    let recalled = recalled.json();
    assert_eq!(recalled["session_id"], id);
    assert_eq!(recalled["recalled"]["id"], steer_id);
    assert_eq!(recalled["recalled"]["draft"], "summarise");
    assert_eq!(recalled["recalled"]["attachments"][0]["name"], "notes.md");
    assert_eq!(recalled["recalled"]["attachments"][0]["size"], 812);
    // The inlined body comes back too: the browser read it from a local file the
    // server never saw, so this is the only copy a recall can re-inline.
    assert_eq!(
        recalled["recalled"]["attachments"][0]["content"],
        "# Note\n"
    );

    // The queue is empty afterwards, on both routes.
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(detail["queued_steers"], json!([]));
    let queues = h.get(&format!("/api/sessions/{id}/queues")).await.json();
    assert_eq!(queues["steers"], json!([]));

    // Nothing reached the conversation: the recall happened before the loop's
    // next iteration could fold it in.
    let users: Vec<String> = detail["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|m| m["role"] == json!("user"))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !users.iter().any(|text| text.contains("summarise")),
        "the recalled steer must not be persisted: {users:?}"
    );

    h.post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
}

/// A follow-up is queued and recalled the same way, out of its own queue.
#[tokio::test]
async fn a_queued_follow_up_can_be_recalled() {
    let (h, id) = running_session().await;

    let queued = h
        .post(
            &format!("/api/sessions/{id}/messages?mode=follow_up"),
            steer_body(),
        )
        .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.text());
    let body = queued.json();
    assert_eq!(body["mode"], "follow_up");
    let follow_up_id = body["follow_up_id"]
        .as_str()
        .expect("follow-up id")
        .to_string();

    let queues = h.get(&format!("/api/sessions/{id}/queues")).await.json();
    assert_eq!(queues["follow_ups"][0]["id"], follow_up_id);
    assert_eq!(
        queues["follow_ups"][0]["attachments"][0]["name"],
        "notes.md"
    );
    assert_eq!(queues["steers"], json!([]));

    let recalled = h
        .delete(&format!("/api/sessions/{id}/queues/{follow_up_id}"))
        .await;
    assert_eq!(recalled.status, StatusCode::OK, "{}", recalled.text());
    assert_eq!(recalled.json()["recalled"]["draft"], "summarise");
    assert_eq!(
        h.get(&format!("/api/sessions/{id}/queues")).await.json()["follow_ups"],
        json!([])
    );

    h.post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
}

/// Recalling something that is not queued is the documented 404, not a silent
/// success: the browser raced the run loop and has to be told.
#[tokio::test]
async fn recalling_an_unknown_message_is_a_404() {
    let (h, id) = running_session().await;

    let missing = h
        .delete(&format!("/api/sessions/{id}/queues/steer_nope"))
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text());
    assert_eq!(missing.error_code(), "not_found");
    assert!(
        missing.error_message().contains("steer_nope"),
        "the error must name the id: {}",
        missing.error_message()
    );

    // An unknown session is a 404 too, and does not leak whether the queue
    // entry exists.
    let unknown_session = h.delete("/api/sessions/s_nope/queues/steer_nope").await;
    assert_eq!(unknown_session.status, StatusCode::NOT_FOUND);

    h.post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
}

/// Attachments are validated before anything is queued: an unknown mode or a
/// nameless chip is a `400 bad_request`, and nothing appears in the queue.
#[tokio::test]
async fn a_malformed_attachment_is_rejected() {
    let (h, id) = running_session().await;

    for (attachments, expected) in [
        (
            json!([{ "name": "notes.md", "path": "notes.md", "mode": "inline-broken" }]),
            "must be `inline` or `upload`",
        ),
        (
            json!([{ "name": "", "path": "notes.md" }]),
            "non-empty `name`",
        ),
        (
            json!([{ "name": "notes.md", "path": " " }]),
            "non-empty `path`",
        ),
        // An upload lives on disk; a body in the request is either a mistake or
        // an attempt to park bytes in the queue.
        (
            json!([{ "name": "shot.png", "path": "a/shot.png", "mode": "upload", "content": "x" }]),
            "may not carry `content`",
        ),
    ] {
        let response = h
            .post(
                &format!("/api/sessions/{id}/messages?mode=steer"),
                json!({ "content": "hi", "attachments": attachments }),
            )
            .await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{}: {}",
            attachments,
            response.text()
        );
        assert_eq!(response.error_code(), "bad_request");
        assert!(
            response.error_message().contains(expected),
            "{}",
            response.error_message()
        );
    }
    assert_eq!(
        h.get(&format!("/api/sessions/{id}/queues")).await.json()["steers"],
        json!([])
    );

    // A plain send accepts the same shape and ignores it: a client that always
    // sends its chips must not fail a send.
    h.post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
    h.wait_until_idle(&id).await;
    let sent = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({ "content": "done", "attachments": [attachment()] }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
}

/// Every queue mutation is broadcast with the whole queue, so a second tab (or a
/// reconnecting one) never shows a stale pending panel.
#[tokio::test]
async fn queueing_and_recalling_publish_a_queued_messages_frame() {
    let (h, id) = running_session().await;
    let (status, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    assert_eq!(status, StatusCode::OK);

    let queued = h
        .post(
            &format!("/api/sessions/{id}/messages?mode=steer"),
            steer_body(),
        )
        .await;
    assert_eq!(queued.status, StatusCode::ACCEPTED, "{}", queued.text());
    let steer_id = queued.json()["steer_id"].as_str().unwrap().to_string();

    let events = events_within(body, Duration::from_millis(600)).await;
    let added = events
        .iter()
        .find(|event| event["type"] == "queued_messages")
        .unwrap_or_else(|| panic!("no queued_messages frame in {events:#?}"));
    assert_eq!(added["session_id"], id);
    assert_eq!(added["steers"][0]["id"], steer_id);
    assert_eq!(added["steers"][0]["draft"], "summarise");
    assert_eq!(added["steers"][0]["attachments"][0]["name"], "notes.md");
    assert_eq!(added["follow_ups"], json!([]));

    // …and the recall is broadcast too, with the queue now empty.
    let (_, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    let recalled = h
        .delete(&format!("/api/sessions/{id}/queues/{steer_id}"))
        .await;
    assert_eq!(recalled.status, StatusCode::OK, "{}", recalled.text());
    let events = events_within(body, Duration::from_millis(600)).await;
    let removed = events
        .iter()
        .find(|event| event["type"] == "queued_messages")
        .unwrap_or_else(|| panic!("no queued_messages frame in {events:#?}"));
    assert_eq!(removed["steers"], json!([]));

    h.post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
}

/// The queues route answers for a session with nothing queued, and for a session
/// that is not running: empty lists, not a 404 and not a missing key.
#[tokio::test]
async fn a_settled_session_has_empty_queues() {
    let h = HarnessBuilder::new(vec![text("done")]).build().await;
    let id = h.create_session(None).await;

    let queues = h.get(&format!("/api/sessions/{id}/queues")).await;
    assert_eq!(queues.status, StatusCode::OK);
    let queues = queues.json();
    assert_eq!(queues["running"], json!(false));
    assert_eq!(queues["steers"], json!([]));
    assert_eq!(queues["follow_ups"], json!([]));
}
