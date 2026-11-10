//! `GET /api/update`, `POST /api/update/check`, `POST /api/update/apply`.
//!
//! The three verbs are one subsystem's whole surface:
//!
//! * `GET` never touches the network. It reports the current build, how this
//!   binary is installed (and whether it may be replaced), when the last check
//!   ran and what it found, any staged artifact, and the last applied update. The
//!   settings screen polls it, so it must be cheap and must not block.
//! * `POST /check` is the manual form of the background check. It is refused when
//!   `[updates] check` is off - with the reason, which is the only way an operator
//!   finds out why "Check now" does nothing.
//! * `POST /apply` stages if needed and installs. It needs the admin credential
//!   when one is configured: replacing the running binary is as consequential as
//!   repointing the provider. `{"restart": true}` also asks for the restart that
//!   makes the new build live, and is refused unless a supervisor would start the
//!   process again.
//!
//! See `crates/kc-server/src/update.rs` for the mechanics and the `updates` topic
//! for the operator's view.

use crate::error::{ApiError, ApiResult};
use crate::settings::authorize_admin;
use crate::state::AppState;
use crate::update::{self, UpdateError, UpdateErrorKind};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

/// `GET /api/update`
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(
        serde_json::to_value(state.updates().status())
            .map_err(|e| ApiError::internal(format!("could not encode the update status: {e}")))?,
    ))
}

/// `POST /api/update/check`
pub async fn check(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    match state.updates().check().await {
        Ok(_) => Ok(Json(
            serde_json::to_value(state.updates().status()).map_err(|e| {
                ApiError::internal(format!("could not encode the update status: {e}"))
            })?,
        )),
        Err(error) => Err(map_error(error)),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    /// Restart after installing, so the new build is the one running. Only
    /// possible when a supervisor will start the process again.
    #[serde(default)]
    pub restart: bool,
}

/// `POST /api/update/apply`
pub async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let request: ApplyRequest = crate::routes::parse_body(&body)?;
    // The admin gate runs before anything is downloaded: a request that is going
    // to be refused should not cost 20 MiB of traffic.
    let config = state.settings().snapshot();
    let presented = headers
        .get(crate::settings::ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    authorize_admin(&config, presented, "installing an update").map_err(ApiError::from)?;

    let outcome = state
        .updates()
        .apply(request.restart)
        .await
        .map_err(map_error)?;
    if outcome.restart_requested {
        update::request_restart(
            &state,
            outcome.to.parse::<u64>().unwrap_or_default(),
            std::time::Duration::from_millis(1500),
        );
    }
    let status = state.updates().status();
    Ok(Json(json!({
        "update": serde_json::to_value(status).unwrap_or(Value::Null),
        "result": serde_json::to_value(outcome)
            .map_err(|e| ApiError::internal(format!("could not encode the result: {e}")))?,
    })))
}

/// The documented error shape for an update failure.
///
/// A refusal is a `409` (`bad_request`, the documented code for a request the
/// server understood and would not perform); an upstream problem is a `502` with
/// the `update` code; a local file problem is a `500`.
fn map_error(error: UpdateError) -> ApiError {
    match error.kind {
        UpdateErrorKind::Refused => ApiError::conflict(error.message),
        UpdateErrorKind::Remote => ApiError::new(StatusCode::BAD_GATEWAY, "update", error.message),
        UpdateErrorKind::Local => ApiError::internal(error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apply_request_defaults_to_not_restarting() {
        let request: ApplyRequest = crate::routes::parse_body(&Default::default()).unwrap();
        assert!(!request.restart, "a plain POST must not restart the server");
        let request: ApplyRequest =
            serde_json::from_slice(br#"{"restart": true}"#).expect("restart parses");
        assert!(request.restart);
        assert!(
            serde_json::from_slice::<ApplyRequest>(br#"{"restart": "yes"}"#).is_err(),
            "a string is not a boolean"
        );
        assert!(
            serde_json::from_slice::<ApplyRequest>(br#"{"resart": true}"#).is_err(),
            "a typo must be rejected rather than silently ignored"
        );
    }

    #[test]
    fn error_kinds_map_to_the_documented_statuses() {
        let refused = map_error(UpdateError::refused("up to date"));
        assert_eq!(refused.status, StatusCode::CONFLICT);
        assert_eq!(refused.code, "bad_request");

        let remote = map_error(UpdateError::remote("manifest down"));
        assert_eq!(remote.status, StatusCode::BAD_GATEWAY);
        assert_eq!(remote.code, "update");

        let local = map_error(UpdateError::local("disk full"));
        assert_eq!(local.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(local.code, "internal");
    }
}
