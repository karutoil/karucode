//! `/api/skills` - the skill library the agent loads on demand.
//!
//! A skill is a page of instructions (`<dir>/<name>/SKILL.md`, or a flat
//! `<name>.md`) that the agent opens only when a task matches its description.
//! The model, the scan and the file format live in [`kc_core::skill`]; this
//! module is the HTTP surface over it, and it has one rule that shapes every
//! handler: **there is no skills table**. The list is whatever
//! [`kc_core::skill::discover`] finds right now, so editing a file on disk shows
//! up on the next request without a restart and without a re-index.
//!
//! The config is the store. Two parts of `[skills]` are written here and nowhere
//! else:
//!
//! * `skills.custom` - the skills pasted in through `POST /api/skills`, kept in
//!   the settings table so they survive a restart with no file on disk;
//! * `skills.disabled` - the ids the user turned off, which hides a skill without
//!   deleting the file it came from.
//!
//! Both go through [`SettingsHandle::update_skills`], so a mutation respects
//! `KARUCODE_LOCK_SETTINGS` (`403`, the same message every other settings route
//! gives) and the admin gate when `KARUCODE_ADMIN_TOKEN` is set. The gate is on
//! the *mutations* only: a skill body is text that reaches the model's prompt, so
//! adding or rewriting one is a prompt change, while reading the list is the same
//! class of action as `GET /api/workspace/file`.
//!
//! # A skill that came from a file cannot be deleted or rewritten
//!
//! `DELETE` on a discovered skill is a `400`, and so is a `PATCH` that touches
//! its name, description or body, because the row in the settings table is not
//! where the text lives: deleting it would leave the file in place, and rewriting
//! it would be overwritten by the next scan. Both refusals name the file and point
//! at the two things that do work - edit the file, or turn the skill off with
//! `PATCH {"enabled": false}`.
//!
//! # `enabled` in the list payload is not a filter
//!
//! `GET /api/skills` always answers with what *exists*; the master switch is
//! reported separately as `"enabled": false` and each skill carries its own
//! on/off flag. Discovery is therefore run with the switch forced on: turning
//! skills off must not make the stored ones undeletable, and "what is on this
//! machine" does not change because a switch is off. The agent-facing behaviour
//! (no prompt block, a `skill` tool that says skills are disabled) is decided by
//! the real config, not by this payload.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_body, parse_required, query_str, QueryMap};
use crate::settings::{authorize_admin, ADMIN_TOKEN_HEADER};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use kc_core::paths::Workspace;
use kc_core::settings::Settings;
use kc_core::skill::{
    self, CustomSkill, Skill, SkillRegistry, SkillSource, SkillsConfig, CUSTOM_BODY_MAX_CHARS,
    DESCRIPTION_MAX_CHARS, NAME_MAX_CHARS, SKILL_BODY_MAX_BYTES,
};
use kc_core::Error;
use serde::Deserialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------------

/// `POST /api/skills`. `id` and `name` default to each other, and the id is
/// slugged, so `{"name": "Cut a release"}` becomes `cut-a-release`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Optional: without one the first line of the body becomes the description,
    /// exactly as it does for a skill read from a file. A skill with no
    /// description is invisible in the prompt, so silence is the wrong default.
    #[serde(default)]
    description: Option<String>,
    /// Required, non-empty, at most [`CUSTOM_BODY_MAX_CHARS`] characters.
    #[serde(default)]
    body: Option<String>,
}

/// `PATCH /api/skills/:id`. Every field is optional; `enabled` works for every
/// skill, the other three only for a hand-added one.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    enabled: Option<bool>,
    name: Option<String>,
    description: Option<String>,
    body: Option<String>,
}

