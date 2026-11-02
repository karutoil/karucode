//! `GET /api/sessions/:id/info` — the numbers behind the inspector's Session tab.
//!
//! Driven through the real router and a scripted provider, so the token totals
//! it asserts are the ones the runtime actually persisted rather than a fixture.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use support::{text, tool_call, HarnessBuilder};

/// A run that answers, plus a second turn that calls a tool: enough for
/// messages-by-role, tool-call counts, a prompt-cache figure and working time.
#[tokio::test]
async fn session_info_summarises_the_whole_session() {
    let h = HarnessBuilder::new(vec![
        // Turn 1: plain answer.
        text("first answer"),
        // Turn 2: one tool call, then the answer that follows it.
        tool_call("read_file", r#"{"path":"a.txt"}"#),
        text("second answer"),
    ])
    .build()
    .await;

    let session = h.create_session(Some("Summarise me")).await;

    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": "hello" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
    h.wait_until_idle(&session).await;

    // A file exists so `read_file` succeeds rather than erroring, which would
    // still count as a call but make the test read oddly.
    std::fs::write(h.workspace_path().join("a.txt"), "contents").expect("write fixture");
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": "now use a tool" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
    h.wait_until_idle(&session).await;

    let response = h.get(&format!("/api/sessions/{session}/info")).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body = response.json();

    // ---- identity ------------------------------------------------------
    assert_eq!(body["session"]["id"], json!(session));
    assert_eq!(body["session"]["title"], "Summarise me");
    assert_eq!(body["running"], json!(false));

    // ---- messages ------------------------------------------------------
    // Two prompts, three assistant turns and one tool result. The turn that
    // called a tool persists a row of its own (the call lives on that row, not
    // on a message), then a second row carries the answer that follows the
    // result - which is what makes the transcript render in order.
    assert_eq!(body["messages"]["by_role"]["user"], json!(2));
    assert_eq!(body["messages"]["by_role"]["assistant"], json!(3));
    assert_eq!(body["messages"]["by_role"]["tool"], json!(1));
    assert_eq!(body["messages"]["total"], json!(6));
    assert_eq!(body["messages"]["main"], json!(6));
    assert_eq!(
        body["messages"]["subagent"],
        json!(0),
        "no subagent ran in this session"
    );

    // ---- tool calls ----------------------------------------------------
    assert_eq!(body["tools"]["total"], json!(1));
    let names = body["tools"]["by_name"].as_array().expect("by_name");
    assert_eq!(names.len(), 1, "{names:?}");
    assert_eq!(names[0]["name"], "read_file");
    assert_eq!(names[0]["count"], json!(1));
    assert_eq!(body["tools"]["truncated"], json!(false));

    // ---- runs and working time -----------------------------------------
    assert_eq!(body["runs"]["total"], json!(2));
    assert_eq!(body["runs"]["by_status"]["completed"], json!(2));
    // Both runs finished, so the sum is real wall clock (possibly 0 ms for two
    // very fast runs - the assertion is that it is a number, not that it is big).
    assert!(body["runs"]["working_ms"].is_u64());
    let recent = body["runs"]["recent"].as_array().expect("recent");
    assert_eq!(recent.len(), 2, "both runs are listed, newest first");
    assert_eq!(recent[0]["status"], "completed");

    // ---- usage ---------------------------------------------------------
    // The scripted provider reports usage on every call, so the totals exist
    // and the two model calls are counted.
    let main = &body["usage"]["main"];
    assert!(
        main["calls"].as_u64().unwrap_or(0) >= 2,
        "at least one recorded call per turn: {main}"
    );
    assert!(main["input_tokens"].as_u64().unwrap_or(0) > 0);
    assert!(
        body["usage"]["total"]["calls"].as_u64().unwrap_or(0) >= 2,
        "the session total includes the main agent"
    );
    // The newest prompt is what fills the window, and it is never a sum.
    let used = main["last_input_tokens"].as_u64().unwrap_or(0);
    assert!(used > 0);
    assert!(
        used <= main["input_tokens"].as_u64().unwrap(),
        "the window fill cannot exceed every token the session ever sent"
    );

    // ---- context -------------------------------------------------------
    assert_eq!(body["context"]["model"], "test-model");
    // The harness model declares no `context_window`, so the honest answer is
    // `null` - the UI must hide the percentage rather than invent a denominator.
    assert!(body["context"]["window"].is_null(), "{}", body["context"]);

    // ---- agents --------------------------------------------------------
    assert_eq!(
        body["agents"].as_array().map(Vec::len),
        Some(0),
        "no subagent ran"
    );
}

/// A subagent's work must be attributed to it, not folded into the main agent -
/// otherwise "how much of the cost was delegation" has no answer.
#[tokio::test]
async fn session_info_separates_subagent_work_from_the_main_agent() {
    let h = HarnessBuilder::new(vec![
        // The parent delegates on its first turn.
        tool_call(
            "spawn_agent",
            r#"{"task":"review the diff","name":"reviewer","role":"reviewer"}"#,
        ),
        // The child answers.
        text("the diff looks fine"),
        // The parent reports back.
        text("all done"),
    ])
    .build()
    .await;

    let session = h.create_session(Some("Delegate")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": "delegate the review" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
    h.wait_until_idle(&session).await;

    let body = h.get(&format!("/api/sessions/{session}/info")).await.json();

    let agents = body["agents"].as_array().expect("agents");
    assert_eq!(agents.len(), 1, "one subagent: {agents:?}");
    let agent = &agents[0];
    assert_eq!(agent["name"], "reviewer");
    assert_eq!(agent["status"], "completed");
    assert_eq!(agent["model"], "test-model");
    assert!(
        agent["messages"].as_u64().unwrap_or(0) >= 1,
        "the child's own task and answer are counted: {agent}"
    );
    assert!(
        agent["duration_ms"].is_i64(),
        "a finished agent reports how long it took"
    );
    assert!(
        agent["usage"]["calls"].as_u64().unwrap_or(0) >= 1,
        "the child's provider call is attributed to it: {agent}"
    );
    assert!(agent["iterations"].as_u64().unwrap_or(0) >= 1);

    // The session split keeps the two apart.
    assert!(
        body["messages"]["subagent"].as_u64().unwrap_or(0) > 0,
        "{}",
        body["messages"]
    );
    let main_calls = body["usage"]["main"]["calls"].as_u64().unwrap_or(0);
    let child_calls = agent["usage"]["calls"].as_u64().unwrap_or(0);
    assert_eq!(
        body["usage"]["total"]["calls"].as_u64().unwrap_or(0),
        main_calls + child_calls,
        "the total is exactly main plus every agent"
    );
    // The window fill is the main agent's own newest prompt, never a subagent's:
    // a child's context is a separate and much smaller window.
    assert_eq!(
        body["context"]["used"], body["usage"]["main"]["last_input_tokens"],
        "the window fill is the main agent's newest prompt"
    );
}

/// A session that never ran still answers, with zeros rather than an error.
#[tokio::test]
async fn session_info_of_an_empty_session_is_all_zeros() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = h.create_session(Some("Empty")).await;

    let response = h.get(&format!("/api/sessions/{session}/info")).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body = response.json();
    assert_eq!(body["messages"]["total"], json!(0));
    assert_eq!(body["tools"]["total"], json!(0));
    assert_eq!(body["runs"]["total"], json!(0));
    assert_eq!(body["runs"]["working_ms"], json!(0));
    assert_eq!(body["usage"]["main"]["calls"], json!(0));
    assert_eq!(body["usage"]["main"]["input_tokens"], json!(0));
    assert_eq!(body["usage"]["main"]["cached_tokens"], json!(0));
    assert!(
        body["usage"]["main"]["cache_hit_ratio"].is_null(),
        "no call reported a cache figure, so there is no ratio to show"
    );
    assert_eq!(body["usage"]["main"]["last_input_tokens"], json!(0));
    assert_eq!(body["agents"], json!([]));
    assert!(body["tools"]["by_name"]
        .as_array()
        .expect("list")
        .is_empty());
}

/// An unknown session is `404 not_found`, not a page of zeros.
#[tokio::test]
async fn session_info_of_a_missing_session_is_not_found() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let response = h.get("/api/sessions/s_missing/info").await;
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "{}",
        response.text()
    );
    assert_eq!(response.error_code(), "not_found");
}

