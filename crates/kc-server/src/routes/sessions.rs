//! Session CRUD, message submission and cancellation.
//
// @kc-doc: ephemeral-sessions
// @kc-title: Run a temporary chat
// @kc-summary: Create an ephemeral session for a program: it never appears in a session list and deletes itself once its work is over.
// @kc-tags: sessions, temporary, ephemeral, api, integration, chat
// @kc-applies-to: route:POST /api/sessions, route:DELETE /api/sessions, route:POST /api/sessions/:id/expire, route:GET /api/sessions, field:ephemeral, event:session_deleted
//
// # Temporary sessions
//
// A session created with `"ephemeral": true` is a chat for a program, not a
// person: a third-party tool that links karucode for one question, one audit or
// one task and does not want its work to litter the session rail.
//
// ```bash
// curl -s -X POST localhost:8787/api/sessions \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"ephemeral": true, "project": "p_...", "title": "audit"}'
// ```
//
// Every ordinary verb works on it: post to `/api/sessions/:id/messages`, poll
// `GET /api/sessions/:id`, steer, queue, cancel, rewind. What changes is
// where the row appears and when it dies.
//
// # Never listed
//
// `GET /api/sessions` (with or without `?project=`), the control plane's
// session lists and `GET /api/control/overview` all exclude ephemeral
// sessions, permanently. The browser rail never shows one. A client that
// created one tracks it by its id - the session id is the only handle, so keep
// it; ids are random, which is what keeps someone else's ephemeral chat
// unguessable.
//
// # When it disappears
//
// An ephemeral session is deleted the moment its work is over. "Over" means:
// no run in flight (including the run that just finished), nothing queued
// (neither steers nor follow-ups), no live subagent, and no approval waiting
// for an answer. The finishing run's own teardown performs the delete, so a
// chain of follow-ups cleans up after the *last* one; a chat that ended with a
// question stops at the approval and stays until the answer (which starts a
// run) leads to a quiet finish, or you retire it early:
//
// ```bash
// curl -s -X POST localhost:8787/api/sessions/s_abc/expire \
//   -H 'X-Karucode-Client: web'
// ```
//
// `expire` refuses a durable session (that is what `DELETE` is for) and
// refuses an ephemeral one while input is still queued - recall the queued
// message or let it drain. `DELETE /api/sessions/:id` also works on an
// ephemeral session and cancels any run in flight first, exactly like for a
// durable one.
//
// Two more sweeps keep no row behind:
//
// * `DELETE /api/sessions` (no id) retires every **idle** ephemeral session at
//   once and answers `{"deleted": N, "sessions": [...]}`; busy ones are left
//   for their runs to finish. It never touches durable sessions.
// * At startup, after interrupted runs are marked, ephemeral sessions with no
//   work left are swept - a restart is not a way to keep one alive, and a
//   program whose chat died with the process gets a `404` on its next poll
//   and creates a fresh one, exactly as it would had the run finished.
//
// The delete publishes `session_deleted` on `/api/events`, the same event an
// ordinary delete sends, so a listening client can drop its row without
// polling. Deletion removes the transcript, runs, agents and approvals with
// the row (`ON DELETE CASCADE`); there is no archive.
//
// # What is deliberately the same
//
// * **Titles are skipped.** Nothing names an ephemeral session - neither the
//   deterministic derivation nor the model-written title - so no background
//   titling request can race the delete. The `title` you pass is stored, and
//   is only ever a label for your own bookkeeping.
// * **Approvals still work.** A run that asks permission becomes pending, the
//   session survives until it is answered, and the answer's run gets the same
//   end-of-life treatment. Auto-expiry never fires while an approval is open.
// * **Model pins still work.** `model` at creation and
//   `PATCH /api/sessions/:id/model` behave exactly as for a durable session;
//   the pin dies with the row.

use crate::error::{ApiError, ApiResult};
use crate::routes::{
    message_before, parse_required, query_str, query_usize, QueryMap, TRANSCRIPT_PAGE_BYTES,
};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kc_core::config::SessionModelSelection;
use kc_core::events::Event;
use kc_core::ids::SessionId;
use kc_core::store::{MessageScope, Session};
use serde::Deserialize;
use serde_json::{json, Value};

