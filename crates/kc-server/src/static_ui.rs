//! Static frontend serving.
//!
//! Strategy:
//! * `KARUCODE_WEB_DIR` (or `Config::web_dir`) is consulted first so `npm run
//!   dev`/a rebuilt `dist` can be served without recompiling.
//! * otherwise the `web/dist` folder embedded at compile time (`embed-ui`
//!   feature, on by default) is used. `build.rs` refreshes that folder from the
//!   frontend sources, and rust-embed's `debug-embed` feature makes dev builds
//!   embed it as well instead of reading the folder at runtime — so a debug
//!   `karucode` is as self-contained as a release one.
//! * if neither has a file, non-`/api` paths fall back to `index.html` (SPA
//!   routing) and finally to a placeholder page that explains how to build the
//!   UI. A missing `web/dist` is therefore not a build error.
//!
//! Hashed assets get an immutable cache header; `index.html` is always
//! revalidated so a rebuilt UI is picked up immediately.

use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::io::Read;

/// A configured web directory is writable input, not a trusted compiled bundle.
/// Keep its per-response memory bounded just like the workspace preview routes.
const MAX_DISK_ASSET_BYTES: u64 = 32 * 1024 * 1024;

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
struct EmbeddedAssets;

pub struct WebAssets {
    dir: Option<std::path::PathBuf>,
}

pub struct Asset {
    pub bytes: Vec<u8>,
    pub mime: String,
}

impl WebAssets {
    pub fn new(dir: Option<std::path::PathBuf>) -> Self {
        Self { dir }
    }

    /// Development directory, if one was configured.
    pub fn dir(&self) -> Option<&std::path::Path> {
        self.dir.as_deref()
    }

    /// Look up a URL path (without the leading slash). Returns `None` when the
    /// asset exists in neither the dev directory nor the embedded bundle.
    pub async fn get(&self, path: &str) -> Option<Asset> {
        let relative = normalize(path)?;
        if let Some(dir) = &self.dir {
            let dir = dir.clone();
            let disk_relative = relative.clone();
            // Directory traversal and disk reads can block on network filesystems;
            // neither belongs on the async executor.
            if let Ok(Some(asset)) = tokio::task::spawn_blocking(move || {
                read_from_dir(&dir, &disk_relative, MAX_DISK_ASSET_BYTES)
            })
            .await
            {
                return Some(asset);
            }
        }
        #[cfg(feature = "embed-ui")]
        {
            if let Some(file) = EmbeddedAssets::get(&relative) {
                if file.data.len() as u64 > MAX_DISK_ASSET_BYTES {
                    return None;
                }
                return Some(Asset {
                    bytes: file.data.into_owned(),
                    mime: mime_for(&relative),
                });
            }
        }
        let _ = path;
        None
    }

    /// Whether an embedded or on-disk `index.html` exists.
    pub async fn has_index(&self) -> bool {
        self.get("index.html").await.is_some()
    }
}

/// Reject traversal and absolute paths. Percent-encoded dot segments are left
/// encoded (they cannot match a real file, so they end up at the SPA fallback).
fn normalize(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some("index.html".to_string());
    }
    if trimmed.contains('\0') || trimmed.contains('\\') {
        return None;
    }
    let mut parts = Vec::new();
    for segment in trimmed.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Some("index.html".to_string());
    }
    Some(parts.join("/"))
}

/// Read a bounded regular file from the dev directory. Every candidate is
/// canonicalised, not only a final-component symlink: an intermediate directory
/// symlink is an equally effective escape and `starts_with` on the unresolved
/// path cannot detect it.
fn read_from_dir(dir: &std::path::Path, relative: &str, limit: u64) -> Option<Asset> {
    let root = std::fs::canonicalize(dir).ok()?;
    let candidate = std::fs::canonicalize(dir.join(relative)).ok()?;
    if !candidate.starts_with(&root) {
        return None;
    }
    // Do this before opening: opening a FIFO for read can wait forever for its
    // writer even in a blocking worker, and devices/sockets are not web assets.
    let meta = std::fs::metadata(&candidate).ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    let file = std::fs::File::open(&candidate).ok()?;
    let mut bytes = Vec::new();
    // Metadata is only a snapshot. The extra byte makes growth after that
    // snapshot fail closed without ever allocating the file's unbounded size.
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > limit {
        return None;
    }
    Some(Asset {
        bytes,
        mime: mime_for(relative),
    })
}

fn mime_for(path: &str) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

fn content_type(mime: &str) -> String {
    match mime {
        "text/html"
        | "text/css"
        | "text/javascript"
        | "application/javascript"
        | "text/plain"
        | "application/json"
        | "image/svg+xml" => format!("{mime}; charset=utf-8"),
        other => other.to_string(),
    }
}

