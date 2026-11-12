//! The two hot read paths, at the size that made them hot.
//!
//! `GET /api/agents` used to answer with every agent on the server (1042 rows,
//! 4.86 MB of JSON) and `GET /api/control/overview` used to build every session
//! and every agent summary to fill twelve-row lists and answer 31 KB. These tests
//! pin the replacement: a page with whole-fleet counts, and a snapshot whose lists
//! are truncated but whose totals are not.
//!
//! Seeding is direct through the store. The behaviour under test is SQL, and a
//! scripted spawn per agent would add a scheduling race to every assertion.

mod support;

use axum::http::StatusCode;
use kc_core::store::{AgentRecord, ApprovalRecord, RunStatus};
use serde_json::{json, Value};
use support::{store_of, Harness, HarnessBuilder};

const BASE_TIME: i64 = 1_700_000_000_000;

/// One seeded agent: `index` orders them, so "newest first" is checkable.
fn agent_record(session_id: &str, index: usize, status: &str) -> AgentRecord {
    AgentRecord {
        id: format!("a_{index:04}"),
        session_id: session_id.to_string(),
        parent_run_id: String::new(),
        parent_agent_id: None,
        name: format!("agent-{index:04}"),
        role: "worker".to_string(),
        task: "look at the open pull requests and report back".to_string(),
        status: status.to_string(),
        model: "test-model".to_string(),
        iterations: index as u32,
        summary: String::new(),
        error: None,
        owned_paths: vec![],
        created_at: BASE_TIME + index as i64,
        updated_at: BASE_TIME + 10_000 + index as i64,
        finished_at: None,
        background: false,
        note: String::new(),
        progress: None,
    }
}

async fn harness() -> Harness {
    HarnessBuilder::new(vec![])
        .no_ai_titles()
        .auto_approvals()
        .build()
        .await
}