/// Bounded prompt size: the UI sends text, not files. 1 MiB is far above any
/// real prompt and keeps a single request from exhausting memory.
const MAX_PROMPT_BYTES: usize = 1024 * 1024;
const MAX_TITLE_CHARS: usize = 200;
/// Title `POST /api/sessions` assigns when the caller supplies no title. A
/// session still carrying it is treated as untitled.
pub const DEFAULT_SESSION_TITLE: &str = "New session";
/// Auto-derived titles are cut here (plus `…`) so the session rail stays
/// readable at narrow widths.
const MAX_AUTO_TITLE_CHARS: usize = 60;
/// History returned by `GET /api/sessions/:id` (tail, most recent last).
const MESSAGE_LIMIT: usize = 5000;
/// Upper bound for `?messages=`, so one request cannot ask for a 100k-message
/// transcript (tens of megabytes of JSON). Older history beyond the returned
/// window is in the database and reachable with a larger `messages`.
const MESSAGE_LIMIT_MAX: usize = 50_000;
const RUN_LIMIT: usize = 200;
/// Ceiling for one inline attachment's text on a queued message.
///
/// The composer inlines text files up to 64 KiB (`INLINE_MAX_BYTES` in
/// `web/src/lib/attachments.ts`); this sits above that so a legitimate chip is
/// never refused, and below `MAX_PROMPT_BYTES` so a queue of sixteen of them
/// still fits in one request.
const MAX_ATTACHMENT_INLINE_BYTES: usize = 128 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleBody {
    #[serde(default)]
    title: Option<String>,
    /// Project to create the session in. Omitted means the default workspace
    /// (and no project), which is what an older client sends.
    #[serde(default)]
    project: Option<String>,
    /// Per-session model selection. Absent leaves the session's selection
    /// alone; `null` clears it (the session follows the install-wide default);
    /// an object pins this session to that profile + model.
    #[serde(default, deserialize_with = "double_option")]
    model: Option<Option<SessionModelBody>>,
    /// Create a **temporary** session: excluded from every listing, never
    /// auto-titled, and deleted automatically once its work is over (see
    /// `ephemeral-sessions`). Absent (`false`) is the durable chat every
    /// existing client creates.
    #[serde(default)]
    ephemeral: bool,
}

/// One session's model pin, as sent by the browser.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionModelBody {
    provider: String,
    model: String,
    /// `minimal|low|medium|high`; omitted leaves the endpoint default.
    #[serde(default)]
    reasoning_effort: Option<String>,
}

impl SessionModelBody {
    fn to_selection(&self) -> SessionModelSelection {
        SessionModelSelection {
            provider: self.provider.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
        }
        .normalized()
    }
}

/// Body of `PATCH /api/sessions/:id/model`.
///
/// The browser sends the selection itself, and `{}` (or `null` fields) means
/// "clear the pin and follow the install-wide default again". A half-filled
/// selection is rejected by [`SessionModelSelection::validate_shape`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionModelPinBody {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
}

impl SessionModelPinBody {
    fn into_selection(self) -> Option<SessionModelSelection> {
        if self.provider.is_none() && self.model.is_none() && self.reasoning_effort.is_none() {
            return None;
        }
        Some(
            SessionModelSelection {
                provider: self.provider.unwrap_or_default(),
                model: self.model.unwrap_or_default(),
                reasoning_effort: self.reasoning_effort,
            }
            .normalized(),
        )
    }
}

/// Distinguishes "field absent" (`None`) from "present and null" (`Some(None)`),
/// which is how a PATCH clears a session's model pin.
fn double_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    #[serde(default)]
    content: Option<String>,
    /// The text the user typed, when `content` is the rendered form. Sent for
    /// `mode=steer`/`mode=follow_up` so a recall can restore the draft without
    /// the attachment blocks pasted in. Ignored by `mode=send`, whose content is
    /// already what the transcript shows.
    #[serde(default)]
    draft: Option<String>,
    /// Chips to show when this message is recalled. They describe files whose
    /// bytes are already inside `content`, so they are a display record, not a
    /// second copy of the payload.
    #[serde(default)]
    attachments: Vec<AttachmentBody>,
}

/// One attachment on a queued message.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachmentBody {
    name: String,
    path: String,
    /// `inline` or `upload`; anything else is a `400 bad_request`.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    /// The inlined text of an `inline` attachment.
    ///
    /// It is already inside `content`, which is what the model sees; keeping it
    /// on the chip is what lets a recall put the file back in the composer, since
    /// the browser's copy of a local file is the only one that exists.
    #[serde(default)]
    content: Option<String>,
}