/// True for Vite-style content-hashed filenames such as `index-a1b2c3d4.js` or
/// `index-q8U6r98I.js` (Vite uses a base64-like alphabet, not only hex).
pub fn is_hashed_asset(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    let Some((stem, _ext)) = file.rsplit_once('.') else {
        return false;
    };
    let Some((_, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    hash.len() >= 8
        && hash
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn cache_control(path: &str) -> &'static str {
    if path == "index.html" {
        "no-cache"
    } else if is_hashed_asset(path) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn asset_response(asset: Asset, path: &str) -> Response {
    let mut response = Response::new(Body::from(asset.bytes));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type(&asset.mime))
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control(path)),
    );
    response
}

fn placeholder() -> Response {
    const BODY: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>karucode — UI not built</title>
<style>
 body{font:15px/1.5 system-ui,sans-serif;margin:0;background:#0f1115;color:#e6e6e6;
      display:flex;align-items:center;justify-content:center;min-height:100vh}
 main{max-width:34rem;padding:2rem}
 h1{font-size:1.3rem;margin:0 0 .75rem}
 code{background:#1b1f27;padding:.15rem .35rem;border-radius:4px}
 pre{background:#1b1f27;padding:.9rem 1rem;border-radius:6px;overflow:auto}
 p{color:#b9c0cc}
</style></head>
<body><main>
<h1>karucode API is running — the web UI is not built</h1>
<p>The server did not find <code>web/dist/index.html</code> (and
<code>KARUCODE_WEB_DIR</code> is not set).</p>
<p>Build the frontend once, or point the server at an existing build:</p>
<pre>cd web &amp;&amp; npm install &amp;&amp; npm run build
# or
KARUCODE_WEB_DIR=/path/to/web/dist karucode</pre>
<p>The API is available under <code>/api</code>; health is
<code>GET /api/health</code>.</p>
</main></body></html>"#;
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        BODY,
    )
        .into_response()
}

/// Handler for every non-`/api` path: serve the asset, else the SPA shell.
pub async fn serve_spa(State(state): State<AppState>, uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let requested = if raw.is_empty() { "index.html" } else { raw };

    if let Some(asset) = state.web().get(requested).await {
        return asset_response(asset, requested);
    }
    if requested != "index.html" {
        if let Some(asset) = state.web().get("index.html").await {
            return asset_response(asset, "index.html");
        }
    }
    placeholder()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_rejected() {
        assert!(normalize("../etc/passwd").is_none());
        assert!(normalize("assets/../../secret").is_none());
        assert!(normalize("a\\b").is_none());
        assert!(normalize("a\0b").is_none());
    }

    #[test]
    fn normal_paths_are_kept() {
        assert_eq!(normalize("/").as_deref(), Some("index.html"));
        assert_eq!(normalize("assets/app.js").as_deref(), Some("assets/app.js"));
        assert_eq!(
            normalize("./assets//app.js").as_deref(),
            Some("assets/app.js")
        );
    }

    #[tokio::test]
    async fn disk_assets_are_read_and_confined() {
        let dir = std::env::temp_dir().join(format!(
            "kc-server-web-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();
        let assets = WebAssets::new(Some(dir.clone()));
        assert!(assets.has_index().await);
        let index = assets.get("/").await.unwrap();
        assert_eq!(index.mime, "text/html");
        assert!(String::from_utf8_lossy(&index.bytes).contains("ok"));
        assert!(assets.get("../etc/passwd").await.is_none());

        #[cfg(unix)]
        {
            let outside = dir.with_extension("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("secret.txt"), "secret").unwrap();
            std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("link.txt")).unwrap();
            assert!(assets.get("link.txt").await.is_none());
            // The old check looked only at a final-component symlink. An escape
            // through a linked parent directory must be rejected just as firmly.
            std::os::unix::fs::symlink(&outside, dir.join("linked-parent")).unwrap();
            assert!(assets.get("linked-parent/secret.txt").await.is_none());
            std::os::unix::fs::symlink(dir.join("index.html"), dir.join("inside.html")).unwrap();
            assert!(assets.get("inside.html").await.is_some());
            std::fs::remove_dir_all(&outside).ok();
        }

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn disk_assets_are_bounded_before_growing_the_read_buffer() {
        let dir = std::env::temp_dir().join(format!(
            "kc-server-web-limit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), b"12345").unwrap();
        assert!(read_from_dir(&dir, "index.html", 4).is_none());
        assert_eq!(
            read_from_dir(&dir, "index.html", 5).map(|asset| asset.bytes),
            Some(b"12345".to_vec())
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn hashed_assets_are_detected() {
        assert!(is_hashed_asset("assets/index-a1b2c3d4.js"));
        assert!(is_hashed_asset("assets/vendor-0123abcd.css"));
        assert!(is_hashed_asset("assets/index-q8U6r98I.js"));
        assert!(!is_hashed_asset("assets/index.js"));
        assert!(!is_hashed_asset("favicon.ico"));
        assert_eq!(cache_control("index.html"), "no-cache");
        assert!(cache_control("assets/index-a1b2c3d4.js").contains("immutable"));
    }
}
