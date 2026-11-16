//! `GET /api/commands` - the slash-command catalog the composer's `/` menu draws.
//!
//! The catalog is not a stored table: it is the built-in command list plus every
//! skill that can be typed as `/id`, discovered from disk on each request exactly
//! like `GET /api/skills`. Nothing caches it, so a skill file saved a second ago is
//! already in the menu and a deleted one is already gone.
//!
//! The menu is only a convenience. The server resolves and runs the command from
//! the message itself, so a client that never fetched this list - a script, a
//! control chat, `curl` - has exactly the same commands. The documentation topic
//! is `slash-commands`.

use crate::error::ApiResult;
use crate::routes::{query_str, QueryMap};
use crate::slash;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde_json::{json, Value};

/// `GET /api/commands[?project=p_...]`
///
/// The workspace decides which skills are listed, so `?project=` is meaningful;
/// an unknown project id is a `404` rather than a quietly default-workspace menu,
/// because a menu that offers a skill the agent cannot read is worse than none.
pub async fn list_commands(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let workspace = match query_str(&query, "project") {
        Some(project_id) => Some(state.workspace_for_project(Some(project_id))?),
        // No project: the install's own workspace if it has one. A server with
        // neither still answers, with the built-in commands and no skills.
        None => state.workspace_for_project(None).ok(),
    };
    let commands = slash::catalog(&state, workspace.as_deref()).await;
    Ok(Json(json!({ "commands": commands })))
}