impl AttachmentBody {
    fn into_queued(self) -> ApiResult<kc_core::queue::QueuedAttachment> {
        if self.name.trim().is_empty() {
            return Err(ApiError::bad_request(
                "an attachment requires a non-empty `name`",
            ));
        }
        if self.path.trim().is_empty() {
            return Err(ApiError::bad_request(
                "an attachment requires a non-empty `path`",
            ));
        }
        let mode = match self.mode.as_deref().map(str::trim) {
            None | Some("") | Some("upload") => kc_core::queue::AttachmentMode::Upload,
            Some("inline") => kc_core::queue::AttachmentMode::Inline,
            Some(other) => {
                return Err(ApiError::bad_request(format!(
                    "attachment `mode` must be `inline` or `upload` (got `{other}`)"
                )))
            }
        };
        // An uploaded file's body is on disk at `path`; accepting a copy would let
        // a client park bytes in memory that no recall needs.
        let content = match mode {
            kc_core::queue::AttachmentMode::Inline => self.content,
            kc_core::queue::AttachmentMode::Upload => {
                if self.content.is_some() {
                    return Err(ApiError::bad_request(
                        "an `upload` attachment may not carry `content`: the file is at `path`",
                    ));
                }
                None
            }
        };
        if let Some(content) = &content {
            if content.len() > MAX_ATTACHMENT_INLINE_BYTES {
                return Err(ApiError::bad_request(format!(
                    "attachment `content` is too large ({} bytes, maximum {MAX_ATTACHMENT_INLINE_BYTES})",
                    content.len()
                )));
            }
        }
        Ok(kc_core::queue::QueuedAttachment {
            name: self.name,
            path: self.path,
            mode,
            size: self.size.unwrap_or(0),
            content,
        })
    }
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let sessions = match query_str(&query, "project") {
        Some(project_id) => {
            if state.store().get_project(project_id).await?.is_none() {
                return Err(ApiError::not_found(format!(
                    "no project with id `{project_id}`"
                )));
            }
            state.store().list_sessions_for_project(project_id).await?
        }
        None => state.store().list_sessions().await?,
    };
    Ok(Json(json!({ "sessions": sessions })))
}

pub async fn create_session(State(state): State<AppState>, body: Bytes) -> ApiResult<Json<Value>> {
    let body: TitleBody = crate::routes::parse_body(&body)?;
    let title = normalize_title(body.title)?;
    // Validate the pin before creating anything, so a bad request cannot leave a
    // half-created session behind.
    let selection = match body.model.as_ref() {
        Some(Some(model)) => {
            let selection = model.to_selection();
            crate::settings::validate_session_model(&state.settings().snapshot(), &selection)
                .map_err(ApiError::from)?;
            Some(selection)
        }
        _ => None,
    };
    let id = SessionId::new();
    let workspace = state.workspace_for_project(body.project.as_deref())?;
    let created = state
        .store()
        .create_session(
            id.as_str(),
            &title,
            &workspace.root().to_string_lossy(),
            body.project.as_deref(),
            body.ephemeral,
        )
        .await?;
    let session = match selection {
        Some(selection) => {
            state
                .store()
                .set_session_model(id.as_str(), Some(&selection))
                .await?;
            session_or_404(&state, id.as_str()).await?
        }
        None => created,
    };
    // Runtime-stamped so the SSE `seq` stays monotonic for all clients.
    state.runtime().emit(
        Some(session.id.as_str()),
        None,
        None,
        Event::SessionCreated {
            session_id: session.id.clone(),
            title: session.title.clone(),
        },
    );
    if let Some(model) = &session.model {
        state.runtime().emit(
            Some(session.id.as_str()),
            None,
            None,
            Event::SessionModelChanged {
                session_id: session.id.clone(),
                model: Some(model.clone()),
            },
        );
    }
    Ok(Json(
        serde_json::to_value(session).map_err(|e| ApiError::internal(e.to_string()))?,
    ))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &id).await?;
    let store = state.store();
    // A long session (thousands of tool calls) has more messages than the
    // default window, and the window is the *newest* messages: without a signal
    // the browser would silently show a transcript whose beginning is missing.
    let limit = query_usize(&query, "messages")?.unwrap_or(MESSAGE_LIMIT);
    let limit = limit.clamp(1, MESSAGE_LIMIT_MAX);
    let page = store
        .list_message_page(
            &session.id,
            MessageScope::Session,
            message_before(&query)?,
            limit,
            TRANSCRIPT_PAGE_BYTES,
        )
        .await?;
    let messages_truncated = page.has_more;
    let messages = page.messages;
    let messages_before = messages_truncated
        .then(|| messages.first().map(|m| m.id))
        .flatten();
    let runs = store.list_runs(&session.id, RUN_LIMIT).await?;
    let agents = store.list_agents(&session.id).await?;
    let approvals = store.list_pending_approvals(Some(&session.id)).await?;
    let claims = store.list_claims(&session.id).await?;
    let running = state.runtime().has_active_run(&session.id);
    // When this session was last rewound, so the UI can offer "undo is not
    // available" honestly instead of implying the transcript is whole.
    let last_rewind_at = store.last_rewind_at(&session.id).await?;
    // Messages the user queued while a run was in flight. Returned with the
    // session so a reload (or a second browser) shows the same pending work
    // instead of silently losing it.
    let queued_steers: Vec<Value> = state
        .runtime()
        .queued_steers(&session.id)
        .iter()
        .map(kc_runtime::QueuedSteer::public_view)
        .collect();
    let queued_follow_ups: Vec<Value> = state
        .runtime()
        .queued_follow_ups(&session.id)
        .iter()
        .map(kc_runtime::QueuedFollowUp::public_view)
        .collect();
    Ok(Json(json!({
        "session": session,
        "messages": messages,
        "messages_truncated": messages_truncated,
        "messages_limit": limit,
        "messages_before": messages_before,
        "runs": runs,
        "agents": agents,
        "approvals": approvals,
        "claims": claims,
        "running": running,
        "queued_steers": queued_steers,
        "queued_follow_ups": queued_follow_ups,
        "last_rewind_at": last_rewind_at,
    })))
}

