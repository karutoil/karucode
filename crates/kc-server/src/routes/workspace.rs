//! Workspace explorer (directory tree, file contents), the editor's save
//! endpoint and attachment uploads.
//!
//! The read endpoints, the write endpoint and the upload all resolve through
//! [`kc_core::Workspace`], so traversal and symlink escapes are refused with a
//! 400 `path_escape` before any I/O.

use crate::error::{ApiError, ApiResult};
use crate::routes::{parse_required, query_bool, query_str, query_usize, QueryMap};
use crate::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use kc_core::config::ApprovalMode;
use kc_core::error::Error;
use kc_core::paths::Workspace;
use kc_core::settings::Settings;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Where composer attachments are written, relative to the project root. The
/// directory lives *inside* the workspace on purpose: the file tools are
/// confined to the workspace, so this is the only place the agent can read an
/// uploaded file from.
pub const UPLOAD_ROOT: &str = ".karucode/attachments";

/// Hard ceiling for one upload. The body is buffered in memory, so this is also
/// the worst-case allocation for a single request.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_ATTACHMENT_NAME_BYTES: usize = 160;
const MAX_SESSION_ID_BYTES: usize = 64;
const MAX_CONTENT_TYPE_BYTES: usize = 128;
/// `name`, `name-1`, … `name-N`; exhausting this is a 409 rather than a hang.
const COLLISION_ATTEMPTS: u32 = 1000;

const DEFAULT_DEPTH: usize = 1;
const MAX_DEPTH: usize = 6;
const MAX_TREE_ENTRIES: usize = 2000;
/// Equally important: cap directory entries *examined*, not just returned. A
/// workspace containing millions of generated files must not turn one UI poll
/// into a full filesystem scan.
const MAX_TREE_WORK_ITEMS: usize = 32_000;
/// Directories that are never useful in a UI listing: build output, dependency
/// trees and VCS internals. They are skipped *before* the entry budget is spent,
/// so a project is not truncated down to `.git` objects and `target/` artifacts.
/// Naming one explicitly (`path: ".git"`) still lists it, because the filter
/// only applies while descending.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".next",
    ".turbo",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    "coverage",
    ".idea",
];
/// Hard ceiling for a single file response, independent of the configurable
/// read limit, so a huge file cannot exhaust memory.
const HARD_READ_LIMIT: usize = 16 * 1024 * 1024;

pub async fn tree(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let path = query_str(&query, "path").unwrap_or(".").to_string();
    let depth = query_usize(&query, "depth")?
        .unwrap_or(DEFAULT_DEPTH)
        .clamp(DEFAULT_DEPTH, MAX_DEPTH);
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;

    let value = tokio::task::spawn_blocking(move || tree_blocking(&workspace, &path, depth))
        .await
        .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;
    Ok(Json(value))
}

