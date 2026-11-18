//! `report_karucode_issue`, end to end: a real run, a real tool call, a real file.
//!
//! These go through the router and the scripted provider like the rest of the tool
//! tests, so what is asserted is what the runtime persisted and what the tool wrote,
//! never a fixture. Filing is switched off with `KARUCODE_ISSUE_SEND=0`, which is
//! the documented "save, never send" control: without it a developer machine with
//! an authenticated `gh` would file a real issue on every `cargo test`.

mod support;

use axum::http::StatusCode;
use kc_core::config::ApprovalMode;
use serde_json::json;
use std::path::{Path, PathBuf};
use support::{text, tool_call, Harness, HarnessBuilder};

/// A provider key the settings handle knows about, so the redaction sweep has
/// something real to remove. Long and odd enough not to appear by accident.
const KEY: &str = "sk-live-report-test-9f3c1a7d";

/// Stop the tool before it can reach GitHub - see the module comment.
fn no_sending() {
    std::env::set_var("KARUCODE_ISSUE_SEND", "0");
    // Never let a stray environment point the report at somebody's real repo.
    std::env::set_var("KARUCODE_ISSUE_REPO", "karucode-test/does-not-exist");
}

fn reports_dir(h: &Harness) -> PathBuf {
    h.state
        .db_path()
        .parent()
        .expect("the database lives in the data dir")
        .join("issue-reports")
}

/// Every saved report, oldest name first.
fn saved_reports(h: &Harness) -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(reports_dir(h)) else {
        return Vec::new();
    };
    let mut out = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
        .map(|path| {
            let body = std::fs::read_to_string(&path).expect("read the saved report");
            (path, body)
        })
        .collect::<Vec<_>>();
    out.sort();
    out
}

async fn prompt(h: &Harness, session: &str, content: &str) {
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": content }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());
    h.wait_until_idle(session).await;
}

/// The content of the first `role: "tool"` message in the session.
async fn tool_content(h: &Harness, session: &str) -> String {
    let session_value = h.get(&format!("/api/sessions/{session}")).await.json();
    session_value["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .expect("a tool message")
        .to_string()
}

fn assert_not_in(body: &str, needle: &str, what: &str) {
    assert!(
        !body.contains(needle),
        "{what} leaked into the report:\n{body}"
    );
}

#[tokio::test]
async fn a_report_is_written_redacted_and_never_claims_it_was_filed() {
    no_sending();
    let summary = format!(
        "The run stopped after the third tool call and never finished. I pasted my key {KEY} into \
         the prompt while debugging and the run still failed with a provider error."
    );
    let h = HarnessBuilder::new(vec![
        tool_call(
            "report_karucode_issue",
            &json!({
                "summary": summary,
                "expected": "The run finishes the task and the tests pass.",
                "steps": "1. ask for two files to be read\n2. wait\n3. the run ends early",
            })
            .to_string(),
        ),
        text("I saved the report."),
    ])
    .auto_approvals()
    .model_api_key(KEY)
    .no_ai_titles()
    .build()
    .await;

    let session = h.create_session(Some("Report the stall")).await;
    prompt(
        &h,
        &session,
        "report this issue to karucode: the run stopped after a tool call",
    )
    .await;

    let reports = saved_reports(&h);
    assert_eq!(reports.len(), 1, "exactly one report was written");
    let (path, body) = &reports[0];
    assert_eq!(path.parent(), Some(reports_dir(&h).as_path()));
    assert!(
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("the-run-stopped-after") && name.ends_with(".md")),
        "the file is named after the report: {path:?}"
    );

    // The user's report, in their words, with the sections the issue form asks for.
    assert!(body.starts_with("### What happened"), "{body}");
    assert!(body.contains("### What you expected"));
    assert!(body.contains("The run finishes the task and the tests pass."));
    assert!(body.contains("### How to reproduce"));
    assert!(body.contains("1. ask for two files to be read"));

    // The diagnostics a maintainer needs, collected from this server.
    assert!(body.contains("### karucode diagnostics"), "{body}");
    assert!(body.contains("| karucode | karucode "), "{body}");
    assert!(body.contains("| approval mode | auto |"), "{body}");
    assert!(body.contains("| platform | "), "{body}");
    assert!(
        body.contains("| provider | `openai_compatible` (profile `default`) · key configured |"),
        "{body}"
    );
    assert!(
        body.contains("| endpoint | `http://127.0.0.1:9/v1` |"),
        "{body}"
    );
    assert!(body.contains("| model | `test-model`"), "{body}");
    assert!(body.contains("| web sign-in | not required |"), "{body}");
    assert!(body.contains("`limits.max_tool_output_bytes`"), "{body}");
    assert!(body.contains("#### Plugins\n\nNone configured."), "{body}");
    assert!(
        body.contains("#### MCP servers\n\nNone configured."),
        "{body}"
    );
    assert!(
        body.contains("#### Last run in this session\n\n`running`"),
        "the run reporting itself is the last run:\n{body}"
    );

    // Nothing sensitive, and no path from this machine.
    assert!(body.contains("***"), "{body}");
    assert_not_in(body, KEY, "the provider key");
    assert_not_in(
        body,
        &h.workspace_path().to_string_lossy(),
        "the workspace path",
    );
    assert_not_in(body, "report this issue to karucode:", "the prompt text");

    // The answer is honest: written, not filed.
    let result = tool_content(&h, &session).await;
    assert!(result.contains("NOT SENT"), "{result}");
    assert_not_in(&result, KEY, "the provider key reaches the answer's title");
    assert!(
        result.contains(&path.to_string_lossy().to_string()),
        "{result}"
    );
    assert!(result.contains("KARUCODE_ISSUE_SEND"), "{result}");
    assert!(result.contains("gh issue create --repo"), "{result}");
    assert!(
        result.contains("Do not tell the user it was reported"),
        "{result}"
    );
}