pub async fn rename_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    let body: TitleBody = parse_required(&body, "PATCH /api/sessions/:id")?;
    let title = match body.title {
        Some(raw) => Some(normalize_title(Some(raw))?),
        None => None,
    };
    if title.is_none() && body.model.is_none() {
        return Err(ApiError::bad_request(
            "title or model is required (send `model: null` to follow the default model)",
        ));
    }

    if let Some(title) = &title {
        rename_session_and_emit(&state, &id, title).await?;
    }

    // `Some(Some(..))` pins the session, `Some(None)` clears the pin.
    let changed_model = match body.model {
        Some(Some(model)) => {
            let selection = model.to_selection();
            crate::settings::validate_session_model(&state.settings().snapshot(), &selection)
                .map_err(ApiError::from)?;
            state
                .store()
                .set_session_model(&id, Some(&selection))
                .await?;
            Some(Some(selection))
        }
        Some(None) => {
            state.store().set_session_model(&id, None).await?;
            Some(None)
        }
        None => None,
    };

    let session = session_or_404(&state, &id).await?;
    if let Some(model) = changed_model {
        state.runtime().emit(
            Some(&id),
            None,
            None,
            Event::SessionModelChanged {
                session_id: id.clone(),
                model,
            },
        );
    }
    Ok(Json(
        serde_json::to_value(session).map_err(|e| ApiError::internal(e.to_string()))?,
    ))
}

/// Delete a session exactly as the routes do: stop any in-flight work, drop the
/// row (children cascade), release per-session approvals, sweep orphaned file
/// before-images, and publish `session_deleted` so every client drops the row.
///
/// One body for the callers that end a session on request - the ordinary
/// `DELETE /api/sessions/:id`, `POST /api/sessions/:id/expire` (a temporary
/// session the caller finished with early), and the collection sweep - so none
/// of them can drift on the teardown order. (The run-teardown auto-expiry uses
/// the runtime's own retire path, which is the same sequence without the
/// HTTP-facing checks.) Returns `false` when the row vanished first (another
/// tab deleted it); the caller decides whether that is an error.
pub(crate) async fn delete_session_rows(state: &AppState, id: &str) -> ApiResult<bool> {
    // Stop any in-flight work before dropping its rows so the run loop cannot
    // resurrect a deleted session.
    let existed = state.store().get_session(id).await?.is_some();
    if !existed {
        return Ok(false);
    }
    state.runtime().cancel(id);
    state.store().delete_session(id).await?;
    state.runtime().clear_session_allows(id);
    state.runtime().emit(
        Some(id),
        None,
        None,
        Event::SessionDeleted {
            session_id: id.to_string(),
        },
    );
    Ok(true)
}

pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    delete_session_rows(&state, &id).await?;
    Ok(Json(json!({ "deleted": true })))
}

