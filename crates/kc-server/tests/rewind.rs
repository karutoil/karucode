//! HTTP tests for the rewind routes: the preview and the perform endpoint, the
//! three modes, and the refusals.
//!
//! These drive the real router and the real runtime, so what is asserted is the
//! JSON a browser actually receives and the transcript a reload actually shows.

mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use support::{text, HarnessBuilder};

/// Every visible message's content, in order.
fn contents(detail: &Value) -> Vec<String> {
    detail["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn user_message_id(detail: &Value, content: &str) -> i64 {
    detail["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["role"] == json!("user") && m["content"] == json!(content))
        .and_then(|m| m["id"].as_i64())
        .unwrap_or_else(|| panic!("no user message `{content}` in {detail}"))
}

/// A session with two completed prompt/answer pairs.
async fn two_turns() -> (support::Harness, String) {
    let h = HarnessBuilder::new(vec![text("first answer"), text("second answer")])
        .build()
        .await;
    let id = h.create_session(Some("rewind")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({"content": "first prompt"}),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
    h.wait_until_idle(&id).await;
    let sent = h
        .post(
            &format!("/api/sessions/{id}/messages"),
            json!({"content": "second prompt"}),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
    h.wait_until_idle(&id).await;
    (h, id)
}

#[tokio::test]
async fn the_preview_describes_the_rewind_without_performing_it() {
    let (h, id) = two_turns().await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let second = user_message_id(&detail, "second prompt");

    let response = h
        .get(&format!("/api/sessions/{id}/rewind?message={second}"))
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body = response.json();
    assert_eq!(body["point"]["prompt"], "second prompt");
    assert_eq!(body["point"]["keep_through"], json!(second));
    assert_eq!(body["point"]["messages_removed"], 1);
    assert_eq!(body["running"], false);
    assert!(
        body["preview"]["files"]
            .as_array()
            .expect("files")
            .is_empty(),
        "a text-only turn changes no files"
    );

    // Nothing happened: the transcript is untouched.
    let after = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(after["messages"], detail["messages"]);
}

#[tokio::test]
async fn the_preview_requires_a_numeric_message_id() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let id = h.create_session(None).await;

    let missing = h.get(&format!("/api/sessions/{id}/rewind")).await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert!(
        missing.error_message().contains("required"),
        "{}",
        missing.text()
    );

    let bad = h
        .get(&format!("/api/sessions/{id}/rewind?message=abc"))
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.error_message().contains("message id"), "{}", bad.text());

    let unknown_session = h.get("/api/sessions/s_nope/rewind?message=1").await;
    assert_eq!(unknown_session.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rewinding_hides_the_tail_and_the_reload_shows_it() {
    let (h, id) = two_turns().await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let second = user_message_id(&detail, "second prompt");

    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": second}),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body = response.json();
    assert_eq!(body["mode"], "none");
    assert_eq!(body["files_restored"], 0);
    assert_eq!(body["files_skipped"], 0);
    assert!(
        body["run_id"].is_null(),
        "a plain rewind does not start a run"
    );

    let reloaded = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(
        contents(&reloaded),
        vec!["first prompt", "first answer", "second prompt"],
        "the chosen prompt survives a plain rewind"
    );
    assert!(
        reloaded["last_rewind_at"].as_i64().unwrap_or(0) > 0,
        "the session reports when it was rewound"
    );
}

#[tokio::test]
async fn retry_hides_the_prompt_and_starts_the_replacement_run() {
    let h = HarnessBuilder::new(vec![text("first attempt"), text("second attempt")])
        .build()
        .await;
    // A titled session is never auto-titled, so the scripted provider is only
    // asked for the turns this test is about (a model title is a second request).
    let id = h.create_session(Some("retry")).await;
    h.post(
        &format!("/api/sessions/{id}/messages"),
        json!({"content": "fix the build"}),
    )
    .await;
    h.wait_until_idle(&id).await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let prompt = user_message_id(&detail, "fix the build");

    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": prompt, "mode": "retry"}),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert!(
        response.json()["run_id"].is_string(),
        "a retry starts a run: {}",
        response.text()
    );
    h.wait_until_idle(&id).await;

    let reloaded = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(
        contents(&reloaded),
        vec!["fix the build", "second attempt"],
        "the retried prompt is asked again, not duplicated"
    );
}

#[tokio::test]
async fn a_note_becomes_the_next_prompt() {
    let h = HarnessBuilder::new(vec![text("first attempt"), text("answered")])
        .build()
        .await;
    // Explicit title: an untitled session would also ask the model for a name,
    // consuming a scripted turn this test is counting on.
    let id = h.create_session(Some("note")).await;
    h.post(
        &format!("/api/sessions/{id}/messages"),
        json!({"content": "add a login page"}),
    )
    .await;
    h.wait_until_idle(&id).await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let prompt = user_message_id(&detail, "add a login page");

    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({
                "message_id": prompt,
                "mode": "note",
                "note": "add a signup page instead",
            }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["mode"], "note");
    h.wait_until_idle(&id).await;

    let reloaded = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(
        contents(&reloaded),
        vec!["add a signup page instead", "answered"],
        "the note replaces the prompt it was written over"
    );
}

#[tokio::test]
async fn a_note_mode_without_a_note_is_refused_and_changes_nothing() {
    let (h, id) = two_turns().await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let second = user_message_id(&detail, "second prompt");

    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": second, "mode": "note"}),
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert!(
        response.error_message().contains("non-empty `note`"),
        "{}",
        response.text()
    );

    // Nothing was hidden: validation runs before the rewind.
    let after = h.get(&format!("/api/sessions/{id}")).await.json();
    assert_eq!(after["messages"], detail["messages"]);
}

#[tokio::test]
async fn an_unknown_mode_and_field_are_refused() {
    let (h, id) = two_turns().await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let second = user_message_id(&detail, "second prompt");

    let bad_mode = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": second, "mode": "undo"}),
        )
        .await;
    assert_eq!(bad_mode.status, StatusCode::BAD_REQUEST);
    assert!(bad_mode
        .error_message()
        .contains("`none`, `retry` or `note`"));

    let bad_field = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": second, "reverts": true}),
        )
        .await;
    assert_eq!(
        bad_field.status,
        StatusCode::BAD_REQUEST,
        "an unknown field must not be silently ignored"
    );
}

#[tokio::test]
async fn a_clean_rewind_emits_a_session_rewound_event() {
    let (h, id) = two_turns().await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let second = user_message_id(&detail, "second prompt");

    let (status, _, body) = h.open_events(&format!("?session={id}"), &[]).await;
    assert_eq!(status, StatusCode::OK);

    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": second}),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());

    let events = support::events_within(body, std::time::Duration::from_secs(2)).await;
    let rewound = events
        .iter()
        .find(|event| event["type"] == json!("session_rewound"))
        .unwrap_or_else(|| panic!("no session_rewound event in {events:?}"));
    assert_eq!(rewound["session_id"], json!(id));
    assert_eq!(rewound["message_id"], json!(second));
    assert_eq!(rewound["messages_removed"], 1);
}

#[tokio::test]
async fn a_committed_rewind_reports_replacement_start_failure_and_one_normalized_event() {
    let h = HarnessBuilder::new(vec![text("original answer")])
        .build()
        .await;
    let id = h.create_session(Some("faulted rewind")).await;
    h.post(
        &format!("/api/sessions/{id}/messages"),
        json!({"content":"original prompt"}),
    )
    .await;
    h.wait_until_idle(&id).await;
    let detail = h.get(&format!("/api/sessions/{id}")).await.json();
    let user = user_message_id(&detail, "original prompt");
    let assistant = detail["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .and_then(|message| message["id"].as_i64())
        .unwrap();
    {
        let connection = rusqlite::Connection::open(h.state.db_path()).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_replacement_prompt
                 BEFORE INSERT ON messages
                 WHEN NEW.content = 'replacement prompt'
                 BEGIN SELECT RAISE(ABORT, 'disk is full'); END;",
            )
            .unwrap();
    }
    let (_, _, events_body) = h.open_events(&format!("?session={id}"), &[]).await;
    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            // Deliberately pick the assistant row: the event must use its parent
            // user-prompt id, not replay the caller's assistant id.
            json!({"message_id":assistant, "mode":"note", "note":"replacement prompt"}),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let outcome = response.json();
    assert_eq!(outcome["committed"], true);
    assert_eq!(outcome["point"]["user_message_id"], user);
    assert!(outcome["run_id"].is_null(), "{outcome}");
    assert!(
        outcome["start_error"]
            .as_str()
            .unwrap_or_default()
            .contains("disk is full"),
        "{outcome}"
    );
    let reloaded = h.get(&format!("/api/sessions/{id}")).await.json();
    assert!(reloaded["messages"].as_array().unwrap().is_empty());

    let events = support::events_within(events_body, std::time::Duration::from_millis(300)).await;
    let rewound: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "session_rewound")
        .collect();
    assert_eq!(
        rewound.len(),
        1,
        "route and runtime must not both emit: {events:?}"
    );
    assert_eq!(rewound[0]["session_id"], id);
    assert_eq!(rewound[0]["message_id"], user);
}

#[tokio::test]
async fn rewinding_a_session_that_is_running_is_a_conflict() {
    use support::Script;
    let h = HarnessBuilder::new(vec![Script::Hang])
        .no_ai_titles()
        .build()
        .await;
    let id = h.create_session(None).await;
    h.post(
        &format!("/api/sessions/{id}/messages"),
        json!({"content": "do something slow"}),
    )
    .await;
    h.wait_until_running(&id).await;

    let response = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": 1}),
        )
        .await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert!(
        response.error_message().contains("already active"),
        "{}",
        response.text()
    );

    let cancelled = h
        .post(&format!("/api/sessions/{id}/cancel"), json!({}))
        .await;
    assert_eq!(cancelled.status, StatusCode::OK);
    h.wait_until_idle(&id).await;
}

#[tokio::test]
async fn a_rewind_needs_a_message_id_and_a_body() {
    let (h, id) = two_turns().await;

    let empty = h
        .post(&format!("/api/sessions/{id}/rewind"), json!({}))
        .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    assert!(
        empty.error_message().contains("message_id"),
        "{}",
        empty.text()
    );

    let unknown = h
        .post(
            &format!("/api/sessions/{id}/rewind"),
            json!({"message_id": 99_999}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}
