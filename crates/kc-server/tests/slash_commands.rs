//! Slash commands: the catalog, the interception, and what the model does *not*
//! see.
//!
//! These drive the real router and runtime. Three things are worth asserting and
//! nothing else is:
//!
//! * `GET /api/commands` lists the built-ins plus the workspace's skills, with the
//!   `insert` text the composer writes;
//! * posting a command answers in place - a `command_result` frame, a persisted
//!   row named `slash:<command>`, and **no run**;
//! * that row is kept out of the next request, because it is an answer for the
//!   reader and not a turn the model ever made.

mod support;

use axum::http::StatusCode;
use kc_core::config::Limits;
use kc_core::store::NewMessage;
use serde_json::{json, Value};
use support::{collect_events_until, store_of, text, Harness, HarnessBuilder};

fn write_skill(h: &Harness, name: &str, contents: &str) {
    let path = h
        .workspace_path()
        .join(".karucode/skills")
        .join(name)
        .join("SKILL.md");
    std::fs::create_dir_all(path.parent().expect("skill dir")).expect("skill dir");
    std::fs::write(&path, contents).expect("skill file");
}

const RELEASE: &str = "---\n\
name: release\n\
description: Cut a release: bump versions, run the gates, tag, push.\n\
---\n\
\n\
# Cutting a release\n\
\n\
1. `cargo test --workspace`\n\
";

fn command<'a>(catalog: &'a Value, name: &str) -> Option<&'a Value> {
    catalog["commands"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == json!(name))
}

async fn post_message(h: &Harness, session: &str, content: &str) -> support::TestResponse {
    h.post(
        &format!("/api/sessions/{session}/messages"),
        json!({ "content": content }),
    )
    .await
}

/// The newest stored message of a session.
async fn newest_message(h: &Harness, session: &str) -> Value {
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    detail["messages"]
        .as_array()
        .and_then(|messages| messages.last().cloned())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn the_catalog_lists_the_builtins_with_the_text_the_composer_inserts() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let response = h.get("/api/commands").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let catalog = response.json();

    let compact = command(&catalog, "compact").expect("/compact is catalogued");
    assert_eq!(compact["source"], json!("builtin"));
    assert_eq!(compact["category"], json!("context"));
    assert_eq!(compact["args"], json!(""));
    assert_eq!(
        compact["insert"],
        json!("/compact"),
        "a command with no arguments inserts no trailing space"
    );

    let model = command(&catalog, "model").expect("/model is catalogued");
    assert_eq!(model["insert"], json!("/model "));
    assert!(model["args"]
        .as_str()
        .is_some_and(|args| args.contains("model")));

    // `/help` is how a person discovers the rest, so it must be there.
    assert!(command(&catalog, "help").is_some());
}

#[tokio::test]
async fn a_workspace_skill_is_a_command_of_its_own() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    write_skill(&h, "release", RELEASE);

    let catalog = h.get("/api/commands").await.json();
    let release = command(&catalog, "release").expect("the skill is catalogued");
    assert_eq!(release["source"], json!("skill"));
    assert_eq!(
        release["title"],
        json!("release"),
        "the display name comes from the skill's own frontmatter"
    );
    assert_eq!(release["insert"], json!("/release "));
}

#[tokio::test]
async fn a_builtin_name_is_never_shadowed_by_a_skill() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    // A skill deliberately named after a built-in: `/help` must still mean help,
    // and the skill stays reachable as `/skill help`.
    write_skill(&h, "help", RELEASE);

    let catalog = h.get("/api/commands").await.json();
    let helps = catalog["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .filter(|entry| entry["name"] == json!("help"))
        .collect::<Vec<_>>();
    assert_eq!(helps.len(), 1, "one row per name: {catalog}");
    assert_eq!(helps[0]["source"], json!("builtin"));
}

