//! `control_read_session` cuts each message at 4,000 characters, and the marker
//! that announces the cut has to name the call that continues it.
//!
//! This drives the real router, the real runtime and a scripted provider, so what
//! is asserted is the text a model actually receives: the first read announces a
//! `message_id` and an `offset`, and asking for that offset returns the part the
//! first read left out. A marker that named nothing - or an offset that did not
//! line up - would make the audit of a long session impossible, because `limit`
//! only moves between messages.

mod support;

use axum::http::StatusCode;
use kc_core::config::Limits;
use serde_json::json;
use support::{text, tool_call, HarnessBuilder};

/// A window large enough that the fixture's tool result is not elided by
/// compaction: the point here is the marker the *tool* wrote, not the fitter.
fn roomy_limits() -> Limits {
    Limits {
        context_max_chars: 400_000,
        max_tool_output_bytes: 30_000,
        ..Default::default()
    }
}

/// The `(message_id, offset)` a cut marker names, if the text carries one.
///
/// Parsed from the marker itself rather than from the whole page: the system
/// prompt and the tool catalogue also mention `offset=`, and a test that matched
/// those would pass without the marker naming anything.
fn marker(text: &str) -> Option<(i64, usize)> {
    let tail = text
        .split("call control_read_session again with")
        .nth(1)?
        .split(']')
        .next()?;
    let mut message_id = None;
    let mut offset = None;
    for part in tail.split_whitespace() {
        let digits = |value: &str| -> String {
            value
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .to_string()
        };
        if let Some(value) = part.strip_prefix("message_id=") {
            message_id = digits(value).parse().ok().or(message_id);
        }
        if let Some(value) = part.strip_prefix("offset=") {
            offset = digits(value).parse().ok().or(offset);
        }
    }
    Some((message_id?, offset?))
}

/// Every prompt the scripted model has seen from `from` on, joined per call.
///
/// The index matters: the audited session's own run is a provider call too, and it
/// carries the whole long message the fixture is about. Only the control chat's
/// prompts show what the *read tool* returned.
fn seen_from(h: &support::Harness, from: usize) -> String {
    h.factory
        .prompts()
        .into_iter()
        .skip(from)
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

/// The newest request the scripted model was given, joined per message.
fn last_prompt(h: &support::Harness) -> String {
    h.factory
        .prompts()
        .pop()
        .unwrap_or_default()
        .join("\n")
}

#[tokio::test]
async fn a_cut_message_names_the_offset_that_returns_the_rest() {
    let h = HarnessBuilder::new(vec![])
        .no_ai_titles()
        .limits(roomy_limits())
        .build()
        .await;
    let target = h.create_session(Some("Long session")).await;

    // A message well past the 4,000-character preview, with a sentinel only the
    // second page can produce.
    let long = format!("{}{}", "a".repeat(4_400), "TAIL-SENTINEL-OMITTED");
    assert_eq!(
        h.post(
            &format!("/api/sessions/{target}/messages"),
            json!({"content": long}),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&target).await;

    let central = h.post("/api/control/sessions", json!({})).await.json()["session_id"]
        .as_str()
        .expect("control chat")
        .to_owned();
    let send = format!("/api/sessions/{central}/messages");
    let control_calls = h.factory.prompts().len();

    // First read: the newest messages, one of which is too long to fit.
    h.factory.push(tool_call(
        "control_read_session",
        &json!({"session_id": target, "limit": 10}).to_string(),
    ));
    h.factory.push(text("Page one."));
    assert_eq!(
        h.post(&send, json!({"content": "Read that session"})).await.status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&central).await;

    let first = seen_from(&h, control_calls);
    assert!(
        first.contains("[truncated at 4000 characters; call control_read_session again"),
        "the cut has to name its continuation: {first}"
    );
    assert!(
        !first.contains("TAIL-SENTINEL-OMITTED"),
        "the tail is exactly what was cut: {first}"
    );
    let (message_id, offset) = marker(&first).expect("the marker names a message_id and an offset");
    assert_eq!(offset, 4_000, "the page is 4,000 characters long");

    // Follow the marker's own instruction. This is the whole point: `limit` cannot
    // reach into a message, and the offset the marker gave must.
    h.factory.push(tool_call(
        "control_read_session",
        &json!({"session_id": target, "message_id": message_id, "offset": offset}).to_string(),
    ));
    h.factory.push(text("Page two."));
    assert_eq!(
        h.post(&send, json!({"content": "Read the rest of it"})).await.status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&central).await;

    // Only the newest request: the run carries the first page in its history, so a
    // page-wide search would find the older marker as well.
    let second = last_prompt(&h);
    assert!(
        second.contains("TAIL-SENTINEL-OMITTED"),
        "the offset the marker named must return the omitted text: {second}"
    );
    // `next_offset: null` is the tool saying this message is done: a page that
    // reached the end must not ask to be read again, which would loop forever.
    assert!(
        second.contains("\"next_offset\":null"),
        "a page that reaches the end of the message must not ask for another: {second}"
    );
}

#[tokio::test]
async fn an_offset_without_a_message_id_is_refused_with_the_reason() {
    let h = HarnessBuilder::new(vec![])
        .no_ai_titles()
        .limits(roomy_limits())
        .build()
        .await;
    let target = h.create_session(Some("Readable")).await;
    let central = h.post("/api/control/sessions", json!({})).await.json()["session_id"]
        .as_str()
        .expect("control chat")
        .to_owned();

    h.factory.push(tool_call(
        "control_read_session",
        &json!({"session_id": target, "offset": 4000}).to_string(),
    ));
    h.factory.push(text("Understood."));
    assert_eq!(
        h.post(
            &format!("/api/sessions/{central}/messages"),
            json!({"content": "Read from 4000"}),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&central).await;

    let seen = seen_from(&h, 0);
    assert!(
        seen.contains("`offset` continues one message, so it needs `message_id` too"),
        "an offset with nothing to index into must say so: {seen}"
    );
}