fn tree_blocking(workspace: &Workspace, path: &str, depth: usize) -> Result<Value, Error> {
    let root = workspace.resolve_existing(path)?;
    if !root.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{path}` is not a directory; use /api/workspace/file for a file"
        )));
    }
    let mut entries = Vec::new();
    let mut work_remaining = MAX_TREE_WORK_ITEMS;
    let mut truncated = false;
    walk(
        workspace,
        &root,
        depth,
        &mut entries,
        &mut work_remaining,
        &mut truncated,
    );
    Ok(json!({
        "path": workspace.relative(&root),
        "entries": entries,
        "truncated": truncated,
        "limit": MAX_TREE_ENTRIES,
    }))
}

type DirectoryItem = (bool, String, PathBuf);

fn directory_item_cmp(a: &DirectoryItem, b: &DirectoryItem) -> std::cmp::Ordering {
    b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1))
}

/// Retain only the best `MAX_TREE_ENTRIES` siblings. This avoids collecting and
/// sorting an arbitrarily large generated directory before the global cap applies.
fn retain_directory_item(items: &mut Vec<DirectoryItem>, item: DirectoryItem) -> bool {
    items.push(item);
    if items.len() <= MAX_TREE_ENTRIES {
        return false;
    }
    let worst = items
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| directory_item_cmp(a, b))
        .map(|(index, _)| index)
        .expect("an over-cap directory is not empty");
    items.swap_remove(worst);
    true
}

/// Depth-first listing, directories first, bounded in returned entries, retained
/// siblings and examined filesystem entries. Unreadable subdirectories are skipped
/// instead of failing the whole tree, and [`IGNORED_DIRS`] never enters the listing.
fn walk(
    workspace: &Workspace,
    dir: &Path,
    depth: usize,
    out: &mut Vec<Value>,
    work_remaining: &mut usize,
    truncated: &mut bool,
) {
    if depth == 0 || out.len() >= MAX_TREE_ENTRIES {
        return;
    }
    let read_dir = match std::fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(e) => {
            tracing::debug!(path = %dir.display(), "skipping unreadable directory: {e}");
            return;
        }
    };
    let mut items: Vec<DirectoryItem> = Vec::new();
    for entry in read_dir.flatten() {
        if *work_remaining == 0 {
            *truncated = true;
            break;
        }
        *work_remaining -= 1;
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        // Do not follow symlinked directories: they could point outside the
        // workspace and recursion could loop.
        let is_dir = file_type.is_dir();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if is_dir && IGNORED_DIRS.contains(&name.as_str()) {
            continue;
        }
        if retain_directory_item(&mut items, (is_dir, name, entry.path())) {
            *truncated = true;
        }
    }
    items.sort_by(directory_item_cmp);

    let item_count = items.len();
    for (index, (is_dir, _name, full)) in items.into_iter().enumerate() {
        if out.len() >= MAX_TREE_ENTRIES {
            // If another sibling remains, the response is definitely partial. If
            // this was the final sibling, the bounded-retention overflow check
            // above has already determined whether anything was omitted.
            if index < item_count {
                *truncated = true;
            }
            return;
        }
        let name = full
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let size = if is_dir {
            0
        } else {
            std::fs::symlink_metadata(&full)
                .map(|m| m.len())
                .unwrap_or(0)
        };
        out.push(json!({
            "name": name,
            "path": workspace.relative(&full),
            "kind": if is_dir { "dir" } else { "file" },
            "size": size,
        }));
        if is_dir {
            walk(workspace, &full, depth - 1, out, work_remaining, truncated);
        }
    }
}

pub async fn file(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<Json<Value>> {
    let path = query_str(&query, "path")
        .ok_or_else(|| ApiError::bad_request("query parameter `path` is required"))?
        .to_string();
    // Reads are confined to the same project the tree/git endpoints use, so a
    // file opened from a project's tree is read from that project.
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let limit = state
        .settings()
        .limits()
        .await
        .max_file_read_bytes
        .clamp(1024, HARD_READ_LIMIT);

    let value = tokio::task::spawn_blocking(move || file_blocking(&workspace, &path, limit))
        .await
        .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;
    Ok(Json(value))
}

fn file_blocking(workspace: &Workspace, path: &str, limit: usize) -> Result<Value, Error> {
    let abs = workspace.resolve_existing(path)?;
    let meta = std::fs::metadata(&abs).map_err(|e| Error::io(abs.clone(), e))?;
    if meta.is_dir() {
        return Err(Error::BadRequest(format!(
            "`{path}` is a directory; use /api/workspace/tree"
        )));
    }
    if !meta.is_file() {
        return Err(Error::BadRequest(format!(
            "`{path}` is not a regular file; FIFOs, sockets and devices cannot be read"
        )));
    }
    let mut file = std::fs::File::open(&abs).map_err(|e| Error::io(abs.clone(), e))?;
    let mut bytes = Vec::new();
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    // Hash the complete file while retaining only the bounded preview prefix.
    // Hashing just that prefix would let a same-prefix edit made after a
    // truncated read pass the editor's optimistic-concurrency check.
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| Error::io(abs.clone(), e))?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
        let keep = (limit + 1).saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..read.min(keep)]);
    }
    let digest = content_digest(hasher);
    let truncated = bytes.len() > limit || size > limit as u64;
    bytes.truncate(limit);

    let binary = is_binary(&bytes);
    let content = if binary {
        String::new()
    } else {
        match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                // Not valid UTF-8: report it as binary rather than mangling it.
                return Ok(json!({
                    "path": workspace.relative(&abs),
                    "content": "",
                    "size": size,
                    "digest": digest,
                    "truncated": truncated,
                    "binary": true,
                    "language": Value::Null,
                }));
            }
        }
    };

    Ok(json!({
        "path": workspace.relative(&abs),
        "content": content,
        "size": size,
        "digest": digest,
        "truncated": truncated,
        "binary": binary,
        "language": language_for(&abs),
    }))
}

/// Ceiling for the raw-bytes preview endpoint. Images and PDFs are the point of
/// it, so it is larger than the text read limit but still bounded so one request
/// cannot exhaust memory.
pub const MAX_RAW_BYTES: u64 = 32 * 1024 * 1024;

/// Route-local CSP for a workspace file's bytes, whose `Content-Type` is guessed
/// from the file name and therefore attacker-chosen.
///
/// `sandbox` is the directive that makes this safe: without `allow-scripts` the
/// browser puts the document in an **opaque origin** and refuses to run script in
/// it. An `evil.html` sitting in the workspace is then inert - it cannot read the
/// app's `localStorage` (the bearer token the auth screen saved), cannot call
/// `/api` (it has no origin to be same-origin with), cannot register a service
/// worker and cannot frame the app. `default-src 'none'` additionally stops it
/// loading or fetching anything, so even `<img src="http://…">` inside it cannot
/// phone home.
///
/// The response keeps its guessed `Content-Type` and stays `inline` rather than
/// `attachment`, because that is what makes the previews work: an `<img src>`
/// needs the image bytes and a PDF `<iframe>` needs the document inline. Turning
/// every response into a download would break both, so the sandbox - not the
/// disposition - is what neutralises an HTML file.
pub const RAW_CONTENT_SECURITY_POLICY: &str = "sandbox; default-src 'none'";

/// Ceiling for one file in a preview tree: the entry document or any asset it
/// loads. Bounded so one request cannot exhaust memory.
pub const MAX_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

/// Policy for [`preview`]: a workspace HTML file rendered *as a page*.
///
/// [`RAW_CONTENT_SECURITY_POLICY`] cannot be used here. `default-src 'none'`
/// and a bare `sandbox` (no `allow-scripts`) leave an HTML file almost entirely
/// inert, which is right for "the bytes of a file" and useless for "look at my
/// page": no inline script, no sibling stylesheet, no relative image.
///
/// This policy keeps the security property that matters - the document is an
/// **opaque origin** - while letting the page be a page. It is built per response
/// ([`preview_content_security_policy`]) because the one directive that makes the
/// difference cannot be written as a keyword:
///
/// * `sandbox allow-scripts` (never `allow-same-origin`) is what makes the
///   origin opaque. Verified in Chromium: the frame's `parent`, `localStorage`
///   and `document.cookie` reads all throw `SecurityError`, `window.open`
///   returns `null`, and `top.location` cannot be navigated.
/// * `script-src`/`style-src`/`img-src`/`font-src`/`media-src` name **this
///   server's own origin**, so `./app.js`, `./style.css` and sibling images load
///   from this workspace. `'self'` cannot do that job: the document runs in an
///   opaque origin, where `'self'` matches nothing at all, which is why every
///   relative asset of every previewed page was refused (measured in Chromium:
///   the stylesheet never applied, the sibling script never ran, the relative
///   image never decoded - each request `ERR_BLOCKED_BY_RESPONSE`). Naming the
///   host is also the narrowest thing that works.
///   Remote sources are deliberately absent: an opaque origin does not stop an
///   `<img>` or stylesheet from transmitting DOM-derived data to another server.
///   `data:`/`blob:` remain available for generated inline resources.
/// * `connect-src 'none'` blocks `fetch`/XHR/WebSocket as well. Workspace HTML is
///   untrusted, script-capable content inside an opaque sandbox, not an authorized
///   general-purpose web page; opening a preview never approves network egress.
/// * `frame-src`/`child-src`/`object-src 'none'` stop the preview nesting more
///   documents or plugins, `form-action 'none'` stops it submitting anywhere,
///   and `base-uri 'self'` stops a `<base>` from re-pointing its own relative
///   URLs at another origin.
/// * `frame-ancestors 'self'` is narrower than the app-wide `'none'` so the file
///   viewer's own iframe may embed this response, and nothing else can. The
///   `sandbox` directive also makes [`crate::app::security_headers`] withhold
///   `X-Frame-Options: DENY`, which would otherwise block that frame.
///
/// Fonts and ES modules still cannot load: both are CORS-mode requests, and this
/// route sends no `Access-Control-Allow-Origin` (granting one would let any web
/// page *read* workspace bytes). A classic `<script src>`, a stylesheet and an
/// image are not CORS-gated, and those are what a preview needs.
pub fn preview_content_security_policy(asset_sources: Option<&str>) -> String {
    let own = asset_sources.unwrap_or("'self'");
    format!(
        "sandbox allow-scripts; default-src 'none'; \
         img-src {own} data: blob:; media-src {own} data: blob:; font-src {own} data:; \
         style-src {own} 'unsafe-inline'; script-src {own} 'unsafe-inline' 'unsafe-eval' blob:; \
         connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; \
         object-src 'none'; form-action 'none'; base-uri 'self'; frame-ancestors 'self'"
    )
}

/// The origin(s) a previewed page may load its own siblings from, taken from the
/// request's `Host`.
///
/// Both schemes are listed because the same node may be reached over plain
/// `http` or behind a TLS terminator, and the browser matches the document's own
/// scheme. A `Host` that is missing or contains anything but a host/port is
/// refused: this string goes into a policy header, and the fallback (`'self'`,
/// which matches nothing in an opaque origin) fails closed rather than widened.
fn preview_asset_sources(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?.trim();
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return None;
    }
    Some(format!("http://{host} https://{host}"))
}

/// `GET /api/workspace/preview/<path>[?project=<id>]` and
/// `GET /api/workspace/preview-project/<project>/<path>`
///
/// One workspace file served from its own workspace path, so a previewed page is
/// a real document that can load its own siblings.
///
/// The path is in the URL rather than a query parameter on purpose: a document
/// loaded from `/api/workspace/preview/docs/index.html` resolves `./style.css`
/// and `../img/logo.png` against its own directory, which is exactly what a page
/// authored in the workspace expects. A query parameter cannot do that - and
/// neither can `srcdoc`, which additionally inherits the app's own
/// `default-src 'self'` policy and then runs in an opaque origin where `'self'
/// matches nothing, so *every* subresource and inline script is refused.
///
/// The same reason is why there are two shapes. A *relative* URL drops the
/// query, so a page opened with `?project=<id>` asked for its own siblings
/// without that parameter and the preview tree resolved against the default
/// project - a 404 for every stylesheet, script and image of a file in any other
/// project. `preview-project/<id>/<path>` puts the scope in the path, where
/// relative resolution keeps it.
///
/// This serves the whole preview *tree*, not just the entry document: the same
/// route answers the page's `style.css`, its `pic.png` and its `app.js`, because
/// those requests carry the same `/api/workspace/preview/...` prefix. That is
/// what makes a preview of a real page look like the page.
///
/// The security shape differs by what is being served, and it is deliberately
/// narrow in both cases:
///
/// * a document (`text/html`) gets [`preview_content_security_policy`], which is
///   an opaque origin with script but no foreign network or remote egress: only
///   its local preview siblings and inline data/blob resources are permitted;
/// * every other asset keeps [`RAW_CONTENT_SECURITY_POLICY`], which is an opaque
///   origin with no script and no further subresources - correct for bytes that
///   are only ever meant to be drawn.
///
/// Both are opaque origins, so neither can reach the app: no `/api` call, no
/// `localStorage`, no cookie, no framing of the app.
pub async fn preview(
    State(state): State<AppState>,
    axum::extract::Path(rest): axum::extract::Path<String>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    let project = query_str(&query, "project");
    preview_response(&state, project, &rest, &headers).await
}

/// `GET /api/workspace/preview-project/<project>/<path>`
///
/// The project-scoped shape of [`preview`]: see there for why the scope has to
/// be in the path rather than the query. Everything else - the tree, the
/// headers, the sandbox - is identical.
pub async fn preview_project(
    State(state): State<AppState>,
    axum::extract::Path((project, rest)): axum::extract::Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    preview_response(&state, Some(&project), &rest, &headers).await
}

async fn preview_response(
    state: &AppState,
    project: Option<&str>,
    rest: &str,
    request_headers: &HeaderMap,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;

    let workspace = state.workspace_for_project(project)?;
    let rest = rest.to_string();
    let (bytes, content_type) =
        tokio::task::spawn_blocking(move || preview_blocking(&workspace, &rest, MAX_PREVIEW_BYTES))
            .await
            .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;

    let is_document = content_type.starts_with("text/html");
    let mut response = axum::body::Bytes::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_static("inline"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    let asset_sources = preview_asset_sources(request_headers);
    let policy = if is_document {
        preview_content_security_policy(asset_sources.as_deref())
    } else {
        RAW_CONTENT_SECURITY_POLICY.to_string()
    };
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_str(&policy)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static(RAW_CONTENT_SECURITY_POLICY)),
    );
    // The app-wide `Cross-Origin-Resource-Policy: same-origin` (see
    // `app::security_headers`) would defeat this route entirely. A previewed page
    // and everything it loads run in an **opaque origin**, so every one of its
    // subresource requests is cross-origin by definition, and CORP
    // `same-origin` makes the browser refuse them: measured in Chromium, the
    // sibling stylesheet never applied, the relative `<img>` never decoded and a
    // sibling `.js` never ran, while the document itself loaded fine.
    //
    // This is the one place the header is widened, and it is deliberately narrow:
    //
    // * it applies to this route only - every other response keeps
    //   `same-origin`, so no other workspace or API byte becomes loadable;
    // * CORP never grants *read* access. It decides whether a subresource may be
    //   embedded, and this server sends no `Access-Control-Allow-Origin`, so a
    //   foreign page can at most make a browser *render* a workspace file - what
    //   any static file server allows - never read it;
    // * what it renders is inert: a document here is served an opaque origin with
    //   `connect-src 'none'`, so even a hostile `.html` cannot phone home.
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        axum::http::HeaderValue::from_static("cross-origin"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

/// Read one file behind a preview URL, with the MIME type its name implies.
///
/// A directory answers with its `index.html` when it has one, so
/// `/api/workspace/preview/branding/` renders the same page a static server
/// would. A document must be valid UTF-8; anything else is served as bytes for
/// an `<img>`/`<link>` to consume.
fn preview_blocking(
    workspace: &Workspace,
    rest: &str,
    limit: u64,
) -> Result<(Vec<u8>, String), ApiError> {
    let path = if rest.trim_matches('/').is_empty() {
        ".".to_string()
    } else {
        rest.trim_matches('/').to_string()
    };
    let mut abs = workspace.resolve_existing(&path)?;
    if abs.is_dir() {
        let index = abs.join("index.html");
        if std::fs::symlink_metadata(&index).is_err() {
            return Err(ApiError::bad_request(format!(
                "`{path}` is a directory with no index.html to render"
            )));
        }
        // Resolve the index through Workspace too: a symlink at this second,
        // appended candidate must not bypass the canonical confinement check.
        abs = workspace.resolve_existing(&workspace.relative(&index))?;
    }
    let meta = std::fs::metadata(&abs).map_err(|e| ApiError::from(Error::io(&abs, e)))?;
    if !meta.is_file() {
        return Err(ApiError::bad_request(format!(
            "`{path}` is not a regular file; FIFOs, sockets and devices cannot be previewed"
        )));
    }
    if meta.len() > limit {
        return Err(ApiError::payload_too_large(
            limit as usize,
            Some("this file is too large to render as a preview"),
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&abs)
        .map_err(|e| ApiError::from(Error::io(&abs, e)))?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ApiError::from(Error::io(&abs, e)))?;
    if bytes.len() as u64 > limit {
        return Err(ApiError::payload_too_large(
            limit as usize,
            Some("this file is too large to render as a preview"),
        ));
    }
    let content_type = mime_guess::from_path(&abs)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    if content_type.starts_with("text/html") && std::str::from_utf8(&bytes).is_err() {
        return Err(ApiError::bad_request(format!(
            "`{path}` is not valid UTF-8, so it cannot be rendered as a page"
        )));
    }
    Ok((bytes, content_type))
}

/// `GET /api/workspace/raw?path=<file>[&project=<id>]`
///
/// The bytes of one workspace file, with the MIME type guessed from its name.
/// This exists so the viewer can `<img src>` a PNG or embed a PDF: the text
/// endpoint answers JSON, which a browser cannot render. It is read-only and
/// confined exactly like `/api/workspace/file`.
///
/// Responses carry `Content-Disposition: inline` (never `attachment`) so images
/// and PDFs embed, plus a route-local [`RAW_CONTENT_SECURITY_POLICY`] whose
/// `sandbox` directive makes the document an opaque origin with script disabled.
/// That is what stops an uploaded HTML file from running in the app's origin
/// (which would otherwise let it `PATCH /api/config` or open a terminal); the
/// app-wide CSP alone could not, because its `default-src 'self'` allows
/// same-origin scripts and this response *is* the app's origin.
pub async fn raw(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;

    let path = query_str(&query, "path")
        .ok_or_else(|| ApiError::bad_request("query parameter `path` is required"))?
        .to_string();
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;

    let (bytes, rel, content_type) =
        tokio::task::spawn_blocking(move || raw_blocking(&workspace, &path, MAX_RAW_BYTES))
            .await
            .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;

    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        axum::http::HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        // The file name can hold quotes and non-ASCII; the path is echoed in a
        // header, so keep it to a safe, quoted form.
        axum::http::HeaderValue::from_str(&format!(
            "inline; filename=\"{}\"",
            rel.rsplit('/')
                .next()
                .unwrap_or("file")
                .replace(['"', '\\'], "_")
        ))
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("inline")),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    // Route-local, so it replaces the app-wide CSP for this response: `sandbox`
    // makes the document an opaque origin with no script, which is what stops a
    // workspace `.html` file from executing in the app's origin. The bytes stay
    // `inline` with their guessed type so `<img src>` and the PDF `<iframe>` keep
    // working. See [`RAW_CONTENT_SECURITY_POLICY`].
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static(RAW_CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

fn raw_blocking(
    workspace: &Workspace,
    path: &str,
    limit: u64,
) -> Result<(Vec<u8>, String, String), ApiError> {
    let abs = workspace.resolve_existing(path)?;
    let meta = std::fs::metadata(&abs).map_err(|e| ApiError::from(Error::io(&abs, e)))?;
    if meta.is_dir() {
        return Err(ApiError::bad_request(format!(
            "`{path}` is a directory; use /api/workspace/tree"
        )));
    }
    if !meta.is_file() {
        return Err(ApiError::bad_request(format!(
            "`{path}` is not a regular file; FIFOs, sockets and devices cannot be served"
        )));
    }
    if meta.len() > limit {
        return Err(ApiError::payload_too_large(
            limit as usize,
            Some("this file is too large to serve inline"),
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&abs)
        .map_err(|e| ApiError::from(Error::io(&abs, e)))?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ApiError::from(Error::io(&abs, e)))?;
    if bytes.len() as u64 > limit {
        return Err(ApiError::payload_too_large(
            limit as usize,
            Some("this file is too large to serve inline"),
        ));
    }
    let content_type = mime_guess::from_path(&abs)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    Ok((bytes, workspace.relative(&abs), content_type))
}

/// Ceiling for one editor save. It matches the `write_file` tool's own cap so the
/// two write paths cannot disagree about what is too big.
pub const MAX_WRITE_BYTES: usize = kc_tools::file_ops::MAX_FILE_WRITE_BYTES;

/// `PUT /api/workspace/file?path=<file>[&project=<id>][&create=true][&force=true]`
///
/// The body is JSON: `{ "content": "...", "expected_size": <bytes, optional>,
/// "expected_digest": "sha256:<64 hex>", optional }`. `content` must be UTF-8 text
/// — this endpoint never writes binary data. Both expected fields are
/// optimistic-concurrency checks; the digest also catches an equal-length edit.
/// A stale snapshot is refused with `409`, so an editor cannot silently overwrite
/// what the agent wrote in the meantime. `force=true` skips both checks (the UI
/// asks the user first).
///
/// Saves are gated by the same policy the `write_file` tool obeys: `read_only`
/// denies them, and `ask`/`ask_commands` require the path to be inside the
/// workspace (which it always is here) and outside nothing — the browser is the
/// user, so there is nobody to ask.
pub async fn write_file(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let path = query_str(&query, "path")
        .ok_or_else(|| ApiError::bad_request("query parameter `path` is required"))?
        .to_string();
    let workspace = state.workspace_for_project(query_str(&query, "project"))?;
    let create = query_bool(&query, "create")?;
    let force = query_bool(&query, "force")?;

    let mode = state.settings().snapshot().approvals.mode;
    if mode == ApprovalMode::ReadOnly {
        return Err(ApiError::forbidden(
            "approvals.mode is `read_only`, so files cannot be written from the editor; \
             change the mode in Settings to edit files",
        ));
    }

    let request: WriteFileBody = parse_required(&body, "writing a workspace file")?;
    if let Some(expected) = request.expected_digest.as_deref() {
        validate_expected_digest(expected)?;
    }
    let content = request.content;
    // The cap is on the decoded bytes: JSON escaping can make the request body
    // larger than the file it writes.
    if content.len() > MAX_WRITE_BYTES {
        return Err(ApiError::payload_too_large(
            MAX_WRITE_BYTES,
            Some("the editor saves UTF-8 text, at most 2 MiB"),
        ));
    }

    let value = tokio::task::spawn_blocking(move || {
        write_file_versioned_blocking(
            &workspace,
            &path,
            &content,
            request.expected_size,
            request.expected_digest.as_deref(),
            create,
            force,
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("workspace task failed: {e}")))??;
    Ok(Json(value))
}

#[derive(Debug, Default, Deserialize)]
pub struct WriteFileBody {
    /// The complete new file contents. Required, but an empty file is legitimate.
    #[serde(default)]
    pub content: String,
    /// Size the editor loaded. Kept for compatibility with older clients; unlike
    /// the digest this cannot detect a replacement of the same byte length.
    #[serde(default)]
    pub expected_size: Option<u64>,
    /// The read response's full-file `digest`. A mismatch is a `409` unless
    /// `force=true`, including when the replacement has the same size.
    #[serde(default)]
    pub expected_digest: Option<String>,
}

#[cfg(test)]
fn write_file_blocking(
    workspace: &Workspace,
    path: &str,
    content: &str,
    expected_size: Option<u64>,
    create: bool,
    force: bool,
) -> Result<Value, ApiError> {
    write_file_versioned_blocking(workspace, path, content, expected_size, None, create, force)
}

#[allow(clippy::too_many_arguments)]
fn write_file_versioned_blocking(
    workspace: &Workspace,
    path: &str,
    content: &str,
    expected_size: Option<u64>,
    expected_digest: Option<&str>,
    create: bool,
    force: bool,
) -> Result<Value, ApiError> {
    if let Some(expected) = expected_digest {
        validate_expected_digest(expected)?;
    }
    // `resolve_existing` refuses traversal, symlink escapes and a missing file in
    // one step; `resolve_for_write` additionally allows a new path, so the two
    // branches below cannot disagree about confinement.
    let abs = if create {
        workspace.resolve_for_write(path)?
    } else {
        workspace.resolve_existing(path).map_err(|e| match e {
            Error::NotFound(_) => ApiError::not_found(format!(
                "`{path}` does not exist; pass `create=true` to create it"
            )),
            other => ApiError::from(other),
        })?
    };

    let previous_size = match std::fs::metadata(&abs) {
        Ok(meta) => {
            if meta.is_dir() {
                return Err(ApiError::bad_request(format!(
                    "`{path}` is a directory; the editor can only write files"
                )));
            }
            Some(meta.len())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ApiError::from(Error::io(&abs, e))),
    };

    if previous_size.is_none() && (expected_size.is_some() || expected_digest.is_some()) && !force {
        // The editor loaded a file that has since been deleted. Recreating it on
        // save is reasonable, but silently reversing somebody's deletion is not.
        return Err(ApiError::conflict(format!(
            "`{path}` was deleted while it was open; save with force=true to create it again"
        )));
    }
    if let (Some(expected), Some(actual)) = (expected_size, previous_size) {
        if !force && expected != actual {
            return Err(ApiError::conflict(format!(
                "`{path}` changed on disk (it was {expected} bytes when you opened it, now {actual}); \
                 reload it or save with force=true to overwrite"
            )));
        }
    }
    if let (Some(expected), Some(_)) = (expected_digest, previous_size) {
        if !force {
            let actual = file_digest(&abs)?;
            if expected != actual {
                return Err(ApiError::conflict(format!(
                    "`{path}` changed on disk (its content digest no longer matches the snapshot); \
                     reload it or save with force=true to overwrite"
                )));
            }
        }
    }

    kc_tools::file_ops::write_atomic(&abs, content.as_bytes())?;

    Ok(json!({
        "path": workspace.relative(&abs),
        "size": content.len(),
        "digest": content_digest(Sha256::new().chain_update(content.as_bytes())),
        "created": previous_size.is_none(),
        "language": language_for(&abs),
    }))
}

fn content_digest(hasher: Sha256) -> String {
    format!("sha256:{:x}", hasher.finalize())
}

fn file_digest(path: &Path) -> Result<String, ApiError> {
    let mut file =
        std::fs::File::open(path).map_err(|error| ApiError::from(Error::io(path, error)))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ApiError::from(Error::io(path, error)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(content_digest(hasher))
}

fn validate_expected_digest(expected: &str) -> ApiResult<()> {
    let hex = expected.strip_prefix("sha256:").unwrap_or_default();
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "`expected_digest` must be `sha256:` followed by 64 hexadecimal characters",
        ));
    }
    // The GET token is canonical lowercase. Refuse a visually equivalent but
    // differently encoded value rather than silently weakening exact-version use.
    if !hex.bytes().all(|byte| !byte.is_ascii_uppercase()) {
        return Err(ApiError::bad_request(
            "`expected_digest` must use lowercase hexadecimal, as returned by GET",
        ));
    }
    Ok(())
}

/// `POST /api/workspace/upload?session=<id>&name=<file>[&project=<id>]`
///
/// The body is the raw file (`Content-Type` is that of the file, or omitted);
/// JSON and `multipart/form-data` bodies are rejected because storing them
/// verbatim is never what a client means. The file lands in
/// `<project>/.karucode/attachments/<session>/`, so it stays readable by the
/// workspace-confined file tools. An existing name is never overwritten: the
/// response reports the deduplicated name it actually used.
pub async fn upload(
    State(state): State<AppState>,
    Query(query): Query<QueryMap>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let session_id = query_str(&query, "session")
        .ok_or_else(|| {
            ApiError::bad_request(
                "query parameter `session` is required (the session the file belongs to)",
            )
        })?
        .to_string();
    validate_session_id(&session_id)?;

    let name = query_str(&query, "name")
        .ok_or_else(|| {
            ApiError::bad_request(
                "query parameter `name` is required (the file name, without directories)",
            )
        })?
        .to_string();
    let name = validate_attachment_name(&name)?;
    let content_type = attachment_content_type(&headers)?;

    // Unknown sessions cannot own files; the session also decides which project
    // the upload belongs to when `project` is omitted.
    let session = state
        .store()
        .get_session(&session_id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("session `{session_id}` does not exist")))?;
    let workspace = match query_str(&query, "project") {
        Some(project_id) => state.workspace_for_project(Some(project_id))?,
        None => state.workspace_for_session(session.project_id.as_deref(), &session.workspace)?,
    };

    // An honest `Content-Length` is rejected before a byte is buffered. A
    // missing or lying one is caught by the bounded read below.
    if let Some(declared) = content_length(&headers)? {
        if declared > MAX_UPLOAD_BYTES as u64 {
            return Err(payload_too_large(declared));
        }
    }
    let bytes = match axum::body::to_bytes(body, MAX_UPLOAD_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) if is_length_limit(&err) => {
            return Err(payload_too_large(MAX_UPLOAD_BYTES as u64 + 1));
        }
        Err(err) => {
            return Err(ApiError::bad_request(format!(
                "could not read the request body: {err}"
            )))
        }
    };
    if bytes.is_empty() {
        return Err(ApiError::bad_request(
            "the request body is empty; send the file bytes as the raw request body",
        ));
    }

    let root = workspace.root().to_string_lossy().to_string();
    let size = bytes.len() as u64;
    let stored =
        tokio::task::spawn_blocking(move || store_upload(&workspace, &session_id, &name, &bytes))
            .await
            .map_err(|e| ApiError::internal(format!("upload task failed: {e}")))??;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "session": stored.session,
            "name": stored.name,
            "path": stored.path,
            "size": size,
            "content_type": content_type.unwrap_or_else(|| "application/octet-stream".into()),
            "workspace": root,
            "deduplicated": stored.deduplicated,
        })),
    ))
}