#[tokio::test]
async fn a_command_is_answered_in_place_and_starts_no_run() {
    // A scripted factory with nothing queued: if a command started a run, the run
    // would fail for want of a script, so an empty factory is also the proof.
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("commands")).await;
    let (_, _, body) = h.open_events("", &[]).await;

    let response = post_message(&h, &session, "/help").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let body_json = response.json();
    assert_eq!(body_json["mode"], json!("command"));
    assert_eq!(body_json["command"], json!("help"));
    assert_eq!(body_json["state"], json!("ok"));
    let message_id = body_json["message_id"].as_i64().expect("a persisted row");

    // The frame carries the same id the row has, so a client can match them.
    let events = collect_events_until(body, "command_result").await;
    let result = events
        .iter()
        .find(|event| event["type"] == "command_result")
        .expect("command_result event");
    assert_eq!(result["command"], json!("help"));
    assert_eq!(result["message_id"], json!(message_id));
    assert_eq!(result["state"], json!("ok"));
    assert!(result["content"]
        .as_str()
        .is_some_and(|content| content.contains("/compact")));

    // It is a transcript row of its own, tagged so the UI can draw it as one.
    let stored = newest_message(&h, &session).await;
    assert_eq!(stored["id"], json!(message_id));
    assert_eq!(stored["name"], json!("slash:help"));
    assert_eq!(stored["role"], json!("assistant"));

    // No run was started.
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    assert_eq!(detail["runs"], json!([]));
    assert_eq!(detail["running"], json!(false));
}

#[tokio::test]
async fn a_command_result_is_not_sent_to_the_model() {
    let h = HarnessBuilder::new(vec![text("done")])
        .no_ai_titles()
        .build()
        .await;
    let session = h.create_session(Some("commands")).await;

    // A marker only the command's output carries.
    assert_eq!(
        post_message(&h, &session, "/context").await.status,
        StatusCode::OK
    );
    assert_eq!(
        post_message(&h, &session, "carry on").await.status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&session).await;

    let prompts = h.factory.prompts();
    let sent = prompts.last().expect("one request").join("\n");
    assert!(sent.contains("carry on"), "{sent}");
    assert!(
        !sent.contains("### Context"),
        "the /context answer must not be in the request: {sent}"
    );
    assert!(
        !sent.contains("slash:"),
        "not even its tag may leak into the request: {sent}"
    );
}

#[tokio::test]
async fn a_typo_names_the_command_it_was_meant_to_be() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = h.create_session(Some("commands")).await;

    let response = post_message(&h, &session, "/compcat").await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.error_code(), json!("bad_request"));
    let message = response.error_message();
    assert!(message.contains("/compcat"), "{message}");
    assert!(message.contains("`/compact`"), "{message}");
}

#[tokio::test]
async fn a_message_that_merely_starts_with_a_slash_is_still_a_prompt() {
    let h = HarnessBuilder::new(vec![text("that is a path")])
        .no_ai_titles()
        .build()
        .await;
    let session = h.create_session(Some("paths")).await;

    let response = post_message(&h, &session, "/home/me/notes.md — why is this failing?").await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    assert!(response.json()["run_id"].is_string());
}

#[tokio::test]
async fn a_skill_invocation_rewrites_the_message_into_a_prompt() {
    let h = HarnessBuilder::new(vec![text("following the release skill")])
        .no_ai_titles()
        .build()
        .await;
    write_skill(&h, "release", RELEASE);
    let session = h.create_session(Some("skills")).await;

    let response = post_message(&h, &session, "/skill release bump to 1.2").await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    h.wait_until_idle(&session).await;

    // The expanded prompt is what the model read, and what the transcript shows:
    // it names the tool, the action and the id, and carries the task verbatim.
    let sent = h.factory.prompts().last().expect("one request").join("\n");
    assert!(sent.contains("Read it with the `skill` tool"), "{sent}");
    assert!(sent.contains("`release` skill"), "{sent}");
    assert!(sent.contains("bump to 1.2"), "{sent}");
}

