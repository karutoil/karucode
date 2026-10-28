//! `GET /api/models/catalog` — models.dev metadata for the settings screen.
//!
//! Two shapes, because the full document is several megabytes:
//!
//! * no `provider`: the list of provider ids and names;
//! * with `provider=<id>`: that provider's models, with the fields the UI
//!   prefills (`name`, `context_window`, `max_output_tokens`, `tool_call`,
//!   `reasoning`, costs).
//!
//! `refresh=true` bypasses the cache. A catalog that cannot be reached answers
//! `200 {ok:false,...}` when there is nothing cached (the settings screen shows
//! the message and the user can still type a model id by hand), and a 404 when
//! the provider id itself is wrong.

use crate::error::{ApiError, ApiResult};
use crate::routes::{query_bool, query_str, QueryMap};
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde_json::{json, Value};

pub async fn catalog(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let refresh = query_bool(&query, "refresh")?;
    let outcome = match state.catalog().get(refresh).await {
        Ok(outcome) => outcome,
        Err(error) => {
            // The catalog is a convenience, never a requirement: report it in
            // the documented probe shape instead of failing the request.
            return Ok(Json(json!({
                "ok": false,
                "providers": [],
                "models": [],
                "fetched_at_ms": 0,
                "stale": false,
                "error": state.redact(&error),
            })));
        }
    };
    let error = outcome.error.as_deref().map(|e| state.redact(e));

    if let Some(provider_id) = query_str(&query, "provider") {
        let provider = outcome.catalog.provider(provider_id).ok_or_else(|| {
            ApiError::not_found(format!(
                "models.dev has no provider with id `{provider_id}`"
            ))
        })?;
        return Ok(Json(json!({
            "ok": true,
            "providers": [],
            "provider": {
                "id": provider.id,
                "name": provider.name,
                "model_count": provider.model_count,
            },
            "models": provider.models,
            "fetched_at_ms": outcome.fetched_at_ms,
            "stale": outcome.stale,
            "error": error,
        })));
    }

    let providers: Vec<Value> = outcome
        .catalog
        .providers()
        .map(|provider| {
            json!({
                "id": provider.id,
                "name": provider.name,
                "model_count": provider.model_count,
            })
        })
        .collect();
    Ok(Json(json!({
        "ok": true,
        "providers": providers,
        "models": [],
        "fetched_at_ms": outcome.fetched_at_ms,
        "stale": outcome.stale,
        "error": error,
    })))
}