#[derive(Debug)]
struct StoredUpload {
    session: String,
    name: String,
    /// Workspace-relative, so it can be fed straight back to
    /// `/api/workspace/file` and to `@`-mentions.
    path: String,
    deduplicated: bool,
}

/// Blocking half of [`upload`]: create the attachment directory and write the
/// file with `create_new`, so two concurrent uploads of the same name cannot
/// overwrite each other (the loser retries with the next candidate name).
fn store_upload(
    workspace: &Workspace,
    session_id: &str,
    name: &str,
    bytes: &[u8],
) -> Result<StoredUpload, ApiError> {
    let dir_rel = format!("{UPLOAD_ROOT}/{session_id}");
    let dir = workspace.resolve_for_write(&dir_rel)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiError::internal(format!("cannot create {}: {e}", dir.display())))?;
    // Re-check the *canonical* directory: a symlinked `.karucode` component is
    // refused by `resolve_for_write`, but the check is repeated here so the file
    // cannot be written through a link created in the meantime.
    let canon_dir = std::fs::canonicalize(&dir)
        .map_err(|e| ApiError::internal(format!("cannot open {}: {e}", dir.display())))?;
    if !workspace.contains(&canon_dir) || !canon_dir.is_dir() {
        return Err(Error::PathEscape {
            path: canon_dir,
            root: workspace.root().to_path_buf(),
        }
        .into());
    }
    restrict_dir(&canon_dir);

    for attempt in 0..COLLISION_ATTEMPTS {
        let candidate = candidate_name(name, attempt);
        let target: PathBuf = canon_dir.join(&candidate);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|e| {
                    // Do not leave a truncated attachment behind.
                    let _ = std::fs::remove_file(&target);
                    ApiError::internal(format!("cannot write {}: {e}", target.display()))
                })?;
                drop(file);
                restrict_file(&target);
                return Ok(StoredUpload {
                    session: session_id.to_string(),
                    name: candidate,
                    path: workspace.relative(&target),
                    deduplicated: attempt > 0,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // A directory or a dangling symlink already sitting on the name is a
            // collision too; never write through either.
            Err(_) if std::fs::symlink_metadata(&target).is_ok() => continue,
            Err(e) => {
                return Err(ApiError::internal(format!(
                    "cannot write {}: {e}",
                    target.display()
                )))
            }
        }
    }
    Err(ApiError::conflict(format!(
        "too many attachments already named `{name}` in {UPLOAD_ROOT}/{session_id}"
    )))
}

