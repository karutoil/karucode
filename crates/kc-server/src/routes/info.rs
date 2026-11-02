//! `GET /api/sessions/:id/info` — the numbers behind a session, for the
//! inspector's **Session** tab.
//!
//! Separate from `GET /api/sessions/:id` on purpose. That route exists to load a
//! transcript: it ships the newest page of messages plus runs, agents, approvals
//! and claims, and every reload of the chat pays for it. This route answers "how
//! much work was this, where did the tokens go, and how close is the window"
//! with counts, durations and token totals — no message bodies — so the tab can
//! stay open through a long run and refresh cheaply.
//!
//! Two things the transcript page cannot give, and the reason this is a query
//! rather than a fold over the client's state:
//!
//! * **Whole-session counts.** The transcript window is bounded
//!   (`MESSAGE_LIMIT`); "12 481 messages" has to be counted in the database.
//! * **Token totals after a reload.** A `usage` event is live-only, so the
//!   per-call rows the runtime writes are the only surviving record.

use crate::error::ApiResult;
use crate::routes::sessions::session_or_404;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use kc_core::settings::Settings as _;
use kc_core::store::UsageTotals;
use serde::Serialize;
use serde_json::{json, Value};

/// Ceiling on the per-tool breakdown, so a session that called 400 distinct
/// tools cannot make this response arbitrarily large. The total still counts
/// every call; `tools_truncated` says the list is not the whole story.
const MAX_TOOL_ENTRIES: usize = 40;

/// One agent's share of the session, as the tab shows it in a row.
#[derive(Debug, Serialize)]
struct AgentInfo {
    id: String,
    name: String,
    role: String,
    status: String,
    model: String,
    iterations: u32,
    /// `None` while an agent is still running: its work is not over.
    duration_ms: Option<i64>,
    created_at: i64,
    finished_at: Option<i64>,
    /// Messages in this agent's own transcript.
    messages: u64,
    /// Provider calls it made, and what they cost.
    usage: UsageView,
}

/// Token totals on the wire.
///
/// `cache_hit_ratio` is computed here, once, so every client shows the same
/// figure and knows the difference between "0%" and "the endpoint does not report
/// it" - the latter is `null`, and `cached_known_calls` says how many calls
/// answered the question at all.
#[derive(Debug, Serialize)]
struct UsageView {
    calls: u32,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cached_known_calls: u32,
    cache_hit_ratio: Option<f64>,
    last_input_tokens: u32,
}

impl From<UsageTotals> for UsageView {
    fn from(totals: UsageTotals) -> Self {
        Self {
            calls: totals.calls,
            input_tokens: totals.input_tokens,
            output_tokens: totals.output_tokens,
            cached_tokens: totals.cached_tokens,
            cached_known_calls: totals.cached_known_calls,
            cache_hit_ratio: totals.cache_hit_ratio(),
            last_input_tokens: totals.last_input_tokens,
        }
    }
}