async fn new_project_session(h: &Harness, title: &str) -> String {
    let project = h.get("/api/projects").await.json()["projects"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.post(
        "/api/sessions",
        json!({ "project": project, "title": title }),
    )
    .await
    .json()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Statuses chosen so `running`/`paused` differ from each other and from the
/// total: 13 live (12 running + 1 paused) is impossible to confuse with a page.
fn fleet_status(index: usize) -> &'static str {
    match index % 10 {
        0 => "running",
        1 => "paused",
        2 => "failed",
        3 => "cancelled",
        _ => "completed",
    }
}

#[tokio::test]
async fn agents_default_page_is_newest_hundred_with_whole_fleet_counts() {
    let h = harness().await;
    let session = new_project_session(&h, "Delegating session").await;
    let store = store_of(&h);
    let total = 130usize;
    for index in 0..total {
        store
            .upsert_agent(&agent_record(&session, index, fleet_status(index)))
            .await
            .unwrap();
    }
    let expected_live = (0..total).filter(|i| i % 10 < 2).count();
    let expected_paused = (0..total).filter(|i| i % 10 == 1).count();

    let page = h.get("/api/agents").await;
    assert_eq!(page.status, StatusCode::OK);
    let body = page.json();
    let agents = body["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 100, "the default page is 100 rows");
    assert_eq!(body["total"], total, "{body}");
    // Counts are fleet facts: they must not shrink to the page.
    assert_eq!(body["running"], expected_live, "{body}");
    assert_eq!(body["paused"], expected_paused, "{body}");
    // 100 agents of ~500 bytes must stay far below the megabyte the default
    // promises, or "the default keeps the response under 1 MB" is not true.
    assert!(
        page.body.len() < 1_000_000,
        "default page was {} bytes",
        page.body.len()
    );
    // Newest first: `created_at` ascends with `index`, so the page ends at 30.
    assert_eq!(agents[0]["id"], "a_0129");
    assert_eq!(agents[99]["id"], "a_0030");
    for agent in agents {
        assert_eq!(agent["session_title"], "Delegating session");
    }

    // Paging walks the same ordered set with the same counts.
    let second = h.get("/api/agents?limit=100&offset=100").await.json();
    let rest = second["agents"].as_array().unwrap();
    assert_eq!(rest.len(), 30);
    assert_eq!(rest[0]["id"], "a_0029");
    assert_eq!(rest[29]["id"], "a_0000");
    assert_eq!(second["total"], total);
    assert_eq!(second["running"], expected_live);
    assert_eq!(second["paused"], expected_paused);

    // Past the end is an empty page, not an error, and `total` still says 130.
    let past = h.get("/api/agents?limit=10&offset=1000").await.json();
    assert!(past["agents"].as_array().unwrap().is_empty());
    assert_eq!(past["total"], total);

    // A small page reports the same fleet counts as the whole list.
    let small = h.get("/api/agents?limit=3").await.json();
    assert_eq!(small["agents"].as_array().unwrap().len(), 3);
    assert_eq!(small["running"], expected_live);
    assert_eq!(small["total"], total);
}

#[tokio::test]
async fn agents_page_bounds_are_documented_and_enforced() {
    let h = harness().await;
    let session = new_project_session(&h, "Small fleet").await;
    let store = store_of(&h);
    for index in 0..5 {
        store
            .upsert_agent(&agent_record(&session, index, "completed"))
            .await
            .unwrap();
    }

    // `?all=true` is how a script asks for the fleet: everything, capped at 2000.
    let all = h.get("/api/agents?all=true").await.json();
    assert_eq!(all["agents"].as_array().unwrap().len(), 5);
    assert_eq!(all["total"], 5);

    // A limit above the cap is refused rather than quietly clamped: the caller
    // asked for a response this route deliberately does not produce.
    let too_many = h.get("/api/agents?limit=2001").await;
    assert_eq!(too_many.status, StatusCode::BAD_REQUEST);
    assert!(
        too_many.error_message().contains("2000"),
        "{}",
        too_many.error_message()
    );
    // Zero is a bug in the caller, not "give me everything".
    let zero = h.get("/api/agents?limit=0").await;
    assert_eq!(zero.status, StatusCode::BAD_REQUEST);
    assert!(
        zero.error_message().contains("all=true"),
        "{}",
        zero.error_message()
    );
    let at_cap = h.get("/api/agents?limit=2000").await;
    assert_eq!(at_cap.status, StatusCode::OK);
    assert_eq!(at_cap.json()["agents"].as_array().unwrap().len(), 5);

    let nonsense = h.get("/api/agents?offset=abc").await;
    assert_eq!(nonsense.status, StatusCode::BAD_REQUEST);
    // An unknown status still wins over paging: the filter is validated first.
    let bad_status = h.get("/api/agents?status=nope").await;
    assert_eq!(bad_status.status, StatusCode::BAD_REQUEST);
}

/// The overview answers twelve-row lists and whole-fleet totals, and the two are
/// derived from the same snapshot.
#[tokio::test]
async fn overview_truncates_lists_but_not_its_totals() {
    let h = harness().await;
    let store = store_of(&h);

    // 15 sessions with a run in flight: more than one page of `running`.
    let mut running_sessions = Vec::new();
    for index in 0..15 {
        let session = new_project_session(&h, &format!("Running {index}")).await;
        store
            .create_run(&format!("r_run_{index}"), &session, "test-model", "go")
            .await
            .unwrap();
        running_sessions.push(session);
    }
    // 14 sessions whose latest run failed: more than one page of `attention`.
    for index in 0..14 {
        let session = new_project_session(&h, &format!("Failed {index}")).await;
        store
            .create_run(&format!("r_fail_{index}"), &session, "test-model", "go")
            .await
            .unwrap();
        store
            .finish_run(&format!("r_fail_{index}"), RunStatus::Failed, Some("boom"))
            .await
            .unwrap();
    }
    // Two quiet sessions, one of them stalled on an unanswered approval.
    let quiet = new_project_session(&h, "Quiet").await;
    let waiting = new_project_session(&h, "Waiting on a person").await;
    store
        .create_approval(&ApprovalRecord {
            id: "ap_1".to_string(),
            session_id: waiting.clone(),
            run_id: String::new(),
            agent_id: None,
            kind: "command".to_string(),
            title: "Run the tests".to_string(),
            detail: "cargo test".to_string(),
            payload: json!({}),
            status: "pending".to_string(),
            decision: None,
            created_at: BASE_TIME,
            resolved_at: None,
        })
        .await
        .unwrap();

    // Agents: 20 live (14 running + 6 paused) plus finished ones, all in the
    // sessions above, so `live_agents` has to truncate too.
    let host = running_sessions[0].clone();
    for index in 0..20 {
        let status = if index < 14 { "running" } else { "paused" };
        store
            .upsert_agent(&agent_record(&host, index, status))
            .await
            .unwrap();
    }
    for index in 20..25 {
        store
            .upsert_agent(&agent_record(&host, index, "completed"))
            .await
            .unwrap();
    }

    // One control chat that must appear in `control` and in none of the lists.
    let control = h
        .post("/api/control/sessions", json!({"title": "Supervisor"}))
        .await
        .json()["session_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let overview = h.get("/api/control/overview").await;
    assert_eq!(overview.status, StatusCode::OK);
    let body = overview.json();

    assert_eq!(body["sessions"]["total"], 31, "{body}");
    assert_eq!(body["sessions"]["running"], 15, "{body}");
    assert_eq!(body["sessions"]["failed"], 14, "{body}");
    assert_eq!(body["sessions"]["control"], 1, "{body}");

    assert_eq!(body["running"].as_array().unwrap().len(), 12, "{body}");
    assert_eq!(body["attention"].as_array().unwrap().len(), 12, "{body}");
    assert_eq!(body["recent"].as_array().unwrap().len(), 12, "{body}");
    let waiting_rows = body["waiting"].as_array().unwrap();
    assert_eq!(waiting_rows.len(), 1, "{body}");
    assert_eq!(waiting_rows[0]["id"], waiting.as_str());
    assert_eq!(body["approvals"]["pending"], 1);

    assert_eq!(body["agents"]["live"], 20, "{body}");
    assert_eq!(body["agents"]["paused"], 6, "{body}");
    assert_eq!(body["agents"]["failed"], 0, "{body}");
    assert_eq!(body["agents"]["total"], 25, "{body}");
    assert_eq!(body["live_agents"].as_array().unwrap().len(), 12, "{body}");

    // The control chat is counted but not listed among project sessions, and the
    // quiet session is a `recent` row, not a `running` one.
    assert_eq!(body["control"].as_array().unwrap().len(), 1);
    assert_eq!(body["control"][0]["id"], Value::String(control.clone()));
    assert!(!body["recent"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == Value::String(control.clone())));
    let recent_ids: Vec<&str> = body["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert!(recent_ids.contains(&quiet.as_str()), "{recent_ids:?}");

    // Per-project counts come from one GROUP BY: 31 sessions in the one project.
    let project = &body["projects"][0];
    assert_eq!(project["sessions"], 31, "{body}");
    assert_eq!(project["running"], 15, "{body}");

    // The response stays small: this is the point of the change, and a regression
    // that starts materialising every row again shows up here as a size jump.
    assert!(
        overview.body.len() < 200_000,
        "overview was {} bytes",
        overview.body.len()
    );
}

/// `control_list_agents` shares the fleet-page economics: it reads a page, not
/// the fleet, and its `total` stays honest both when the page is short (the
/// list *is* the answer) and when it is full (the count comes from SQL under the
/// same filter).
#[tokio::test]
async fn control_list_agents_pages_the_fleet_and_keeps_totals_honest() {
    let h = harness().await;
    let session = new_project_session(&h, "Delegating session").await;
    let store = store_of(&h);
    // More than the tool's default page of 40, with a status mix the count can
    // disagree with: 48 completed and 12 running means a `running` filter over
    // a full page must still count 12, not 40.
    let total = 60usize;
    for index in 0..total {
        let status = if index % 5 == 0 {
            "running"
        } else {
            "completed"
        };
        store
            .upsert_agent(&agent_record(&session, index, status))
            .await
            .unwrap();
    }
    let running_total = (0..total).filter(|i| i % 5 == 0).count();
    let central = h
        .post("/api/control/sessions", json!({"title": "Supervisor"}))
        .await
        .json()["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let send = format!("/api/sessions/{central}/messages");

    // Default page: 40 of 60, so `total` cannot come from the rows returned.
    h.factory
        .push(support::tool_call("control_list_agents", "{}"));
    h.factory.push(support::text("Sixty agents."));
    assert_eq!(
        h.post(&send, json!({"content": "Who is doing what?"}))
            .await
            .status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&central).await;
    let detail = h.get(&format!("/api/sessions/{central}")).await.json();
    let body = tool_result_json(&detail, "agents");
    assert_eq!(body["total"], total, "{body}");
    assert_eq!(body["agents"].as_array().unwrap().len(), 40, "{body}");

    // A filtered full page: `total` counts the filtered fleet (13), not the 40
    // rows a filtered page would carry on a bigger fleet.
    h.factory.push(support::tool_call(
        "control_list_agents",
        &json!({"status": "running"}).to_string(),
    ));
    h.factory.push(support::text("Only the running ones."));
    assert_eq!(
        h.post(&send, json!({"content": "Who is running?"}))
            .await
            .status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&central).await;
    let detail = h.get(&format!("/api/sessions/{central}")).await.json();
    let body = tool_result_json(&detail, "agents");
    assert_eq!(body["total"], running_total, "{body}");
    assert_eq!(
        body["agents"].as_array().unwrap().len(),
        running_total,
        "{body}"
    );

    // A short page under the same filter reports its rows and the same count.
    h.factory.push(support::tool_call(
        "control_list_agents",
        &json!({"status": "running", "limit": 5}).to_string(),
    ));
    h.factory.push(support::text("Five runners."));
    assert_eq!(
        h.post(&send, json!({"content": "Show me five runners"}))
            .await
            .status,
        StatusCode::ACCEPTED
    );
    h.wait_until_idle(&central).await;
    let detail = h.get(&format!("/api/sessions/{central}")).await.json();
    let body = tool_result_json(&detail, "agents");
    assert_eq!(body["total"], running_total, "{body}");
    assert_eq!(body["agents"].as_array().unwrap().len(), 5, "{body}");
}

/// Pull the parsed JSON of the newest tool result carrying `key` out of a
/// session detail.
fn tool_result_json(detail: &Value, key: &str) -> Value {
    detail["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| serde_json::from_str::<Value>(message["content"].as_str()?).ok())
        .find(|value| value.get(key).is_some())
        .unwrap_or_else(|| panic!("no tool result carrying `{key}` in {detail}"))
}
