//! The two halves of "the UI and the token must coexist": a token-protected
//! deployment still has to load the app shell (or nobody can ever enter the
//! token), and a workspace file served as raw bytes must not be able to script
//! the app's origin.
//!
//! Both are end-to-end through the real router, the real middlewares and the real
//! handlers, like every other test in this crate.

mod support;

use axum::http::{header, StatusCode};
use support::{Harness, HarnessBuilder};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A web directory with a shell and one hashed asset, so `GET /` and
/// `GET /assets/...` have something real to serve.
fn web_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-server-security-ui-{name}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body>APP-SHELL</body></html>",
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/index-1234abcd.js"),
        "console.log('bundle')",
    )
    .unwrap();
    dir
}

fn write(harness: &Harness, rel: &str, bytes: &[u8]) {
    let path = harness.workspace_path().join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, bytes).expect("write");
}

/// With `KARUCODE_AUTH_TOKEN` set the browser gets the app shell and its bundle
/// without any credential - it has no way to send one for a top-level navigation
/// or a `<script src>` - while every `/api` path keeps its `401`. Without this the
/// deployment has no UI at all: the bundle 401s, so the app's own auth screen
/// (the only place a token can be typed) never renders.
#[tokio::test]
async fn the_shell_and_assets_load_without_a_token() {
    let web = web_dir("shell");
    let h = HarnessBuilder::new(Vec::new())
        .auth_token(TOKEN)
        .web_dir(web.clone())
        .build()
        .await;

    let shell = h.get("/").await;
    assert_eq!(
        shell.status,
        StatusCode::OK,
        "GET / must load without a token: {}",
        shell.text()
    );
    assert!(
        shell.headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .contains("text/html"),
        "GET / must answer HTML, not a JSON 401: {:?}",
        shell.headers[header::CONTENT_TYPE]
    );
    assert!(shell.text().contains("APP-SHELL"));

    // `index.html` by name, and a client-side deep link, behave the same.
    for path in ["/index.html", "/sessions/s_123"] {
        let response = h.get(path).await;
        assert_eq!(response.status, StatusCode::OK, "GET {path} must load");
        assert!(response.text().contains("APP-SHELL"), "GET {path}");
    }

    let asset = h.get("/assets/index-1234abcd.js").await;
    assert_eq!(
        asset.status,
        StatusCode::OK,
        "the bundle must load without a token"
    );
    assert!(asset.headers[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .contains("javascript"));

    // A *wrong* token on a non-API path still gets the shell, never a 401 body:
    // the middleware must not answer JSON for anything outside `/api`.
    let wrong = h
        .get_with("/", &[("authorization", "Bearer not-the-token")])
        .await;
    assert_eq!(wrong.status, StatusCode::OK);
    assert!(wrong.text().contains("APP-SHELL"));
    assert!(
        !wrong
            .headers
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str().unwrap_or_default().contains("json"))
            .unwrap_or(false),
        "a non-API path must never answer JSON"
    );

    // The API and the operator endpoint stay locked: the shell being public does
    // not leak one byte of data.
    let config = h.get("/api/config").await;
    assert_eq!(config.status, StatusCode::UNAUTHORIZED);
    assert_eq!(config.error_code(), "unauthorized");
    let health = h.get("/api/health").await;
    assert_eq!(health.status, StatusCode::UNAUTHORIZED);
    let unknown = h.get("/api/nope").await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);
    let metrics = h.get("/metrics").await;
    assert_eq!(metrics.status, StatusCode::UNAUTHORIZED);

    // `?token=` is unchanged: still accepted on the SSE stream, never elsewhere.
    let sse = h.get(&format!("/api/health?token={TOKEN}")).await;
    assert_eq!(sse.status, StatusCode::UNAUTHORIZED);
    let (sse_status, _, sse_body) = h.open_events(&format!("?token={TOKEN}"), &[]).await;
    assert_eq!(sse_status, StatusCode::OK);
    drop(sse_body);

    // With the header, the API answers normally.
    let authorized = h
        .get_with(
            "/api/config",
            &[("authorization", &format!("Bearer {TOKEN}"))],
        )
        .await;
    assert_eq!(authorized.status, StatusCode::OK, "{}", authorized.text());
    let metrics = h
        .get_with("/metrics", &[("authorization", &format!("Bearer {TOKEN}"))])
        .await;
    assert_eq!(metrics.status, StatusCode::OK);

    // The shell is still confined to what the web directory holds: a traversal
    // attempt falls through to the SPA, it does not read outside it.
    let escape = h.get("/../../etc/passwd").await;
    assert_eq!(escape.status, StatusCode::OK);
    assert!(
        !escape.text().contains("root:"),
        "a traversal attempt must not read a file outside the web dir"
    );

    std::fs::remove_dir_all(web).ok();
}