#[tokio::test]
async fn the_approval_prompt_shows_the_whole_report_and_a_denial_writes_nothing() {
    let h = HarnessBuilder::new(vec![
        tool_call(
            "report_karucode_issue",
            &json!({
                "summary": "The agent loop ended with no answer and no error after a tool call.",
            })
            .to_string(),
        ),
        text("I did not file it."),
    ])
    .no_ai_titles()
    .build()
    .await;

    let session = h.create_session(Some("Ask before sending")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": "please report this issue to karucode" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::ACCEPTED, "{}", sent.text());

    // `ask` is the default mode, and an external tool parks on the prompt.
    let approval = h.wait_for_approval(&session).await;
    assert_eq!(approval["kind"], json!("network_tool"), "{approval}");
    let title = approval["title"].as_str().expect("title");
    assert!(
        title.starts_with("File this report on GitHub at "),
        "{title}"
    );
    let detail = approval["detail"].as_str().expect("detail");
    assert!(detail.contains("### What happened"), "{detail}");
    assert!(
        detail.contains("The agent loop ended with no answer"),
        "{detail}"
    );
    // The body the operator approves is the body that would be filed.
    let approved_body = approval["payload"]["body"].as_str().expect("body");
    assert!(
        approved_body.contains("### karucode diagnostics"),
        "{approved_body}"
    );
    assert!(
        detail.contains("The body below is the whole report"),
        "{detail}"
    );

    let denied = h
        .post(
            &format!("/api/approvals/{}", approval["id"].as_str().unwrap()),
            json!({ "decision": "deny" }),
        )
        .await;
    assert_eq!(denied.status, StatusCode::OK, "{}", denied.text());
    h.wait_until_idle(&session).await;

    assert!(
        saved_reports(&h).is_empty(),
        "a denied report must not be written: {:?}",
        saved_reports(&h).first().map(|(path, _)| path)
    );
    let result = tool_content(&h, &session).await;
    assert!(result.contains("denied by user"), "{result}");
    assert!(
        result.contains("nothing was sent and no file was written"),
        "{result}"
    );
}

#[tokio::test]
async fn read_only_refuses_to_file_anything() {
    let h = HarnessBuilder::new(vec![
        tool_call(
            "report_karucode_issue",
            &json!({ "summary": "The approval card never appeared for a shell command." })
                .to_string(),
        ),
        text("Refused."),
    ])
    .approval_mode(ApprovalMode::ReadOnly)
    .no_ai_titles()
    .build()
    .await;

    let session = h.create_session(Some("Read only")).await;
    prompt(&h, &session, "report this to karucode please").await;

    assert!(saved_reports(&h).is_empty());
    let result = tool_content(&h, &session).await;
    assert!(result.contains("read_only"), "{result}");
    assert!(
        result.contains("nothing may leave this machine"),
        "{result}"
    );
}

#[tokio::test]
async fn a_vague_request_is_refused_without_writing_anything() {
    no_sending();
    let h = HarnessBuilder::new(vec![
        tool_call(
            "report_karucode_issue",
            &json!({ "summary": "it broke" }).to_string(),
        ),
        text("What exactly went wrong?"),
    ])
    .auto_approvals()
    .no_ai_titles()
    .build()
    .await;

    let session = h.create_session(Some("Vague")).await;
    prompt(&h, &session, "report this issue to karucode").await;

    assert!(saved_reports(&h).is_empty());
    let result = tool_content(&h, &session).await;
    assert!(result.contains("must say what the issue is"), "{result}");
    assert!(
        result.contains("Ask the user what the problem is"),
        "{result}"
    );
}

#[tokio::test]
async fn diagnostics_can_be_left_out_entirely() {
    no_sending();
    let h = HarnessBuilder::new(vec![
        tool_call(
            "report_karucode_issue",
            &json!({
                "summary": "The session rail lost its scroll position after a long transcript.",
                "attach_diagnostics": false,
            })
            .to_string(),
        ),
        text("Saved."),
    ])
    .auto_approvals()
    .model_api_key(KEY)
    .no_ai_titles()
    .build()
    .await;

    let session = h.create_session(Some("Bare report")).await;
    prompt(&h, &session, "report this issue to karucode").await;

    let reports = saved_reports(&h);
    assert_eq!(reports.len(), 1, "{:?}", reports);
    let body = &reports[0].1;
    assert!(!body.contains("### karucode diagnostics"), "{body}");
    assert!(!body.contains("karucode-test"), "{body}");
    assert!(body.contains("The session rail lost its scroll position"));
    assert_not_in(body, KEY, "the provider key");
}

/// The answer a `/report` posted into the transcript.
async fn report_answer(h: &Harness, session: &str) -> String {
    let detail = h.get(&format!("/api/sessions/{session}")).await.json();
    detail["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .rev()
        .find(|message| {
            message["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("slash:report"))
        })
        .and_then(|message| message["content"].as_str())
        .expect("a /report answer")
        .to_string()
}

/// `/report` is the fallback that needs no model turn: it saves the same bundle,
/// answers in place, and never sends.
#[tokio::test]
async fn the_report_command_saves_without_a_model_turn() {
    no_sending();
    // No scripts at all: if this path called the provider the run would fail, which
    // is the point - a broken model is exactly when someone reaches for `/report`.
    let h = HarnessBuilder::new(Vec::new())
        .model_api_key(KEY)
        .no_ai_titles()
        .build()
        .await;

    let session = h.create_session(Some("Broken run")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": format!("/report the run stopped after a tool call - my key was {KEY}") }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(sent.json()["mode"], json!("command"), "{}", sent.text());

    let answer = report_answer(&h, &session).await;
    // The reply names the saved file and the two ways to finish, and says nothing
    // was sent.
    assert!(answer.contains("Report saved"), "{answer}");
    assert!(answer.contains("Nothing was sent"), "{answer}");
    assert!(answer.contains("gh issue create --repo"), "{answer}");
    assert!(answer.contains("issues/new?title="), "{answer}");
    assert_not_in(&answer, KEY, "the provider key");

    let reports = saved_reports(&h);
    assert_eq!(reports.len(), 1, "{:?}", reports);
    let saved = &reports[0].1;
    assert!(saved.contains("### karucode diagnostics"), "{saved}");
    assert!(saved.contains("### What happened"), "{saved}");
    assert_not_in(saved, KEY, "the provider key");
    assert!(
        answer.contains(&reports[0].0.to_string_lossy().to_string()),
        "the answer names the file: {answer}"
    );
}

/// A `/report` with no description is refused - the same rail as the tool.
#[tokio::test]
async fn the_report_command_needs_to_know_what_went_wrong() {
    let h = HarnessBuilder::new(Vec::new()).no_ai_titles().build().await;
    let session = h.create_session(Some("No detail")).await;
    let sent = h
        .post(
            &format!("/api/sessions/{session}/messages"),
            json!({ "content": "/report" }),
        )
        .await;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.text());
    assert_eq!(sent.json()["state"], json!("error"), "{}", sent.text());
    let answer = report_answer(&h, &session).await;
    assert!(answer.contains("needs to know what went wrong"), "{answer}");
    assert!(answer.contains("report this issue to karucode"), "{answer}");
    assert!(saved_reports(&h).is_empty());
}

/// The saved copy lives beside the database, never inside the project workspace.
#[tokio::test]
async fn the_report_is_not_written_into_the_workspace() {
    no_sending();
    let h = HarnessBuilder::new(vec![
        tool_call(
            "report_karucode_issue",
            &json!({ "summary": "The changes tab showed a diff from another project." })
                .to_string(),
        ),
        text("Saved."),
    ])
    .auto_approvals()
    .no_ai_titles()
    .build()
    .await;

    let session = h.create_session(Some("Not in the repo")).await;
    prompt(&h, &session, "report this issue to karucode").await;

    let workspace = h.workspace_path();
    let in_workspace = std::fs::read_dir(workspace)
        .expect("workspace dir")
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(
        in_workspace.is_empty(),
        "the workspace was touched: {in_workspace:?}"
    );
    assert_eq!(saved_reports(&h).len(), 1);
    assert_is_inside(reports_dir(&h).as_path(), h.state.db_path());
}

/// `dir` is the data directory's child, not somewhere else on disk.
fn assert_is_inside(dir: &Path, db_path: &Path) {
    assert_eq!(
        dir,
        db_path
            .parent()
            .expect("data dir")
            .join("issue-reports")
            .as_path()
    );
}