/// A rewind hides messages, so the counts drop with the transcript instead of
/// reporting work the session no longer contains.
#[tokio::test]
async fn session_info_counts_only_visible_messages() {
    let h = HarnessBuilder::new(vec![text("first"), text("second")])
        .build()
        .await;
    let session = h.create_session(Some("Rewind me")).await;
    for prompt in ["one", "two"] {
        let sent = h
            .post(
                &format!("/api/sessions/{session}/messages"),
                json!({ "content": prompt }),
            )
            .await;
        assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
        h.wait_until_idle(&session).await;
    }
    let before = h.get(&format!("/api/sessions/{session}/info")).await.json();
    let total_before = before["messages"]["total"].as_u64().expect("total");
    assert_eq!(total_before, 4, "two prompts, two answers");

    // Rewind to the second prompt: its answer and one turn's messages go.
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    let prompts: Vec<i64> = detail["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(prompts.len(), 2);
    let rewound = h
        .post(
            &format!("/api/sessions/{session}/rewind"),
            json!({ "message_id": prompts[1], "mode": "none", "keep_prompt": false }),
        )
        .await;
    assert_eq!(rewound.status, StatusCode::OK, "{}", rewound.text());

    let after = h.get(&format!("/api/sessions/{session}/info")).await.json();
    let total_after = after["messages"]["total"].as_u64().expect("total");
    assert!(
        total_after < total_before,
        "hidden rows must not be counted: {total_before} -> {total_after}"
    );
    assert_eq!(
        after["messages"]["main_by_role"]["user"],
        json!(1),
        "only the surviving prompt is counted"
    );
}