impl UpdateBody {
    /// True when the request would rewrite text that belongs to a file.
    fn rewrites_content(&self) -> bool {
        self.name.is_some() || self.description.is_some() || self.body.is_some()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/skills` - the switches, the folders that were scanned and every
/// skill they yielded.
pub async fn list_skills(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let config = state.settings().skills().await;
    let registry = discover(&config, &workspace).await?;

    Ok(Json(json!({
        "enabled": config.enabled,
        "auto_detect": config.auto_detect,
        "directories": path_strings(&config),
        "prompt": config.prompt,
        "prompt_max_chars": config.prompt_max_chars,
        "max_skills": config.max_skills,
        // True when `skills.max_skills` cut the list: the UI says so rather than
        // pretending the tail does not exist.
        "truncated": registry.truncated,
        "roots": registry.roots.iter().map(|root| json!({
            "path": root.path.to_string_lossy(),
            "source": source_name(root.source),
            "exists": root.exists,
            "configured": root.configured,
            "count": root.count,
        })).collect::<Vec<_>>(),
        "skills": registry.skills.iter().map(skill_view).collect::<Vec<_>>(),
    })))
}

/// `GET /api/skills/:id` - one skill plus the body the agent would receive.
///
/// The body is frontmatter-stripped and bounded by [`SKILL_BODY_MAX_BYTES`];
/// `body_truncated` says whether the file was longer than that, so a caller can
/// tell a short skill from a cut one instead of assuming the page ends there.
pub async fn get_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let config = state.settings().skills().await;
    let registry = discover(&config, &workspace).await?;
    let skill = registry
        .get(&id)
        .ok_or_else(|| ApiError::not_found(unknown_skill(&id)))?;

    let body = registry.body(skill).map_err(|message| {
        ApiError::internal(format!("could not read skill `{}`: {message}", skill.id))
    })?;
    let body_truncated = skill
        .path
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len() > SKILL_BODY_MAX_BYTES as u64)
        .unwrap_or(false);

    let mut view = skill_view(skill);
    view["body"] = json!(body);
    view["body_truncated"] = json!(body_truncated);
    Ok(Json(view))
}

/// `POST /api/skills` - add a skill that lives in the settings table.
pub async fn create_skill(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    // Lock and admin gate before the body is parsed: a locked deployment answers
    // `403` even for a malformed body, exactly like `PATCH /api/config`.
    ensure_unlocked(&state)?;
    require_admin(&state, &headers, "adding a skill")?;
    let request: CreateBody = parse_required(&body, "POST /api/skills")?;

    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let config = state.settings().skills().await;
    let registry = discover(&config, &workspace).await?;

    let body_text = request.body.as_deref().unwrap_or_default();
    check_body(body_text, "body")?;

    let hint = request
        .id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(request.name.as_deref())
        .unwrap_or_default();
    let id = skill::slug(hint);
    if id.is_empty() {
        return Err(ApiError::bad_request(
            "a skill needs an `id` or a `name` with at least one letter or digit in it",
        ));
    }
    if id.chars().count() > NAME_MAX_CHARS {
        return Err(ApiError::bad_request(format!(
            "the skill id may be at most {NAME_MAX_CHARS} characters (got {})",
            id.chars().count()
        )));
    }
    // The id is unique across every source, so this catches a collision with a
    // file-backed skill too: the file is what the workspace declares, and a
    // custom skill of the same id would only be discovered and then dropped.
    if registry.get(&id).is_some() {
        return Err(exists(&id));
    }
    if let Some(name) = request.name.as_deref() {
        if name.trim().chars().count() > NAME_MAX_CHARS {
            return Err(ApiError::bad_request(format!(
                "a skill name may be at most {NAME_MAX_CHARS} characters"
            )));
        }
    }

    let created = CustomSkill {
        name: request
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| id.clone()),
        description: describe(request.description.as_deref(), body_text),
        body: body_text.trim().to_string(),
        id: id.clone(),
    };
    let stored = created.clone();
    let stored_id = stored.id.clone();

    let updated = state
        .settings()
        .update_skills(move |skills| {
            // Re-checked under the update lock: two simultaneous POSTs of one id
            // would otherwise both pass the pre-check and have one row silently
            // dropped by `normalize`.
            if skills
                .custom
                .iter()
                .any(|skill| skill.id.as_str() == stored_id.as_str())
            {
                return Err(Error::BadRequest(format!(
                    "a skill with id `{stored_id}` already exists; PATCH /api/skills/{stored_id} to change it"
                )));
            }
            skills.custom.push(stored);
            // A skill nobody has seen before is on: an id left in `disabled` by
            // an earlier delete-then-recreate must not resurrect it as off.
            skills.disabled.retain(|disabled| disabled != &stored_id);
            Ok(())
        })
        .await
        .map_err(ApiError::from)?;

    let registry = discover(&updated, &workspace).await?;
    let skill = registry.get(&id).ok_or_else(|| {
        ApiError::internal(format!(
            "the skill `{id}` was stored but discovery did not return it"
        ))
    })?;
    Ok((StatusCode::CREATED, Json(skill_view(skill))))
}

/// `PATCH /api/skills/:id` - turn any skill on or off, rewrite a hand-added one.
pub async fn patch_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    ensure_unlocked(&state)?;
    require_admin(&state, &headers, "changing a skill")?;
    let request: UpdateBody = parse_body(&body)?;

    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let config = state.settings().skills().await;
    let registry = discover(&config, &workspace).await?;
    let found = registry
        .get(&id)
        .ok_or_else(|| ApiError::not_found(unknown_skill(&id)))?;
    let skill_id = found.id.clone();

    if request.rewrites_content() {
        // Naming the file matters more than naming the rule: the operator has to
        // know *which* SKILL.md to open.
        let location = found
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "a file".to_string());
        if found.source.is_file() {
            return Err(ApiError::bad_request(format!(
                "the skill `{skill_id}` is read from {location}: edit that file, or turn the skill \
                 off with {{\"enabled\": false}} - the server does not own its text"
            )));
        }
        if let Some(text) = request.body.as_deref() {
            check_body(text, "body")?;
        }
        if let Some(name) = request.name.as_deref() {
            if name.trim().is_empty() {
                return Err(ApiError::bad_request("`name` may not be empty"));
            }
            if name.trim().chars().count() > NAME_MAX_CHARS {
                return Err(ApiError::bad_request(format!(
                    "a skill name may be at most {NAME_MAX_CHARS} characters"
                )));
            }
        }
    }

    let updated = state
        .settings()
        .update_skills(|skills| {
            if let Some(enabled) = request.enabled {
                if enabled {
                    skills.disabled.retain(|disabled| disabled != &skill_id);
                } else if !skills.disabled.contains(&skill_id) {
                    skills.disabled.push(skill_id.clone());
                }
            }
            if request.rewrites_content() {
                // The pre-checks above ran outside the lock; this is where the
                // text actually changes, and a stored skill that vanished in
                // between is a 404 rather than a silent no-op.
                let Some(custom) = skills
                    .custom
                    .iter_mut()
                    .find(|custom| custom.id == skill_id)
                else {
                    return Err(Error::NotFound(format!(
                        "the stored skill `{skill_id}` no longer exists"
                    )));
                };
                if let Some(name) = request.name.as_deref() {
                    custom.name = name.trim().to_string();
                }
                if let Some(description) = request.description.as_deref() {
                    custom.description = description.trim().to_string();
                }
                if let Some(text) = request.body.as_deref() {
                    custom.body = text.trim().to_string();
                }
                // A description the patch blanked out falls back to the body's
                // first line again, so clearing it cannot make the skill
                // invisible in the prompt.
                if custom.description.is_empty() {
                    custom.description = describe(None, &custom.body);
                }
            }
            Ok(())
        })
        .await
        .map_err(ApiError::from)?;

    let registry = discover(&updated, &workspace).await?;
    let skill = registry
        .get(&skill_id)
        .ok_or_else(|| ApiError::not_found(unknown_skill(&skill_id)))?;
    Ok(Json(skill_view(skill)))
}

/// `DELETE /api/skills/:id` - remove a hand-added skill. A file-backed one is
/// refused, because the row is not what holds its text.
pub async fn delete_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    ensure_unlocked(&state)?;
    require_admin(&state, &headers, "deleting a skill")?;

    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let config = state.settings().skills().await;
    let registry = discover(&config, &workspace).await?;
    let found = registry
        .get(&id)
        .ok_or_else(|| ApiError::not_found(unknown_skill(&id)))?;
    let skill_id = found.id.clone();

    if found.source.is_file() {
        let location = found
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "a file".to_string());
        return Err(ApiError::bad_request(format!(
            "the skill `{skill_id}` comes from {location}, which this route cannot delete: delete \
             that file, or turn the skill off with PATCH /api/skills/{skill_id} \
             {{\"enabled\": false}}"
        )));
    }

    state
        .settings()
        .update_skills(|skills| {
            skills.custom.retain(|custom| custom.id != skill_id);
            // Drop the switch too: a skill added again under this id should start
            // enabled rather than inherit a stale off.
            skills.disabled.retain(|disabled| disabled != &skill_id);
            Ok(())
        })
        .await
        .map_err(ApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run discovery off the async runtime: it is filesystem work, and a skills
/// folder can sit on a slow mount.
async fn discover(config: &SkillsConfig, workspace: &Workspace) -> ApiResult<SkillRegistry> {
    // Forced on, deliberately: this route answers "what skills exist", which the
    // master switch does not change - it only decides whether they are used. See
    // the module documentation.
    let config = SkillsConfig {
        enabled: true,
        ..config.clone()
    };
    let root = workspace.root().to_path_buf();
    tokio::task::spawn_blocking(move || skill::discover(&config, &root))
        .await
        .map_err(|e| ApiError::internal(format!("the skills scan failed: {e}")))
}

/// One skill in the documented wire shape.
fn skill_view(skill: &Skill) -> Value {
    json!({
        "id": skill.id,
        "name": skill.name,
        "description": skill.description,
        "source": source_name(skill.source),
        "path": skill.path.as_ref().map(|path| path.to_string_lossy()),
        "enabled": skill.enabled,
        "custom": skill.source == SkillSource::Custom,
    })
}

/// `SkillSource`'s own (serde) spelling: `workspace | user | config | custom`.
///
/// Serialised rather than hand-mapped so the API cannot drift from the enum:
/// [`SkillSource::label`] is the *prompt/tool* wording (`configured`, `added`)
/// and is not the wire spelling, and hand-writing the four words here would be a
/// second source of truth that a future variant silently escapes.
fn source_name(source: SkillSource) -> String {
    match serde_json::to_value(source) {
        Ok(Value::String(name)) => name,
        // Unreachable for a unit-variant enum; falling back to the debug name
        // still tells a reader what they are looking at.
        _ => format!("{source:?}").to_ascii_lowercase(),
    }
}

fn path_strings(config: &SkillsConfig) -> Vec<String> {
    config
        .directories
        .iter()
        .map(|dir| dir.to_string_lossy().to_string())
        .collect()
}

/// The description to store: the one given, or the body's first real line.
fn describe(description: Option<&str>, body: &str) -> String {
    let given = description.map(str::trim).unwrap_or_default();
    let text = if given.is_empty() {
        body.lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .unwrap_or_default()
    } else {
        given
    };
    // `SkillsConfig::normalize` cuts this to `DESCRIPTION_MAX_CHARS` as well; the
    // cut here keeps the value that is echoed back already the stored one.
    text.chars().take(DESCRIPTION_MAX_CHARS).collect()
}

/// Validate a hand-added body: present, non-blank, not a book.
fn check_body(text: &str, field: &str) -> ApiResult<()> {
    if text.trim().is_empty() {
        return Err(ApiError::bad_request(format!(
            "`{field}` may not be empty: a skill is the text the agent reads"
        )));
    }
    if text.chars().count() > CUSTOM_BODY_MAX_CHARS {
        return Err(ApiError::bad_request(format!(
            "`{field}` may be at most {CUSTOM_BODY_MAX_CHARS} characters (got {})",
            text.chars().count()
        )));
    }
    Ok(())
}

fn exists(id: &str) -> ApiError {
    ApiError::conflict(format!(
        "a skill with id `{id}` already exists; PATCH /api/skills/{id} to change it"
    ))
}

fn unknown_skill(id: &str) -> String {
    format!(
        "no skill with id `{}`: GET /api/skills lists the ids this workspace and machine have",
        skill::slug(id)
    )
}

/// Refuse a write to a locked deployment, with the wording every other settings
/// route uses.
fn ensure_unlocked(state: &AppState) -> ApiResult<()> {
    if state.settings().is_locked() {
        return Err(ApiError::forbidden(
            "settings are locked (KARUCODE_LOCK_SETTINGS); change them in the config file or environment",
        ));
    }
    Ok(())
}

/// The admin gate `PATCH /api/config` applies, for a change that reaches the
/// model's prompt without going through a [`ConfigPatch`].
fn require_admin(state: &AppState, headers: &HeaderMap, what: &str) -> ApiResult<()> {
    let config = state.settings().snapshot();
    let presented = headers
        .get(ADMIN_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    authorize_admin(&config, presented, what).map_err(ApiError::from)
}