/// `POST /api/sessions/:id/expire` — retire a session now.
///
/// For a temporary session this is the platform's "I am done with this chat"
/// switch: it runs the same teardown an automatic expiry would, so the
/// transcript, runs and agents go with the row and every listener sees
/// `session_deleted`. Calling it on a durable session is refused (`403`-shaped
/// `bad_request`): durability is the user's decision, not a client's, and a
/// tool that silently deleted real chats would be a footgun. An in-flight run
/// is cancelled first, exactly like `DELETE`.
pub async fn expire_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &id).await?;
    if !session.ephemeral {
        return Err(ApiError::bad_request(
            "session is not ephemeral; use DELETE to remove it",
        ));
    }
    let queued = state.runtime().queued_all(&id);
    if !queued.steers.is_empty() || !queued.follow_ups.is_empty() {
        return Err(ApiError::conflict(
            "session still has queued input; recall it (DELETE /api/sessions/:id/queues/:queued_id) \
             or let it drain before expiring",
        ));
    }
    delete_session_rows(&state, &id).await?;
    Ok(Json(json!({ "expired": true, "session_id": id })))
}

/// `DELETE /api/sessions` — sweep every idle temporary session.
///
/// Not a bulk delete of durable chats: those are the user's records and stay.
/// The sweep is the "platform shut down / clean up" call: it retires every
/// temporary session that has nothing left in flight (a running one, or one
/// with queued input, is left for its run to finish and expire on its own) and
/// reports which ids went, so the caller can tell its own bookkeeping.
///
/// Deletion goes through the runtime's retire path rather than the route's
/// delete helper: the sweep must not cancel live work (that is what "idle"
/// means), and the retire path is the one place that knows all four meanings
/// of "still busy" - registered loops, both queues, live agents, pending
/// approvals.
pub async fn sweep_ephemeral_sessions(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let candidates = state.store().idle_ephemeral_session_ids().await?;
    let mut expired = Vec::with_capacity(candidates.len());
    for id in candidates {
        // The runtime re-checks everything (including the in-memory queues the
        // SQL cannot see) and emits `session_deleted` itself.
        if state.runtime().retire_idle_ephemeral_session(&id).await {
            expired.push(id);
        }
    }
    Ok(Json(json!({
        "deleted": expired.len(),
        "sessions": expired,
    })))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Response> {
    let session = session_or_404(&state, &id).await?;
    let mode = MessageMode::parse(query_str(&query, "mode"))?;
    let body: MessageBody = parse_required(&body, "POST /api/sessions/:id/messages")?;
    let content = body
        .content
        .clone()
        .ok_or_else(|| ApiError::bad_request("content is required"))?;
    validate_prompt(&content)?;
    // A queued message keeps the chips the composer drew, so a recall puts the
    // form back exactly as it was. `mode=send` has nowhere to put them (the
    // transcript shows the rendered content), so they are accepted and ignored
    // rather than rejected - a client that always sends them must not fail a
    // plain send.
    let attachments = body
        .attachments
        .into_iter()
        .map(AttachmentBody::into_queued)
        .collect::<ApiResult<Vec<_>>>()?;
    let queued_input =
        kc_runtime::QueuedInput::with_attachments(content.clone(), body.draft, attachments);

    match mode {
        MessageMode::Steer => {
            let steer = state
                .runtime()
                .steer(&id, queued_input)
                .map_err(steer_error)?;
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "mode": "steer",
                    "session_id": id,
                    "steer_id": steer.id,
                    "queued": true,
                    "queued_message": steer.public_view(),
                })),
            )
                .into_response())
        }
        MessageMode::FollowUp => {
            let follow_up = state
                .runtime()
                .enqueue_follow_up(&id, queued_input)
                .map_err(steer_error)?;
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "mode": "follow_up",
                    "session_id": id,
                    "follow_up_id": follow_up.id,
                    "queued": true,
                    "queued_message": follow_up.public_view(),
                })),
            )
                .into_response())
        }
        MessageMode::Send => {
            // Cheap up-front check gives the documented 409 instead of the
            // runtime's generic "already active" bad_request. `submit` still
            // re-checks under its lock, so the race window cannot start two runs.
            // `has_active_run`, not `is_running`: a detached subagent keeps a
            // loop registered for this session, and refusing every prompt for as
            // long as one runs would make background work unusable.
            if state.runtime().has_active_run(&id) {
                return Err(ApiError::conflict(
                    "a run is already active for this session; send `?mode=steer` or \
                     `?mode=follow_up` to queue this message instead, or cancel the run",
                ));
            }
            // The first user message titles an untitled session: immediately with
            // a derived title (so the rail is never empty), then - if `[titles]`
            // is on, which it is by default - with a short model-written one in
            // the background. `expected` is `None` for a session the user already
            // named, which is what disables the model title for it.
            let expected = maybe_auto_title(&state, &session, &content).await;
            let handle = state
                .runtime()
                .submit(&id, content.clone())
                .await
                .map_err(|error| match error {
                    kc_core::Error::BadRequest(message) if message.contains("already active") => {
                        ApiError::conflict(message)
                    }
                    other => ApiError::from(other),
                })?;

            // The AI title is a second, optional request: spawned so the run is
            // never delayed by it. `expected` is the title the session carries
            // right now, which is what the generated title must still replace for
            // the rename to be honest about who set it.
            if let Some(expected) = expected {
                spawn_ai_title(state.clone(), id.clone(), expected, content);
            }

            Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "mode": "send",
                    "run_id": handle.run_id,
                    "session_id": handle.session_id,
                })),
            )
                .into_response())
        }
    }
}