/// `name`, then `stem-1.ext`, `stem-2.ext`, … An extensionless or dotfile name
/// (`.env`) is suffixed as a whole.
fn candidate_name(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => {
            format!("{}-{attempt}{}", &name[..dot], &name[dot..])
        }
        _ => format!("{name}-{attempt}"),
    }
}

/// A file name is a single path segment: no directories, no traversal, no
/// control characters. Everything else (dots, spaces, unicode) is allowed.
fn validate_attachment_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("`name` may not be empty"));
    }
    if name.len() > MAX_ATTACHMENT_NAME_BYTES {
        return Err(ApiError::bad_request(format!(
            "`name` is too long ({} bytes, maximum {MAX_ATTACHMENT_NAME_BYTES})",
            name.len()
        )));
    }
    if name == "." || name == ".." {
        return Err(ApiError::bad_request(
            "`name` must be a file name, not a directory reference",
        ));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(ApiError::bad_request(
            "`name` must not contain a directory separator; attachments are named, not pathed",
        ));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(ApiError::bad_request(
            "`name` must not contain control characters",
        ));
    }
    Ok(name.to_string())
}

/// Session ids reach the filesystem as a directory name, so only the alphabet
/// the ids actually use is accepted (`s_<hex>`).
fn validate_session_id(id: &str) -> ApiResult<()> {
    if id.is_empty() || id.len() > MAX_SESSION_ID_BYTES {
        return Err(ApiError::bad_request(format!(
            "`session` must be 1–{MAX_SESSION_ID_BYTES} characters"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "`session` may only contain letters, digits, `_` and `-`",
        ));
    }
    Ok(())
}

fn attachment_content_type(headers: &HeaderMap) -> ApiResult<Option<String>> {
    let Some(raw) = headers.get(header::CONTENT_TYPE) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| ApiError::bad_request("`Content-Type` must be ASCII"))?;
    // Parameters (`; charset=…`) belong to the file, not to the MIME type we
    // echo back.
    let essence = raw.split(';').next().unwrap_or("").trim();
    if essence.is_empty() {
        return Ok(None);
    }
    if essence.len() > MAX_CONTENT_TYPE_BYTES
        || !essence.is_ascii()
        || essence.chars().any(|c| c.is_control())
    {
        return Err(ApiError::bad_request(
            "`Content-Type` must be a short ASCII MIME type",
        ));
    }
    if essence.eq_ignore_ascii_case("multipart/form-data")
        || essence.eq_ignore_ascii_case("application/x-www-form-urlencoded")
    {
        return Err(ApiError::bad_request(
            "send the file bytes as the raw request body; form encodings are not unpacked",
        ));
    }
    if !essence.contains('/') {
        return Err(ApiError::bad_request(
            "`Content-Type` must be a MIME type such as `text/plain` or `application/pdf`",
        ));
    }
    Ok(Some(essence.to_ascii_lowercase()))
}

