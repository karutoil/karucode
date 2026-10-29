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
//!
//! `GET /api/models/lookup?model=<id>` finds the best catalog entry for one
//! arbitrary model id, stripping gateway routing prefixes (`a6api/model`,
//! `a6api/upstream/model`) until a suffix matches. `match` is `null` when the
//! catalog simply does not carry the model — an ordinary result, not an error.

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

/// `GET /api/models/lookup?model=<id>` — metadata detection for one id.
///
/// Same availability contract as [`catalog`]: when the catalog cannot be
/// reached at all the answer is `200 {ok:false, match:null, error}` and the
/// caller falls back to adding the id without metadata.
pub async fn lookup(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let model = query_str(&query, "model")
        .ok_or_else(|| ApiError::bad_request("query parameter `model` is required"))?;
    let outcome = match state.catalog().get(false).await {
        Ok(outcome) => outcome,
        Err(error) => {
            return Ok(Json(json!({
                "ok": false,
                "match": Value::Null,
                "fetched_at_ms": 0,
                "stale": false,
                "error": state.redact(&error),
            })));
        }
    };
    let error = outcome.error.as_deref().map(|e| state.redact(e));
    let found = outcome.catalog.lookup(model);
    Ok(Json(json!({
        "ok": true,
        "match": match &found {
            Some(found) => json!({
                "provider": found.provider,
                "matched_id": found.matched_id,
                "model": found.model,
            }),
            None => Value::Null,
        },
        "fetched_at_ms": outcome.fetched_at_ms,
        "stale": outcome.stale,
        "error": error,
    })))
}