/// How a message relates to the run that may or may not be in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageMode {
    /// Start a run. The historical behaviour, and the default.
    Send,
    /// Fold this message into the *running* conversation at the next iteration.
    Steer,
    /// Start a new run once the current one finishes.
    FollowUp,
}

impl MessageMode {
    fn parse(raw: Option<&str>) -> ApiResult<Self> {
        match raw.map(str::trim) {
            None | Some("") | Some("send") => Ok(MessageMode::Send),
            Some("steer") => Ok(MessageMode::Steer),
            Some(value)
                if value.eq_ignore_ascii_case("follow_up")
                    || value.eq_ignore_ascii_case("follow-up") =>
            {
                Ok(MessageMode::FollowUp)
            }
            Some(other) => Err(ApiError::bad_request(format!(
                "query parameter `mode` must be `send`, `steer` or `follow_up` (got `{other}`)"
            ))),
        }
    }
}

/// Shared prompt validation for all three modes: the same message travels
/// through each, so the same limits apply.
fn validate_prompt(content: &str) -> ApiResult<()> {
    if content.trim().is_empty() {
        return Err(ApiError::bad_request("content may not be empty"));
    }
    if content.len() > MAX_PROMPT_BYTES {
        return Err(ApiError::bad_request(format!(
            "content is too large ({} bytes, maximum {MAX_PROMPT_BYTES})",
            content.len()
        )));
    }
    if content.contains('\0') {
        return Err(ApiError::bad_request("content may not contain NUL bytes"));
    }
    Ok(())
}

/// Map a queue rejection from the runtime to its documented status.
///
/// "No run in flight" is a `409` (the request is well-formed but the state is
/// wrong for this mode), and a full queue is a `409` too. Everything else is the
/// runtime's own classification.
fn steer_error(error: kc_core::Error) -> ApiError {
    match error {
        kc_core::Error::BadRequest(message) => ApiError::conflict(message),
        other => ApiError::from(other),
    }
}

/// `GET /api/sessions/:id/queues` — the messages waiting for this session's run.
///
/// The same pair `GET /api/sessions/:id` carries, without the transcript: the
/// composer's pending panel polls nothing (it follows the `queued_messages`
/// event), but a client that cannot hold an event stream open, and every client
/// that wants one cheap read on mount, can ask here.
pub async fn list_queues(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = session_or_404(&state, &id).await?;
    let snapshot = state.runtime().queued_all(&session.id);
    Ok(Json(json!({
        "session_id": session.id,
        "running": state.runtime().has_active_run(&session.id),
        "steers": snapshot
            .steers
            .iter()
            .map(kc_runtime::QueuedSteer::public_view)
            .collect::<Vec<_>>(),
        "follow_ups": snapshot
            .follow_ups
            .iter()
            .map(kc_runtime::QueuedFollowUp::public_view)
            .collect::<Vec<_>>(),
    })))
}

/// `DELETE /api/sessions/:id/queues/:queued_id` — recall a queued message.
///
/// The message never reaches the model and the caller gets it back whole: the
/// draft to put in the composer and the chips to redraw. An id that is already
/// gone (it landed, or another tab recalled it first) is the documented
/// `404 not_found`; the browser then refreshes rather than reviving a message
/// the run already applied.
pub async fn recall_queued(
    State(state): State<AppState>,
    Path((id, queued_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    let recalled = state
        .runtime()
        .recall_queued(&id, &queued_id)
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "session_id": id,
        "recalled": recalled.public_view(),
    })))
}