/// `GET /api/workspace/raw` guesses `Content-Type` from a file name the agent (or
/// a prompt injection) can choose, and the response shares the app's origin - so
/// without a sandbox, `<script src="/api/workspace/raw?path=x.js">` would be a
/// same-origin script able to `PATCH /api/config`. The route-local CSP makes the
/// document an opaque origin with script disabled.
#[tokio::test]
async fn raw_workspace_files_cannot_execute() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    let html = b"<html><body><script>alert(1)</script></body></html>";
    write(&h, "evil.html", html);
    // A file with a script extension is the sharper case: `<script src>` needs a
    // JavaScript content type to be treated as code at all.
    write(&h, "evil.js", b"fetch('/api/config');");

    for path in ["evil.html", "evil.js"] {
        let response = h.get(&format!("/api/workspace/raw?path={path}")).await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.text());

        let csp = response.headers[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .expect("the raw route must carry its own CSP")
            .to_string();
        assert!(
            csp.contains("sandbox"),
            "the response must be sandboxed (`{path}`): {csp}"
        );
        assert!(
            !csp.contains("allow-scripts"),
            "the sandbox must not re-enable script (`{path}`): {csp}"
        );
        assert!(
            !csp.contains("'self'"),
            "a sandboxed response must not inherit the app CSP's same-origin \
             allowances (`{path}`): {csp}"
        );
        assert_eq!(
            response.headers[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff",
            "`{path}` must not be sniffed into a script"
        );
        assert_eq!(
            response.headers[header::CACHE_CONTROL],
            "no-store",
            "a workspace file must not be cached"
        );
    }

    // The guessed type and `inline` disposition are deliberately KEPT: an
    // `<img src>` and a PDF `<iframe>` hotlink this route, so switching it to
    // `attachment` would break both previews. The sandbox is what neutralises
    // the HTML, not the disposition.
    //
    // `X-Frame-Options` is the other half of that: the sandboxed response must NOT
    // carry `DENY`, or our own preview iframe cannot embed it (a PDF pane renders
    // nothing with it). It is redundant on an opaque-origin document anyway. The
    // pages we serve ourselves keep DENY - asserted here so the exemption cannot
    // widen by accident.
    let html_response = h.get("/api/workspace/raw?path=evil.html").await;
    assert!(
        !html_response.headers.contains_key(header::X_FRAME_OPTIONS),
        "a sandboxed document must stay embeddable by our own previews, or the \
         PDF/image preview breaks"
    );
    let api_response = h.get("/api/config").await;
    assert_eq!(
        api_response.headers[header::X_FRAME_OPTIONS],
        "DENY",
        "an ordinary API response still refuses framing"
    );
    assert_eq!(
        html_response.headers[header::CONTENT_TYPE],
        "text/html",
        "the type stays so image/PDF previews keep working"
    );
    assert_eq!(
        html_response.headers[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap(),
        "inline"
    );

    // The same treatment applies to an image (the legitimate hotlink case): the
    // sandbox header is unconditional, not a per-type decision.
    write(&h, "logo.png", b"\x89PNG\r\n\x1a\n");
    let image = h.get("/api/workspace/raw?path=logo.png").await;
    assert_eq!(image.status, StatusCode::OK);
    assert_eq!(
        image.headers[header::CONTENT_TYPE],
        "image/png",
        "an image keeps its type, or the preview would not render"
    );
    assert!(image.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains("sandbox"));
}

/// The shell is public, but it is still only the shell: `raw` needs the token
/// like every other `/api` route, so an unauthenticated `<img src>` cannot read a
/// workspace file either.
#[tokio::test]
async fn the_raw_route_still_needs_the_token() {
    let h = HarnessBuilder::new(Vec::new())
        .auth_token(TOKEN)
        .build()
        .await;
    write(&h, "secret.txt", b"private");

    let anonymous = h.get("/api/workspace/raw?path=secret.txt").await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    let authorized = h
        .get_with(
            "/api/workspace/raw?path=secret.txt",
            &[("authorization", &format!("Bearer {TOKEN}"))],
        )
        .await;
    assert_eq!(authorized.status, StatusCode::OK);
    assert_eq!(authorized.body.as_ref(), b"private");
    assert!(authorized.headers[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap()
        .contains("sandbox"));
}