/// `GET /api/sessions/:id/info`
pub async fn session_info(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &id).await?;
    let store = state.store();

    let stats = store.session_stats(&session.id).await?;
    let usage = store.usage_totals(&session.id).await?;
    let agents = store.list_agents(&session.id).await?;
    let runs = store.list_runs(&session.id, RUN_LIMIT).await?;
    let running = state.runtime().has_active_run(&session.id);

    // Per-agent message counts: `list_agent_messages` is the right answer
    // (it is what the Agents view shows), but it is one query per agent. The
    // session's own message table already carries `agent_id`, so one grouped
    // count over it is both cheaper and consistent with `SessionStats`.
    let agent_message_counts = store.agent_message_counts(&session.id).await?;

    let main_usage = usage.get(&None).copied().unwrap_or_default();

    // The window the newest prompt is measured against: the model this session
    // would resolve to right now (its pin wins over the install default). Absent
    // when the model entry does not declare a window - the UI must not draw a
    // percentage against a made-up denominator.
    let resolved = state.settings().resolve_model(session.model.as_ref()).await;
    let context_window = resolved.context_window.filter(|window| *window > 0);

    let mut tools: Vec<(String, u64)> = stats
        .tool_calls
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let tools_truncated = tools.len() > MAX_TOOL_ENTRIES;
    tools.truncate(MAX_TOOL_ENTRIES);

    let agent_rows: Vec<AgentInfo> = agents
        .iter()
        .map(|agent| {
            let duration_ms = agent
                .finished_at
                .map(|finished| (finished - agent.created_at).max(0));
            AgentInfo {
                id: agent.id.clone(),
                name: agent.name.clone(),
                role: agent.role.clone(),
                status: agent.status.clone(),
                model: agent.model.clone(),
                iterations: agent.iterations,
                duration_ms,
                created_at: agent.created_at,
                finished_at: agent.finished_at,
                messages: agent_message_counts.get(&agent.id).copied().unwrap_or(0),
                usage: usage
                    .get(&Some(agent.id.clone()))
                    .copied()
                    .unwrap_or_default()
                    .into(),
            }
        })
        .collect();

    Ok(Json(json!({
        "session": session,
        "running": running,
        "stats": stats,
        "usage": {
            "main": UsageView::from(main_usage),
            // Every agent's share, so a client that does not want the full agent
            // list can still show the totals split.
            "by_agent": usage
                .iter()
                .filter_map(|(agent_id, totals)| {
                    agent_id
                        .as_ref()
                        .map(|id| (id.clone(), UsageView::from(*totals)))
                })
                .collect::<std::collections::BTreeMap<_, _>>(),
            // The session as a whole: the main agent plus every subagent. One
            // number to show, and the sum of exactly what the breakdown holds.
            "total": UsageView::from(session_usage_total(&usage)),
        },
        "context": {
            "model": resolved.model,
            "window": context_window,
            // The newest main-agent call's prompt size, which is what fills the
            // window. A subagent has its own small context; mixing them in would
            // make the number drop whenever a subagent ran.
            "used": main_usage.last_input_tokens,
        },
        "agents": agent_rows,
        "messages": {
            "total": stats.messages_total,
            "main": stats.messages_main,
            "subagent": stats.messages_subagent,
            "by_role": stats.messages_by_role,
            "main_by_role": stats.messages_main_by_role,
        },
        "tools": {
            "total": stats.tool_calls_total,
            "by_name": tools
                .into_iter()
                .map(|(name, count)| json!({ "name": name, "count": count }))
                .collect::<Vec<_>>(),
            "truncated": tools_truncated,
        },
        "runs": {
            "total": stats.runs_total,
            "by_status": stats.runs_by_status,
            "working_ms": stats.working_ms,
            // Only the newest few, for a timeline: the full list is what
            // `GET /api/sessions/:id` already returns.
            "recent": runs,
        },
    })))
}

/// How many runs `recent` carries. The tab shows a short timeline, not a log.
const RUN_LIMIT: usize = 20;

/// The whole session's token totals: the main agent plus every subagent.
fn session_usage_total(
    usage: &std::collections::BTreeMap<Option<String>, UsageTotals>,
) -> UsageTotals {
    let mut total = UsageTotals::default();
    for totals in usage.values() {
        total.calls += totals.calls;
        total.input_tokens += totals.input_tokens;
        total.output_tokens += totals.output_tokens;
        total.cached_tokens += totals.cached_tokens;
        total.cached_known_calls += totals.cached_known_calls;
        // The window fill is the *latest* prompt, not the sum: taking the
        // maximum is the honest reduction when several agents each reported one,
        // because the number is never a cost that adds up.
        total.last_input_tokens = total.last_input_tokens.max(totals.last_input_tokens);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(input: u64, cached: u64, known: u32, last: u32) -> UsageTotals {
        UsageTotals {
            calls: 1,
            input_tokens: input,
            output_tokens: 1,
            cached_tokens: cached,
            cached_known_calls: known,
            last_input_tokens: last,
        }
    }

    #[test]
    fn the_session_total_adds_cost_and_takes_the_largest_window_fill() {
        let mut usage = std::collections::BTreeMap::new();
        usage.insert(None, totals(100, 10, 1, 40));
        usage.insert(Some("a_1".to_string()), totals(50, 0, 1, 12));
        usage.insert(Some("a_2".to_string()), UsageTotals::default());

        let total = session_usage_total(&usage);
        assert_eq!(total.calls, 2, "an agent with no call contributes nothing");
        assert_eq!(total.input_tokens, 150);
        assert_eq!(total.output_tokens, 2);
        assert_eq!(total.cached_tokens, 10);
        assert_eq!(total.cached_known_calls, 2);
        assert_eq!(
            total.last_input_tokens, 40,
            "the window fill is the largest prompt, not a sum"
        );
    }

    #[test]
    fn an_empty_session_totals_to_zero() {
        let total = session_usage_total(&std::collections::BTreeMap::new());
        assert_eq!(total, UsageTotals::default());
        assert!(total.cache_hit_ratio().is_none());
    }
}