/// Ask a model for a session title, in the background.
///
/// Fire-and-forget by construction: the task cannot report an error to the
/// client (it outlives the request) and does not need to, because the deterministic
/// title is already stored. Spawning rather than awaiting is the whole point -
/// the run must start immediately, and a titling request that hangs for its
/// `timeout_secs` would otherwise hold the composer.
fn spawn_ai_title(state: AppState, session_id: String, expected_title: String, prompt: String) {
    if !state.settings().snapshot().titles.enabled {
        return;
    }
    tokio::spawn(async move {
        crate::titles::maybe_auto_title(&state, &session_id, &expected_title, &prompt).await;
    });
}

/// `PATCH /api/sessions/:id/model` — pin this session to a provider + model, or
/// clear the pin with an empty body so it follows the install-wide default.
///
/// This is the endpoint the model switcher calls; the session PATCH accepts the
/// same selection as a `model` field for clients that prefer one request.
pub async fn set_session_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    let selection = crate::routes::parse_body::<SessionModelPinBody>(&body)?.into_selection();
    if let Some(selection) = &selection {
        crate::settings::validate_session_model(&state.settings().snapshot(), selection)
            .map_err(ApiError::from)?;
    }
    state
        .store()
        .set_session_model(&id, selection.as_ref())
        .await?;
    let session = session_or_404(&state, &id).await?;
    state.runtime().emit(
        Some(&id),
        None,
        None,
        Event::SessionModelChanged {
            session_id: id.clone(),
            model: selection,
        },
    );
    Ok(Json(
        serde_json::to_value(session).map_err(|e| ApiError::internal(e.to_string()))?,
    ))
}

pub async fn cancel_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let _ = session_or_404(&state, &id).await?;
    let cancelled = state.runtime().cancel(&id);
    Ok(Json(json!({ "cancelled": cancelled })))
}

/// Give an untitled session a title derived from its first user message.
///
/// Give an untitled session a deterministic title derived from its first user
/// message, and say whether the session is eligible for a model-written title.
///
/// Runs before `runtime.submit` so the rail shows a real title as soon as the run
/// starts - the model-written title (see `crate::titles`) arrives later and
/// replaces this one. Cosmetic only: any failure is logged and the request
/// proceeds with whatever title the session already had.
///
/// Returns `Some(title)` when the session was still untitled (the AI title may go
/// on to replace `title`), and `None` when somebody already named it. The `None`
/// case is what keeps the documented promise that an explicitly titled session is
/// never retitled by a model: there is nothing to improve on, and a user who typed
/// a name meant it.
async fn maybe_auto_title(state: &AppState, session: &Session, content: &str) -> Option<String> {
    // A temporary session has no rail row to name, and its transcript can be
    // deleted by the expiry path at any moment - a background titling request
    // racing that delete is work for nothing. Titles are for sessions that
    // stay.
    if session.ephemeral {
        return None;
    }
    if !is_placeholder_title(&session.title) {
        return None;
    }
    let title = derive_title(content)?;
    match state.store().rename_session(&session.id, &title).await {
        Ok(()) => {
            state.runtime().emit(
                Some(&session.id),
                None,
                None,
                Event::SessionRenamed {
                    session_id: session.id.clone(),
                    title: title.clone(),
                },
            );
            Some(title)
        }
        Err(error) => {
            tracing::warn!(
                session = %session.id,
                error = %error,
                "could not auto-title session"
            );
            // The derived title did not land, so there is nothing stable for the
            // model's title to replace. Leave the session alone.
            None
        }
    }
}

/// True while the session still carries the title it was created with.
///
/// The store default is the empty string (rows created before titles existed or
/// outside the API) and `POST /api/sessions` fills in [`DEFAULT_SESSION_TITLE`].
/// A user who explicitly renames into one of those two values gets auto-titled
/// once more; the schema keeps no `renamed` provenance bit, so that case is
/// indistinguishable from an untouched session.
fn is_placeholder_title(title: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == DEFAULT_SESSION_TITLE
}