#[tokio::test]
async fn a_skill_invoked_by_its_own_name_works_too() {
    let h = HarnessBuilder::new(vec![text("ok")])
        .no_ai_titles()
        .build()
        .await;
    write_skill(&h, "release", RELEASE);
    let session = h.create_session(Some("skills")).await;

    let response = post_message(&h, &session, "/release").await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    h.wait_until_idle(&session).await;
    let sent = h.factory.prompts().last().expect("one request").join("\n");
    assert!(sent.contains("`release` skill"), "{sent}");
}

#[tokio::test]
async fn a_skill_invocation_titles_the_session_from_what_was_typed() {
    // The expansion replaces the *message*, not the reason the person is here. A
    // title derived from it read "Use the `release` skill. Read it with the `skill`
    // tool..." - the boilerplate, with the task buried in it.
    let h = HarnessBuilder::new(vec![text("following the release skill")])
        .no_ai_titles()
        .build()
        .await;
    write_skill(&h, "release", RELEASE);
    // Untitled on purpose: a session the user named keeps its name.
    let session = h.create_session(None).await;

    let response = post_message(&h, &session, "/skill release bump to 1.2").await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    h.wait_until_idle(&session).await;

    let row = store_of(&h)
        .get_session(&session)
        .await
        .expect("read the session")
        .expect("the session exists");
    assert!(
        row.title.contains("bump to 1.2"),
        "titled from the task the person typed: {:?}",
        row.title
    );
    assert!(
        !row.title.contains("skill` tool"),
        "not from the expanded prompt: {:?}",
        row.title
    );
}

#[tokio::test]
async fn prose_that_merely_looks_like_a_command_keeps_its_whole_title() {
    // `/tmp file is stale` parses as an invocation with args, but the catalog knows
    // no `tmp`: it is prose, and it is sent as written. Its title must be the whole
    // message, not just the part that looked like arguments.
    let h = HarnessBuilder::new(vec![text("ok")])
        .no_ai_titles()
        .build()
        .await;
    let session = h.create_session(None).await;

    let response = post_message(&h, &session, "/tmp file is stale").await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.text());
    h.wait_until_idle(&session).await;

    let sent = h.factory.prompts().last().expect("one request").join("\n");
    assert!(sent.contains("/tmp file is stale"), "sent as prose: {sent}");
    let row = store_of(&h)
        .get_session(&session)
        .await
        .expect("read the session")
        .expect("the session exists");
    // `derive_title` strips the leading `/` sigil, so the whole message minus that
    // sigil is the title - and crucially not just the part that parsed as arguments.
    assert_eq!(
        row.title, "tmp file is stale",
        "titled from the whole message, not its would-be arguments"
    );
}

#[tokio::test]
async fn an_unknown_skill_is_a_refusal_in_the_transcript() {
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("skills")).await;

    let response = post_message(&h, &session, "/skill nope").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["state"], json!("error"));

    // A refusal is recorded like any other answer, marked as one.
    let stored = newest_message(&h, &session).await;
    assert_eq!(stored["name"], json!("slash:skill:error"));
    assert!(stored["content"]
        .as_str()
        .is_some_and(|content| content.contains("There is no `nope` skill")));
}

#[tokio::test]
async fn rename_and_model_change_the_session_they_were_typed_in() {
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("old name")).await;

    let renamed = post_message(&h, &session, "/rename Release checklist").await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    assert_eq!(renamed.json()["state"], json!("ok"));
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    assert_eq!(detail["session"]["title"], json!("Release checklist"));

    let pinned = post_message(&h, &session, "/model test-model").await;
    assert_eq!(pinned.status, StatusCode::OK, "{}", pinned.text());
    assert_eq!(pinned.json()["state"], json!("ok"));
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    assert_eq!(detail["session"]["model"]["model"], json!("test-model"));
}

