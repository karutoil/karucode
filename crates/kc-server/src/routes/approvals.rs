//! Approval listing and resolution.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, query_str, QueryMap};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::Json;
use kc_core::approval::ApprovalDecision;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionBody {
    #[serde(default)]
    decision: Option<String>,
}

/// `GET /api/approvals?session=<optional>` — pending approvals only.
pub async fn list_approvals(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let session = query_str(&query, "session");
    let approvals = state.store().list_pending_approvals(session).await?;
    Ok(Json(json!({ "approvals": approvals })))
}

/// `POST /api/approvals/:id` `{ "decision": "allow_once"|"allow_session"|"deny" }`.
pub async fn resolve_approval(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let body: DecisionBody = parse_required(&body, "POST /api/approvals/:id")?;
    let raw = body
        .decision
        .ok_or_else(|| ApiError::bad_request("decision is required"))?;
    let decision = parse_decision(&raw)?;

    // `resolve_approval` writes the record, wakes the waiting tool and emits
    // `approval_resolved`; unknown/expired ids are a 404 from the runtime.
    state.runtime().resolve_approval(&id, decision).await?;
    Ok(Json(json!({
        "resolved": true,
        "decision": decision.as_str(),
    })))
}

fn parse_decision(raw: &str) -> ApiResult<ApprovalDecision> {
    match raw.trim() {
        "allow_once" => Ok(ApprovalDecision::AllowOnce),
        "allow_session" => Ok(ApprovalDecision::AllowSession),
        "deny" => Ok(ApprovalDecision::Deny),
        "expired" => Err(ApiError::bad_request(
            "`expired` is produced by the server; send allow_once, allow_session or deny",
        )),
        other => Err(ApiError::bad_request(format!(
            "unknown decision `{other}`: expected allow_once, allow_session or deny"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_parse_exactly() {
        assert_eq!(
            parse_decision("allow_once").unwrap(),
            ApprovalDecision::AllowOnce
        );
        assert_eq!(
            parse_decision("allow_session").unwrap(),
            ApprovalDecision::AllowSession
        );
        assert_eq!(parse_decision("deny").unwrap(), ApprovalDecision::Deny);
        assert!(parse_decision("expired").is_err());
        assert!(parse_decision("yes").is_err());
    }
}