/// Derive a rail-friendly title from a prompt.
///
/// 1. trim the edges;
/// 2. drop one trivial leading marker (a leading `/` command sigil or a
///    `[tool:...]` prefix) so synthetic prompts do not title the session
///    `[tool:...]`;
/// 3. collapse every whitespace run (newlines included) to one space;
/// 4. keep at most [`MAX_AUTO_TITLE_CHARS`] characters on a char boundary and
///    append `…` when text was dropped (so at most 61 characters total).
///
/// Returns `None` when nothing printable is left, which leaves the title alone.
///
/// Shared with `crate::titles`, which derives the placeholder title a control
/// chat or a dispatched session gets before any model sees it.
pub(crate) fn derive_title(content: &str) -> Option<String> {
    let text = collapse_whitespace(strip_leading_marker(content.trim()));
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= MAX_AUTO_TITLE_CHARS {
        return Some(text);
    }
    let mut title: String = text.chars().take(MAX_AUTO_TITLE_CHARS).collect();
    let kept = title.trim_end().len();
    title.truncate(kept);
    title.push('…');
    Some(title)
}

/// Remove a leading `/` command sigil or `[tool:...]` prefix. Anything more
/// elaborate (quoting, escapes) is not worth guessing at.
fn strip_leading_marker(text: &str) -> &str {
    let mut rest = text;
    if let Some(after) = rest.strip_prefix('/') {
        rest = after.trim_start();
    }
    if let Some(after) = rest.strip_prefix("[tool:") {
        if let Some(end) = after.find(']') {
            rest = after[end + 1..].trim_start();
        }
    }
    let rest = rest.trim();
    // A prompt that was nothing but the marker keeps its original text.
    if rest.is_empty() {
        text
    } else {
        rest
    }
}

/// Replace every run of whitespace (spaces, tabs, newlines, ...) with a single
/// space and trim the edges.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rename exactly as the ordinary HTTP route does: persist, read the canonical row
/// back, then announce the stored title so every client shares one session shape.
pub(crate) async fn rename_session_and_emit(
    state: &AppState,
    id: &str,
    title: &str,
) -> ApiResult<Session> {
    state.store().rename_session(id, title).await?;
    let session = session_or_404(state, id).await?;
    state.runtime().emit(
        Some(id),
        None,
        None,
        Event::SessionRenamed {
            session_id: id.to_string(),
            title: session.title.clone(),
        },
    );
    Ok(session)
}

pub(crate) async fn session_or_404(state: &AppState, id: &str) -> ApiResult<Session> {
    state
        .store()
        .get_session(id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("session `{id}` does not exist")))
}

fn normalize_title(raw: Option<String>) -> ApiResult<String> {
    let title = raw.unwrap_or_default();
    let title = title.trim();
    if title.is_empty() {
        return Ok("New session".to_string());
    }
    if title.contains('\0') {
        return Err(ApiError::bad_request("title may not contain NUL bytes"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(ApiError::bad_request(format!(
            "title is too long ({} characters, maximum {MAX_TITLE_CHARS})",
            title.chars().count()
        )));
    }
    Ok(title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_normalized_and_bounded() {
        assert_eq!(normalize_title(None).unwrap(), DEFAULT_SESSION_TITLE);
        assert_eq!(normalize_title(Some("  hi  ".into())).unwrap(), "hi");
        assert!(normalize_title(Some("x".repeat(201))).is_err());
        assert!(normalize_title(Some("a\0b".into())).is_err());
    }

    #[test]
    fn placeholder_titles_are_detected() {
        assert!(is_placeholder_title(""));
        assert!(is_placeholder_title("   "));
        assert!(is_placeholder_title(DEFAULT_SESSION_TITLE));
        assert!(is_placeholder_title(" New session "));
        assert!(!is_placeholder_title("New session!"));
        assert!(!is_placeholder_title("fix the build"));
    }

    #[test]
    fn titles_are_derived_from_content() {
        assert_eq!(
            derive_title("  fix   the\n\nbuild\tplease  ").unwrap(),
            "fix the build please"
        );
        assert_eq!(derive_title("/clear").unwrap(), "clear");
        assert_eq!(
            derive_title("[tool:read] summarise this").unwrap(),
            "summarise this"
        );
        assert_eq!(derive_title("   \n\t "), None);
        assert_eq!(
            derive_title("é".repeat(61).as_str())
                .unwrap()
                .chars()
                .count(),
            61
        );
        // Exactly at the limit: no ellipsis, no truncation.
        assert_eq!(derive_title(&"a".repeat(60)).unwrap(), "a".repeat(60));
        let long = derive_title(&"é".repeat(200)).unwrap();
        assert_eq!(long.chars().count(), 61);
        assert!(long.ends_with('…'));
        assert_eq!(long.chars().filter(|c| *c == 'é').count(), 60);
    }
}