#[tokio::test]
async fn an_unknown_model_is_refused_without_touching_the_pin() {
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("model")).await;

    let response = post_message(&h, &session, "/model not-a-real-model").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["state"], json!("error"));
    assert!(newest_message(&h, &session).await["content"]
        .as_str()
        .is_some_and(|content| content.contains("not a model")));
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    assert_eq!(
        detail["session"]["model"],
        Value::Null,
        "the pin is untouched"
    );
}

#[tokio::test]
async fn stop_with_nothing_running_says_so_instead_of_failing() {
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("stop")).await;

    let response = post_message(&h, &session, "/stop").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["state"], json!("error"));
    assert!(newest_message(&h, &session).await["content"]
        .as_str()
        .is_some_and(|content| content.contains("Nothing is running")));
}

#[tokio::test]
async fn compacting_a_short_conversation_reports_that_there_was_nothing_to_do() {
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("compact")).await;

    let response = post_message(&h, &session, "/compact").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["state"], json!("ok"));
    assert!(newest_message(&h, &session).await["content"]
        .as_str()
        .is_some_and(|content| content.contains("Nothing to compact")));
}

#[tokio::test]
async fn a_command_is_answered_even_when_it_arrives_as_a_steer() {
    // `?mode=steer` with `/stop` is the case the interception exists for: the
    // command must run, not wait behind the run it is trying to stop.
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("steer")).await;

    let response = h
        .post(
            &format!("/api/sessions/{session}/messages?mode=steer"),
            json!({ "content": "/stop" }),
        )
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["mode"], json!("command"));
}

/// Append `pairs` realistic user/assistant exchanges of roughly `chars` each.
///
/// Alternating matters: compaction drops whole exchanges, so a transcript of
/// nothing but user messages has no boundary to cut at and is (correctly) left
/// alone.
async fn seed_exchanges(h: &Harness, session: &str, pairs: usize, chars: usize) {
    let store = store_of(h);
    for index in 0..pairs {
        for role in ["user", "assistant"] {
            store
                .append_message(&NewMessage {
                    session_id: session.to_string(),
                    run_id: None,
                    agent_id: None,
                    role: role.to_string(),
                    content: format!("{role} {index}: {}", "x".repeat(chars)),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    name: None,
                    images: Vec::new(),
                    reasoning_content: None,
                })
                .await
                .expect("stored message");
        }
    }
}

#[tokio::test]
async fn compacting_a_long_conversation_reports_what_it_saved() {
    // A budget small enough that a handful of exchanges fill it: `/compact` aims at
    // a quarter of it, so the conversation really is folded rather than skipped.
    let h = HarnessBuilder::new(vec![text("carried on")])
        .no_ai_titles()
        .limits(Limits {
            context_max_chars: 20_000,
            ..Default::default()
        })
        .build()
        .await;
    let session = h.create_session(Some("long")).await;
    // Short messages, so the loaded window holds many of them: the newest 32 are
    // protected from being dropped at all, and a window that is mostly protected
    // tail is (correctly) left alone.
    seed_exchanges(&h, &session, 300, 100).await;

    let response = post_message(&h, &session, "/compact").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(response.json()["state"], json!("ok"));
    let content = newest_message(&h, &session).await["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(content.contains("Compacted"), "{content}");

    // The compacted window is what the next run answers from, so it has to be a
    // request the provider accepts: an orphaned tool result or a dropped task
    // would fail the turn rather than the compaction.
    assert_eq!(
        post_message(&h, &session, "carry on").await.status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&session).await;
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    assert_eq!(
        detail["runs"]
            .as_array()
            .and_then(|runs| runs.last())
            .map(|run| run["status"].clone()),
        Some(json!("completed"))
    );
}

#[tokio::test]
async fn an_unknown_project_is_a_404_for_the_catalog() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let response = h.get("/api/commands?project=p_missing").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}