fn content_length(headers: &HeaderMap) -> ApiResult<Option<u64>> {
    let Some(raw) = headers.get(header::CONTENT_LENGTH) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .map_err(|_| ApiError::bad_request("`Content-Length` must be ASCII"))?;
    raw.trim()
        .parse::<u64>()
        .map(Some)
        .map_err(|_| ApiError::bad_request("`Content-Length` must be a non-negative integer"))
}

fn payload_too_large(declared: u64) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        format!("the upload is {declared} bytes; the maximum is {MAX_UPLOAD_BYTES} bytes"),
    )
}

/// `axum::body::to_bytes` reports a tripped limit as `http-body-util`'s
/// `LengthLimitError`, which this crate cannot name (axum only exposes it as a
/// boxed error), so match its message; any other failure is a broken body.
fn is_length_limit(err: &axum::Error) -> bool {
    use std::error::Error as StdError;
    let mut current: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(err) = current {
        if err.to_string().contains("length limit") {
            return true;
        }
        current = err.source();
    }
    false
}

/// Attachments can contain secrets; keep the directory private. Best effort -
/// a filesystem without Unix modes (or a pre-existing user directory) must not
/// fail the upload.
fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!(path = %dir.display(), "could not restrict attachment directory: {e}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

fn restrict_file(file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(path = %file.display(), "could not restrict attachment file: {e}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

fn language_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "jsonc" => "json",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "php" => "php",
        "sh" | "bash" | "zsh" => "shell",
        "fish" => "shell",
        "ps1" => "powershell",
        "sql" => "sql",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "less" => "less",
        "xml" => "xml",
        "yaml" | "yml" => "yaml",
        "ini" | "cfg" | "conf" => "ini",
        "dockerfile" => "dockerfile",
        "lua" => "lua",
        "swift" => "swift",
        "zig" => "zig",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "scala" => "scala",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (std::path::PathBuf, Workspace) {
        let dir = std::env::temp_dir().join(format!(
            "kc-server-ws-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("README.md"), "# hi\n").unwrap();
        let ws = Workspace::new(&dir).unwrap();
        (dir, ws)
    }

    #[test]
    fn tree_skips_noise_directories() {
        let (dir, ws) = workspace();
        for path in [
            ".git/objects",
            "node_modules/pkg",
            "target/debug",
            "dist/assets",
            ".github/workflows",
        ] {
            std::fs::create_dir_all(dir.join(path)).unwrap();
        }
        std::fs::write(dir.join(".git/HEAD"), "ref: main").unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(dir.join("target/debug/bin"), "x").unwrap();
        std::fs::write(dir.join("dist/assets/app.js"), "x").unwrap();
        std::fs::write(dir.join(".github/workflows/ci.yml"), "on: push").unwrap();

        let value = tree_blocking(&ws, ".", 4).unwrap();
        let paths: Vec<String> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["path"].as_str().unwrap().to_string())
            .collect();

        for ignored in [".git", "node_modules", "target", "dist"] {
            assert!(
                !paths
                    .iter()
                    .any(|path| path == ignored || path.starts_with(&format!("{ignored}/"))),
                "{ignored} must not be listed: {paths:?}"
            );
        }
        // Dot-directories that hold real project files are still listed.
        assert!(paths.contains(&".github/workflows/ci.yml".to_string()));
        assert!(paths.contains(&"src/main.rs".to_string()));

        // Asking for an ignored directory explicitly still works.
        let value = tree_blocking(&ws, ".git", 1).unwrap();
        let explicit: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect();
        assert!(explicit.contains(&".git/HEAD"), "{explicit:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tree_lists_directories_first() {
        let (dir, ws) = workspace();
        let value = tree_blocking(&ws, ".", 2).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries[0]["kind"], "dir");
        assert!(entries.iter().any(|e| e["path"] == "src/main.rs"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tree_bounds_wide_directory_work_and_reports_truncation() {
        let (dir, ws) = workspace();
        std::fs::create_dir_all(dir.join("wide")).unwrap();
        for index in (0..=MAX_TREE_ENTRIES).rev() {
            std::fs::write(dir.join("wide").join(format!("file-{index:04}.txt")), "x").unwrap();
        }
        let value = tree_blocking(&ws, "wide", 1).unwrap();
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries.len(), MAX_TREE_ENTRIES);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["limit"], MAX_TREE_ENTRIES);
        assert_eq!(entries[0]["path"], "wide/file-0000.txt");
        assert_eq!(entries[MAX_TREE_ENTRIES - 1]["path"], "wide/file-1999.txt");

        // Build an independent exact-cap directory: exactly the cap is complete,
        // not conservatively marked truncated.
        std::fs::create_dir_all(dir.join("exact")).unwrap();
        for index in 0..MAX_TREE_ENTRIES {
            std::fs::write(dir.join("exact").join(format!("file-{index:04}.txt")), "x").unwrap();
        }
        let exact = tree_blocking(&ws, "exact", 1).unwrap();
        assert_eq!(exact["entries"].as_array().unwrap().len(), MAX_TREE_ENTRIES);
        assert_eq!(exact["truncated"], false);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn retained_siblings_stay_bounded_and_keep_the_sort_order_prefix() {
        let mut items = Vec::new();
        for index in (0..10_000).rev() {
            retain_directory_item(
                &mut items,
                (
                    false,
                    format!("{index:05}"),
                    PathBuf::from(format!("f{index}")),
                ),
            );
            assert!(items.len() <= MAX_TREE_ENTRIES);
        }
        items.sort_by(directory_item_cmp);
        assert_eq!(items.len(), MAX_TREE_ENTRIES);
        assert_eq!(items[0].1, "00000");
        assert_eq!(items[MAX_TREE_ENTRIES - 1].1, "01999");
    }

    #[test]
    fn file_reports_content_and_language() {
        let (dir, ws) = workspace();
        let value = file_blocking(&ws, "src/main.rs", 1024).unwrap();
        assert_eq!(value["content"], "fn main() {}\n");
        assert_eq!(value["language"], "rust");
        assert_eq!(value["binary"], false);
        assert_eq!(value["truncated"], false);
        assert_eq!(
            value["digest"].as_str().unwrap(),
            file_digest(&dir.join("src/main.rs")).unwrap()
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn file_truncates_and_detects_binary() {
        let (dir, ws) = workspace();
        let value = file_blocking(&ws, "README.md", 1024).unwrap();
        assert!(!value["binary"].as_bool().unwrap());

        std::fs::write(dir.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let value = file_blocking(&ws, "blob.bin", 1024).unwrap();
        assert_eq!(value["binary"], true);
        assert_eq!(value["content"], "");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn workspace_read_routes_refuse_fifos_before_opening_them() {
        use std::os::unix::ffi::OsStrExt;

        let (dir, ws) = workspace();
        let fifo = dir.join("pipeline.html");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

        // All three would block opening this FIFO if metadata accepted it as a
        // readable non-directory. They must return immediately instead.
        let text = file_blocking(&ws, "pipeline.html", 1024).unwrap_err();
        assert!(text.to_string().contains("regular file"), "{text}");
        for result in [
            preview_blocking(&ws, "pipeline.html", 4096).map(|_| ()),
            raw_blocking(&ws, "pipeline.html", 4096).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(error.message.contains("regular file"), "{error:?}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn traversal_is_refused() {
        let (dir, ws) = workspace();
        let err = file_blocking(&ws, "../../etc/passwd", 1024).unwrap_err();
        assert!(matches!(err, Error::PathEscape { .. }));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn preview_serves_a_page_and_its_own_siblings() {
        let (dir, ws) = workspace();
        std::fs::create_dir_all(dir.join("site")).unwrap();
        std::fs::write(
            dir.join("site/index.html"),
            "<link rel=stylesheet href=sibling.css><h1>hi</h1>",
        )
        .unwrap();
        std::fs::write(dir.join("site/sibling.css"), "body { background: red; }").unwrap();
        std::fs::write(dir.join("site/pic.png"), [0u8, 1, 2, 3]).unwrap();

        let (bytes, content_type) = preview_blocking(&ws, "site/index.html", 4096).unwrap();
        assert_eq!(content_type, "text/html");
        assert!(String::from_utf8(bytes).unwrap().contains("<h1>hi</h1>"));

        // The page's own references arrive through the same prefix, so the
        // browser resolves `sibling.css` and `pic.png` against it.
        let (_, css_type) = preview_blocking(&ws, "site/sibling.css", 4096).unwrap();
        assert_eq!(css_type, "text/css");
        let (_, png_type) = preview_blocking(&ws, "site/pic.png", 4096).unwrap();
        assert_eq!(png_type, "image/png");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn preview_renders_a_directory_through_its_index() {
        let (dir, ws) = workspace();
        std::fs::create_dir_all(dir.join("site")).unwrap();
        std::fs::write(dir.join("site/index.html"), "<h1>index</h1>").unwrap();
        let (bytes, content_type) = preview_blocking(&ws, "site/", 4096).unwrap();
        assert_eq!(content_type, "text/html");
        assert!(String::from_utf8(bytes).unwrap().contains("index"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn preview_directory_index_is_canonicalised_and_confined() {
        let (dir, ws) = workspace();
        let outside = dir.with_extension("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(dir.join("site")).unwrap();
        std::fs::write(outside.join("secret.html"), "outside secret").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.html"), dir.join("site/index.html"))
            .unwrap();
        assert!(preview_blocking(&ws, "site", 4096).is_err());
        std::fs::remove_dir_all(outside).ok();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn preview_refuses_what_it_cannot_render() {
        let (dir, ws) = workspace();
        // A directory with no index.html is not a page.
        assert!(preview_blocking(&ws, "src", 4096).is_err());
        // A file over the cap is refused rather than truncated into a broken page.
        std::fs::write(dir.join("big.html"), "<p>x</p>".repeat(200)).unwrap();
        assert!(preview_blocking(&ws, "big.html", 64).is_err());
        // Non-UTF-8 HTML cannot be decoded into a document.
        std::fs::write(dir.join("bad.html"), [0xffu8, 0xfe, 0x00]).unwrap();
        assert!(preview_blocking(&ws, "bad.html", 4096).is_err());
        // Traversal is refused exactly like every other workspace read.
        assert!(preview_blocking(&ws, "../../etc/passwd", 4096).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    fn preview_policy(sources: Option<&str>) -> String {
        preview_content_security_policy(sources)
    }

    #[test]
    fn the_preview_policy_is_an_opaque_origin_with_script_but_no_network() {
        // The whole safety story for rendering a workspace page. `allow-scripts`
        // makes the page work; `allow-same-origin` must never appear beside it,
        // or the page would share the app's origin and could read its storage
        // and call `/api`.
        let policy = preview_policy(Some("http://127.0.0.1:8787 https://127.0.0.1:8787"));
        assert!(policy.contains("sandbox allow-scripts"));
        assert!(!policy.contains("allow-same-origin"));
        assert!(policy.contains("connect-src 'none'"));
        assert!(policy.contains("form-action 'none'"));
        assert!(policy.contains("object-src 'none'"));
        // The file viewer's own iframe must be allowed to embed it.
        assert!(policy.contains("frame-ancestors 'self'"));
        // The page's own siblings load from this server and nowhere else.
        assert!(policy.contains("script-src http://127.0.0.1:8787"));
        assert!(!policy.contains("*"));
        // Assets keep the stricter no-script policy.
        assert!(RAW_CONTENT_SECURITY_POLICY.contains("sandbox"));
        assert!(!RAW_CONTENT_SECURITY_POLICY.contains("allow-scripts"));
    }

    #[test]
    fn the_preview_policy_widens_only_to_a_host_it_can_vouch_for() {
        use axum::http::HeaderValue;
        let mut headers = HeaderMap::new();
        assert_eq!(preview_asset_sources(&headers), None);
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:8787"));
        assert_eq!(
            preview_asset_sources(&headers).as_deref(),
            Some("http://127.0.0.1:8787 https://127.0.0.1:8787")
        );
        // A `Host` carrying anything a CSP source list could be confused by is
        // refused, and the fallback fails closed.
        for hostile in ["evil.example/", "a b", "x;script-src *", ""] {
            headers.insert(header::HOST, HeaderValue::from_str(hostile).unwrap());
            assert_eq!(preview_asset_sources(&headers), None, "{hostile}");
        }
        let fallback = preview_policy(preview_asset_sources(&HeaderMap::new()).as_deref());
        assert!(fallback.contains("script-src 'self'"));
        assert!(!fallback.contains("http://"));
    }

    #[test]
    fn collision_names_add_a_counter_before_the_extension() {
        assert_eq!(candidate_name("report.pdf", 0), "report.pdf");
        assert_eq!(candidate_name("report.pdf", 2), "report-2.pdf");
        assert_eq!(candidate_name("archive.tar.gz", 1), "archive.tar-1.gz");
        assert_eq!(candidate_name("README", 1), "README-1");
        assert_eq!(candidate_name(".env", 1), ".env-1");
    }

    #[test]
    fn attachment_names_are_a_single_segment() {
        assert_eq!(
            validate_attachment_name("  notes.txt ").unwrap(),
            "notes.txt"
        );
        assert!(validate_attachment_name("dir/notes.txt").is_err());
        assert!(validate_attachment_name("..\\notes.txt").is_err());
        assert!(validate_attachment_name("..").is_err());
        assert!(validate_attachment_name(".").is_err());
        assert!(validate_attachment_name("").is_err());
        assert!(validate_attachment_name("a\0b").is_err());
        assert!(validate_attachment_name(&"a".repeat(MAX_ATTACHMENT_NAME_BYTES + 1)).is_err());
        // Unicode and spaces are ordinary file names, not attacks.
        assert!(validate_attachment_name("Résumé final.pdf").is_ok());
    }

    #[test]
    fn session_ids_cannot_escape_the_upload_directory() {
        assert!(validate_session_id("s_ab12cd34").is_ok());
        for bad in ["", "../x", "s_ab/12", "s ab", "s_ab\\12", "."] {
            assert!(validate_session_id(bad).is_err(), "{bad} must be refused");
        }
        assert!(validate_session_id(&"s".repeat(MAX_SESSION_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn store_upload_writes_under_the_session_directory() {
        let (dir, ws) = workspace();
        let stored = store_upload(&ws, "s_test", "notes.txt", b"hello").unwrap();
        assert_eq!(stored.path, ".karucode/attachments/s_test/notes.txt");
        assert!(!stored.deduplicated);
        assert_eq!(
            std::fs::read(dir.join(".karucode/attachments/s_test/notes.txt")).unwrap(),
            b"hello"
        );

        // A second upload of the same name must not overwrite the first.
        let again = store_upload(&ws, "s_test", "notes.txt", b"second").unwrap();
        assert_eq!(again.name, "notes-1.txt");
        assert!(again.deduplicated);
        assert_eq!(
            std::fs::read(dir.join(".karucode/attachments/s_test/notes.txt")).unwrap(),
            b"hello"
        );

        // A directory already occupying the name is a collision, not a crash.
        let squatted = dir.join(".karucode/attachments/s_dir");
        std::fs::create_dir_all(squatted.join("taken.txt")).unwrap();
        let stored = store_upload(&ws, "s_dir", "taken.txt", b"data").unwrap();
        assert_eq!(stored.name, "taken-1.txt");
        assert!(stored.deduplicated);

        // Attachments can hold secrets, so they are not world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(dir.join(".karucode/attachments/s_test/notes.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
            let dir_mode = std::fs::metadata(dir.join(".karucode/attachments/s_test"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn store_upload_refuses_a_symlinked_attachment_root() {
        let (dir, ws) = workspace();
        let outside = std::env::temp_dir().join(format!(
            "kc-server-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(dir.join(".karucode")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(UPLOAD_ROOT)).unwrap();

        let err = store_upload(&ws, "s_test", "notes.txt", b"hello").unwrap_err();
        assert_eq!(err.code, "path_escape");
        assert!(!outside.join("s_test").exists());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    // -----------------------------------------------------------------------
    // PUT /api/workspace/file (the editor's save)
    // -----------------------------------------------------------------------

    #[test]
    fn write_replaces_a_file_and_reports_the_new_size() {
        let (dir, ws) = workspace();
        let value =
            write_file_blocking(&ws, "README.md", "# bye\n", Some(5), false, false).unwrap();
        assert_eq!(value["path"], "README.md");
        assert_eq!(value["size"], 6);
        assert_eq!(value["created"], false);
        assert_eq!(value["language"], "markdown");
        assert_eq!(
            std::fs::read_to_string(dir.join("README.md")).unwrap(),
            "# bye\n"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_refuses_a_stale_expected_size_unless_forced() {
        let (dir, ws) = workspace();
        // README.md is 5 bytes; the editor thinks it is 4.
        let err = write_file_blocking(&ws, "README.md", "x", Some(4), false, false).unwrap_err();
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert!(err.message.contains("changed on disk"));
        // Nothing was written.
        assert_eq!(
            std::fs::read_to_string(dir.join("README.md")).unwrap(),
            "# hi\n"
        );
        // `force` is the user's answer to that prompt.
        write_file_blocking(&ws, "README.md", "x", Some(4), false, true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("README.md")).unwrap(), "x");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_detects_an_equal_length_edit_by_digest_and_force_still_overrides() {
        let (dir, ws) = workspace();
        let loaded = file_blocking(&ws, "README.md", 1024).unwrap();
        let digest = loaded["digest"].as_str().unwrap().to_string();
        std::fs::write(dir.join("README.md"), "# HI\n").unwrap();
        assert_eq!(loaded["size"], 5, "fixture original size");
        assert_eq!(std::fs::metadata(dir.join("README.md")).unwrap().len(), 5);

        let err = write_file_versioned_blocking(
            &ws,
            "README.md",
            "replacement",
            Some(5),
            Some(&digest),
            false,
            false,
        )
        .unwrap_err();
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert!(err.message.contains("digest"), "{}", err.message);
        assert_eq!(
            std::fs::read_to_string(dir.join("README.md")).unwrap(),
            "# HI\n"
        );

        let saved = write_file_versioned_blocking(
            &ws,
            "README.md",
            "forced",
            Some(5),
            Some(&digest),
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            saved["digest"],
            file_digest(&dir.join("README.md")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("README.md")).unwrap(),
            "forced"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_rejects_a_malformed_digest() {
        let (dir, ws) = workspace();
        for invalid in [
            String::new(),
            "deadbeef".into(),
            format!("sha256:{}", "A".repeat(64)),
        ] {
            assert_eq!(
                write_file_versioned_blocking(
                    &ws,
                    "README.md",
                    "x",
                    None,
                    Some(&invalid),
                    false,
                    false,
                )
                .unwrap_err()
                .status,
                StatusCode::BAD_REQUEST
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_can_create_a_file_only_when_asked() {
        let (dir, ws) = workspace();
        let err = write_file_blocking(&ws, "notes/todo.md", "x", None, false, false).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert!(!dir.join("notes").exists());

        let value = write_file_blocking(&ws, "notes/todo.md", "x", None, true, false).unwrap();
        assert_eq!(value["created"], true);
        assert_eq!(value["size"], 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("notes/todo.md")).unwrap(),
            "x"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_refuses_to_resurrect_a_deleted_file_without_force() {
        let (dir, ws) = workspace();
        std::fs::remove_file(dir.join("README.md")).unwrap();
        // Without `create` the missing file is simply a 404, like a read would be.
        let err = write_file_blocking(&ws, "README.md", "x", Some(5), false, false).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert!(!dir.join("README.md").exists());

        // With `create` the editor is deliberately writing a new file, but it still
        // has to admit that what it loaded is gone before it overwrites nothing.
        let err = write_file_blocking(&ws, "README.md", "x", Some(5), true, false).unwrap_err();
        assert_eq!(err.status, StatusCode::CONFLICT);
        assert!(!dir.join("README.md").exists());

        let value = write_file_blocking(&ws, "README.md", "x", Some(5), true, true).unwrap();
        assert_eq!(value["created"], true);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_is_confined_to_the_workspace() {
        let (dir, ws) = workspace();
        let err = write_file_blocking(&ws, "../../etc/passwd", "x", None, true, true).unwrap_err();
        assert_eq!(err.code, "path_escape");
        let err = write_file_blocking(&ws, "/etc/passwd", "x", None, true, true).unwrap_err();
        assert_eq!(err.code, "path_escape");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn write_refuses_a_directory() {
        let (dir, ws) = workspace();
        let err = write_file_blocking(&ws, "src", "x", None, true, true).unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn write_refuses_a_symlink_that_escapes_the_workspace() {
        let (dir, ws) = workspace();
        let outside = std::env::temp_dir().join(format!(
            "kc-server-write-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link")).unwrap();

        let err = write_file_blocking(&ws, "link/pwned.txt", "x", None, true, true).unwrap_err();
        assert_eq!(err.code, "path_escape");
        assert!(!outside.join("pwned.txt").exists());
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(outside).ok();
    }

    #[test]
    fn the_save_body_limit_leaves_room_for_json_escaping() {
        // The handler reports `413` itself in terms of MAX_WRITE_BYTES, so the
        // transport limit has to be the larger of the two, at the worst-case
        // escaping ratio JSON allows.
        const { assert!(crate::routes::MAX_WRITE_BODY_BYTES > MAX_WRITE_BYTES * 6) };
    }
}
