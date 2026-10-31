//! `GET /api/health`.
//!
//! The probe never fails: a supervisor needs to *see* the reason a deployment is
//! unhealthy, and a `500` with an error body is not that. A broken store is
//! reported as `"status": "degraded"` with `"db": "error"` and a `db_error`
//! message, still with `200 OK`.

use crate::error::ApiResult;
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use kc_core::store::Project;
use serde_json::{json, Value};

pub async fn health(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let config = state.settings().snapshot();
    let effective = config.effective_model();
    let provider_configured =
        !effective.base_url.trim().is_empty() && !effective.model.trim().is_empty();
    let db = state.db_status().await;
    let auth_required = state.auth_token().is_some();
    // `db_status` above deliberately swallows store errors, so this call must
    // not undo that: a corrupt, locked or disk-full database would turn the
    // probe into a 500 and the supervisor would lose the reason. Report it.
    let projects = state.store().list_projects().await;
    let body = health_body(HealthFacts {
        db,
        projects,
        auth_required,
        provider_configured,
        uptime_secs: state.uptime_secs(),
        // `null` on a fresh install: no workspace exists until a project is created.
        workspace: state.workspace_root_display(),
    });
    Ok(Json(body))
}

/// Everything the probe reports, already gathered. Kept separate from the
/// handler so the degraded path is testable without an `AppState`.
struct HealthFacts {
    db: &'static str,
    projects: kc_core::Result<Vec<Project>>,
    auth_required: bool,
    provider_configured: bool,
    uptime_secs: u64,
    /// `None` on a fresh install: no workspace exists until a project is made.
    workspace: Option<String>,
}

fn health_body(facts: HealthFacts) -> Value {
    let (projects, db_error) = match facts.projects {
        Ok(projects) => (projects, None),
        Err(error) => {
            tracing::warn!("health probe could not list projects: {error}");
            (Vec::new(), Some(error.to_string()))
        }
    };
    let degraded = facts.db != "ok" || db_error.is_some();

    let mut body = json!({
        "status": if degraded { "degraded" } else { "ok" },
        "version": crate::VERSION,
        "uptime_secs": facts.uptime_secs,
        "workspace": facts.workspace,
        "projects": projects,
        "auth_required": facts.auth_required,
        "provider_configured": facts.provider_configured,
        "db": facts.db,
    });
    // Additive field: only present when the store actually failed, so the
    // documented shape is unchanged for a healthy server.
    if let Some(message) = db_error {
        if let Some(object) = body.as_object_mut() {
            object.insert("db_error".into(), Value::String(message));
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(projects: kc_core::Result<Vec<Project>>, db: &'static str) -> HealthFacts {
        HealthFacts {
            db,
            projects,
            auth_required: false,
            provider_configured: true,
            uptime_secs: 12,
            workspace: Some("/w".into()),
        }
    }

    #[test]
    fn a_store_failure_is_degraded_not_an_error() {
        let body = health_body(facts(
            Err(kc_core::Error::Store("database is locked".into())),
            "error",
        ));
        assert_eq!(body["status"], json!("degraded"));
        assert_eq!(body["projects"], json!([]));
        assert_eq!(body["db"], json!("error"));
        assert!(
            body["db_error"].as_str().unwrap().contains("locked"),
            "the reason must be visible: {body}"
        );
        // The documented fields are all still there.
        for key in [
            "status",
            "version",
            "uptime_secs",
            "workspace",
            "projects",
            "auth_required",
            "provider_configured",
            "db",
        ] {
            assert!(body.get(key).is_some(), "missing `{key}` in {body}");
        }
    }

    #[test]
    fn a_project_listing_failure_alone_degrades_the_probe() {
        // `db_status` can pass while the project query fails (a locked table, a
        // full disk); that must still be `degraded`, not `ok`.
        let body = health_body(facts(Err(kc_core::Error::Store("disk full".into())), "ok"));
        assert_eq!(body["status"], json!("degraded"));
        assert_eq!(body["db"], json!("ok"));
        assert!(body.get("db_error").is_some());
    }

    #[test]
    fn a_healthy_store_has_no_db_error_field() {
        let body = health_body(facts(Ok(Vec::new()), "ok"));
        assert_eq!(body["status"], json!("ok"));
        assert_eq!(body["projects"], json!([]));
        assert!(body.get("db_error").is_none());
    }
}
