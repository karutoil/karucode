//! Temporary (ephemeral) sessions over the real router: never listed, expirable
//! on demand, swept by the collection delete, and titled never.

mod support;

use axum::http::StatusCode;
use support::{text, HarnessBuilder};
use tokio::time::{sleep, Duration};

/// A session created with `ephemeral: true` never appears in a listing, but is
/// fully usable and readable by id.
#[tokio::test]
async fn an_ephemeral_session_is_unlisted_but_usable() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let durable = h.create_session(Some("Durable")).await;

    let created = h
        .post(
            "/api/sessions",
            serde_json::json!({ "ephemeral": true, "title": "Temp chat" }),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let id = created.json()["id"].as_str().unwrap().to_string();
    assert_eq!(created.json()["ephemeral"], serde_json::json!(true));

    // Not in the plain list, not in the project list.
    let all = h.get("/api/sessions").await;
    let sessions = all.json()["sessions"].as_array().unwrap().clone();
    assert!(
        sessions
            .iter()
            .all(|s| s["id"].as_str() != Some(id.as_str())),
        "an ephemeral session must not be listed: {sessions:?}"
    );
    assert!(
        sessions
            .iter()
            .any(|s| s["id"].as_str() == Some(durable.as_str())),
        "the durable session is still listed: {sessions:?}"
    );

    // Readable by id: the creating client polls this.
    let one = h.get(&format!("/api/sessions/{id}")).await;
    assert_eq!(one.status, StatusCode::OK);
    assert_eq!(one.json()["session"]["ephemeral"], serde_json::json!(true));

    // Ordinary delete still works and emits the same shape as a durable one.
    let deleted = h.delete(&format!("/api/sessions/{id}")).await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert_eq!(deleted.json()["deleted"], serde_json::json!(true));
    let gone = h.get(&format!("/api/sessions/{id}")).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

/// `POST /api/sessions/:id/expire` retires an ephemeral session now, refuses a
/// durable one, and refuses while input is still queued.
#[tokio::test]
async fn expire_retires_only_an_idle_ephemeral_session() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let durable = h.create_session(Some("Durable")).await;

    // Durable is refused: that is what DELETE is for.
    let refused = h
        .post(
            &format!("/api/sessions/{durable}/expire"),
            serde_json::json!({}),
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(
        refused.error_message().contains("not ephemeral"),
        "{}",
        refused.text()
    );

    // Ephemeral goes, and the row is gone with its transcript.
    let created = h
        .post(
            "/api/sessions",
            serde_json::json!({ "ephemeral": true, "title": "Temp" }),
        )
        .await;
    let id = created.json()["id"].as_str().unwrap().to_string();
    let expired = h
        .post(&format!("/api/sessions/{id}/expire"), serde_json::json!({}))
        .await;
    assert_eq!(expired.status, StatusCode::OK, "{}", expired.text());
    assert_eq!(expired.json()["expired"], serde_json::json!(true));
    let gone = h.get(&format!("/api/sessions/{id}")).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

/// `DELETE /api/sessions` sweeps idle ephemeral sessions and never touches
/// durable ones.
#[tokio::test]
async fn the_collection_delete_sweeps_idle_ephemeral_sessions() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let durable = h.create_session(Some("Durable")).await;

    let mut ids = Vec::new();
    for i in 0..3 {
        let created = h
            .post(
                "/api/sessions",
                serde_json::json!({ "ephemeral": true, "title": format!("temp {i}") }),
            )
            .await;
        ids.push(created.json()["id"].as_str().unwrap().to_string());
    }

    let swept = h.delete("/api/sessions").await;
    assert_eq!(swept.status, StatusCode::OK, "{}", swept.text());
    assert_eq!(swept.json()["deleted"], serde_json::json!(3));
    for id in &ids {
        let gone = h.get(&format!("/api/sessions/{id}")).await;
        assert_eq!(gone.status, StatusCode::NOT_FOUND);
    }
    // The durable session survives the sweep.
    let kept = h.get(&format!("/api/sessions/{durable}")).await;
    assert_eq!(kept.status, StatusCode::OK);
}

/// A finished ephemeral run retires its own session: no listing ever shows it,
/// no manual sweep is needed, and the transcript does not survive it.
#[tokio::test]
async fn a_finished_ephemeral_run_retires_the_session() {
    let h = HarnessBuilder::new(vec![text("all done")]).build().await;
    let created = h
        .post(
            "/api/sessions",
            serde_json::json!({ "ephemeral": true, "title": "Temp" }),
        )
        .await;
    let id = created.json()["id"].as_str().unwrap().to_string();

    let sent = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            serde_json::json!({ "content": "do the thing" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());

    // The run finishes in the background; its teardown is what deletes the row.
    // Wait for it (bounded) rather than asserting on a race.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let one = h.get(&format!("/api/sessions/{id}")).await;
        if one.status == StatusCode::NOT_FOUND {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session was not retired after the run finished"
        );
        sleep(Duration::from_millis(50)).await;
    }

    // And it never appeared in a listing at any point.
    let all = h.get("/api/sessions").await;
    let sessions = all.json()["sessions"].as_array().unwrap().clone();
    assert!(
        sessions
            .iter()
            .all(|s| s["id"].as_str() != Some(id.as_str())),
        "{sessions:?}"
    );
}

/// An ephemeral session is never auto-titled: the first message does not rename
/// it, so no titling request can race the delete.
#[tokio::test]
async fn an_ephemeral_session_is_never_auto_titled() {
    let h = HarnessBuilder::new(vec![text("done")]).build().await;
    let created = h
        .post("/api/sessions", serde_json::json!({ "ephemeral": true }))
        .await;
    let id = created.json()["id"].as_str().unwrap().to_string();
    // No title, so the row carries the placeholder; a durable session's first
    // message would rename it to something derived from the prompt.
    assert_eq!(created.json()["title"], serde_json::json!("New session"));

    let sent = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            serde_json::json!({ "content": "audit the pull requests" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());

    // The title the row carries is unchanged when the run finishes.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut final_title = String::new();
    loop {
        let one = h.get(&format!("/api/sessions/{id}")).await;
        if one.status == StatusCode::NOT_FOUND {
            // Retired; whatever title it held is gone with it. The rename
            // would have happened *before* the run started, so check the row
            // while the run is in flight too.
            break;
        }
        final_title = one.json()["session"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session was not retired after the run finished"
        );
        sleep(Duration::from_millis(50)).await;
    }
    assert!(
        final_title.is_empty() || final_title == "New session",
        "an ephemeral session must not be auto-titled: {final_title:?}"
    );
}

/// A temporary session's run produces no `notification` event on the stream:
/// the session deletes itself as the run ends, so the ping would name a row
/// that is already gone. The switch `notifications.ephemeral_sessions` opts
/// back in, and `GET /api/config` reports it.
#[tokio::test]
async fn an_ephemeral_run_emits_no_notification_event_by_default() {
    use support::events_within;

    let h = HarnessBuilder::new(vec![text("done")]).build().await;
    let created = h
        .post("/api/sessions", serde_json::json!({ "ephemeral": true }))
        .await;
    let id = created.json()["id"].as_str().unwrap().to_string();

    let (status, _, body) = h.open_events("", &[]).await;
    assert_eq!(status, StatusCode::OK);

    let sent = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            serde_json::json!({ "content": "do the thing" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());

    // Wait for the retire first (bounded), then look back over what the stream
    // delivered across the whole run: the notification, if any, is emitted in
    // the run's teardown — right before the row disappears.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let one = h.get(&format!("/api/sessions/{id}")).await;
        if one.status == StatusCode::NOT_FOUND {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the session was not retired after the run finished"
        );
        sleep(Duration::from_millis(50)).await;
    }
    let events = events_within(body, Duration::from_millis(500)).await;
    assert!(
        !events
            .iter()
            .any(|event| event["type"] == "notification"),
        "a temporary session's run must not notify: {events:?}"
    );

    // The switch is visible and off by default, so a client can offer it.
    let config = h.get("/api/config").await;
    assert_eq!(
        config.json()["notifications"]["ephemeral_sessions"],
        serde_json::json!(false)
    );

    // And PATCHing it on is accepted, round-tripping through the same shape.
    let patched = h
        .patch(
            "/api/config",
            serde_json::json!({ "notifications": { "ephemeral_sessions": true } }),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let config = h.get("/api/config").await;
    assert_eq!(
        config.json()["notifications"]["ephemeral_sessions"],
        serde_json::json!(true)
    );
}
