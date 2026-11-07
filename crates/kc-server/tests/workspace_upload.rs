//! End-to-end tests for `POST /api/workspace/upload`, the attachment endpoint
//! the web composer calls.
//!
//! The endpoint takes a **raw** request body, which the shared harness
//! (`tests/support/mod.rs`) cannot express - it only serialises JSON - so these
//! tests build their requests directly against the harness router. Everything
//! else (middlewares, SQLite store, project registry) is the production wiring.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use support::{Harness, HarnessBuilder, TestResponse};
use tower::ServiceExt;

const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
const UPLOAD_PATH: &str = "/api/workspace/upload";

// ---------------------------------------------------------------------------
// Raw-body request builder
// ---------------------------------------------------------------------------

struct Upload<'a> {
    query: String,
    body: Vec<u8>,
    content_type: Option<&'a str>,
    headers: Vec<(&'a str, String)>,
    csrf: bool,
}

impl<'a> Upload<'a> {
    fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            body: Vec::new(),
            // A file upload always carries a type in practice; the endpoint
            // treats a missing one as `application/octet-stream`.
            content_type: Some("text/plain"),
            headers: Vec::new(),
            csrf: true,
        }
    }

    fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    fn content_type(mut self, content_type: &'a str) -> Self {
        self.content_type = Some(content_type);
        self
    }

    fn without_content_type(mut self) -> Self {
        self.content_type = None;
        self
    }

    fn header(mut self, name: &'a str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    fn without_csrf(mut self) -> Self {
        self.csrf = false;
        self
    }

    async fn send(self, h: &Harness) -> TestResponse {
        let uri = format!("{UPLOAD_PATH}?{}", self.query);
        let mut builder = Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", "127.0.0.1:8787")
            .header("accept", "application/json");
        if self.csrf {
            builder = builder.header("x-karucode-client", "web");
        }
        if let Some(content_type) = self.content_type {
            builder = builder.header("content-type", content_type);
        }
        for (name, value) in &self.headers {
            builder = builder.header(*name, value.as_str());
        }
        let request = builder
            .body(Body::from(self.body))
            .expect("build upload request");

        let response = h
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router call");
        let status = response.status();
        let headers = response.headers().clone();
        let body = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .expect("read response body");
        TestResponse {
            status,
            headers,
            body,
        }
    }
}

async fn create_session(h: &Harness) -> String {
    let created = h
        .post("/api/sessions", json!({ "title": "attach a file" }))
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    created.json()["id"]
        .as_str()
        .expect("session id")
        .to_string()
}

fn attachment_dir(h: &Harness, session: &str) -> std::path::PathBuf {
    h.workspace_path()
        .join(".karucode")
        .join("attachments")
        .join(session)
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upload_writes_the_file_and_returns_the_documented_json() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let response = Upload::new(format!("session={session}&name=notes.txt"))
        .body(b"hello attachment")
        .send(&h)
        .await;

    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    let body = response.json();
    assert_eq!(body["session"], session.as_str());
    assert_eq!(body["name"], "notes.txt");
    assert_eq!(
        body["path"],
        format!(".karucode/attachments/{session}/notes.txt")
    );
    assert_eq!(body["size"], 16);
    assert_eq!(body["content_type"], "text/plain");
    assert_eq!(body["deduplicated"], false);
    assert!(
        body["workspace"].as_str().unwrap().ends_with("workspace"),
        "workspace is the project root: {}",
        body["workspace"]
    );

    // The file is on disk, byte for byte, under the session directory.
    let stored = attachment_dir(&h, &session).join("notes.txt");
    assert_eq!(std::fs::read(&stored).unwrap(), b"hello attachment");

    // ... and readable through the same path the file tools use.
    let read = h
        .get(&format!(
            "/api/workspace/file?path={}",
            body["path"].as_str().unwrap()
        ))
        .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text());
    assert_eq!(read.json()["content"], "hello attachment");
}

#[tokio::test]
async fn upload_preserves_binary_bytes_and_the_declared_type() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let bytes: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let response = Upload::new(format!("session={session}&name=blob.bin"))
        .body(bytes.clone())
        .content_type("application/octet-stream")
        .send(&h)
        .await;

    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert_eq!(response.json()["content_type"], "application/octet-stream");
    assert_eq!(
        std::fs::read(attachment_dir(&h, &session).join("blob.bin")).unwrap(),
        bytes
    );
}

#[tokio::test]
async fn upload_defaults_the_content_type_when_the_client_sends_none() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let response = Upload::new(format!("session={session}&name=data"))
        .body(b"x")
        .without_content_type()
        .send(&h)
        .await;

    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert_eq!(response.json()["content_type"], "application/octet-stream");
}

#[tokio::test]
async fn a_url_encoded_name_with_spaces_and_unicode_is_stored_verbatim() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let response = Upload::new(format!(
        "session={session}&name=R%C3%A9sum%C3%A9%20final.pdf"
    ))
    .body(b"%PDF-1.4")
    .content_type("application/pdf; charset=binary")
    .send(&h)
    .await;

    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    let body = response.json();
    assert_eq!(body["name"], "Résumé final.pdf");
    // Parameters are stripped from the echoed MIME type.
    assert_eq!(body["content_type"], "application/pdf");
    assert!(attachment_dir(&h, &session)
        .join("Résumé final.pdf")
        .exists());
}

// ---------------------------------------------------------------------------
// Collisions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_upload_of_the_same_name_is_renamed_not_overwritten() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let first = Upload::new(format!("session={session}&name=report.pdf"))
        .body(b"first")
        .send(&h)
        .await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.text());
    assert_eq!(first.json()["name"], "report.pdf");
    assert_eq!(first.json()["deduplicated"], false);

    let second = Upload::new(format!("session={session}&name=report.pdf"))
        .body(b"second")
        .send(&h)
        .await;
    assert_eq!(second.status, StatusCode::CREATED, "{}", second.text());
    assert_eq!(second.json()["name"], "report-1.pdf");
    assert_eq!(second.json()["deduplicated"], true);
    assert_eq!(
        second.json()["path"],
        format!(".karucode/attachments/{session}/report-1.pdf")
    );

    let third = Upload::new(format!("session={session}&name=report.pdf"))
        .body(b"third")
        .send(&h)
        .await;
    assert_eq!(third.json()["name"], "report-2.pdf");

    // Nothing was clobbered: each upload kept its own bytes.
    let dir = attachment_dir(&h, &session);
    assert_eq!(std::fs::read(dir.join("report.pdf")).unwrap(), b"first");
    assert_eq!(std::fs::read(dir.join("report-1.pdf")).unwrap(), b"second");
    assert_eq!(std::fs::read(dir.join("report-2.pdf")).unwrap(), b"third");
}

// ---------------------------------------------------------------------------
// Rejections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn traversal_in_the_name_is_refused_and_nothing_is_written() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    // Percent-encoded separators and dots must be rejected after decoding, not
    // before: `%2e%2e%2f` *is* `../` to the file system.
    let names = [
        "..%2Fescape.txt",
        "%2E%2E%2Fescape.txt",
        "%2e%2e%5cescape.txt",
        "nested%2Fescape.txt",
        "..",
        ".",
        "",
        "bad%00name.txt",
        &"a".repeat(200),
    ];
    for name in names {
        let response = Upload::new(format!("session={session}&name={name}"))
            .body(b"payload")
            .send(&h)
            .await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "name `{name}` must be refused: {}",
            response.text()
        );
        assert_eq!(response.error_code(), "bad_request");
    }

    // A missing `name` is refused too, rather than silently inventing one.
    let missing = Upload::new(format!("session={session}"))
        .body(b"payload")
        .send(&h)
        .await;
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
    assert!(missing.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("`name` is required"));

    // No escape landed anywhere: not beside the workspace, not in the
    // attachments root, not on the parent directory.
    assert!(!h.workspace_path().join("escape.txt").exists());
    assert!(!h
        .workspace_path()
        .join(".karucode/attachments/escape.txt")
        .exists());
    let root = h.workspace_path().parent().unwrap();
    assert!(!root.join("escape.txt").exists());
    assert!(!root.join("nested").exists());
    assert!(
        !attachment_dir(&h, &session).exists()
            || attachment_dir(&h, &session)
                .read_dir()
                .unwrap()
                .next()
                .is_none(),
        "a rejected upload must not create files"
    );
}

#[tokio::test]
async fn an_unusable_session_is_refused() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    for query in [
        "name=notes.txt",                       // missing
        "session=&name=notes.txt",              // empty
        "session=..%2F..%2Ftmp&name=notes.txt", // traversal
        "session=s_ab%2F12&name=notes.txt",     // separator
        "session=s%20ab&name=notes.txt",        // whitespace
    ] {
        let response = Upload::new(query).body(b"x").send(&h).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "query `{query}`: {}",
            response.text()
        );
        assert_eq!(response.error_code(), "bad_request");
    }

    // Well-formed but unknown: a 404 naming the session, not a 400.
    let unknown = Upload::new("session=s_00000000000000000000000000000000&name=notes.txt")
        .body(b"x")
        .send(&h)
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error_code(), "not_found");
    assert!(unknown.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does not exist"));
}

#[tokio::test]
async fn an_empty_body_is_refused() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let response = Upload::new(format!("session={session}&name=empty.txt"))
        .body(Vec::new())
        .send(&h)
        .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.error_code(), "bad_request");
    assert!(response.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("empty"));
    assert!(!attachment_dir(&h, &session).join("empty.txt").exists());
}

#[tokio::test]
async fn an_oversized_body_is_refused_before_it_is_stored() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    // A declared size over the limit fails without buffering the body at all.
    let declared = Upload::new(format!("session={session}&name=big.bin"))
        .body(b"small")
        .header("content-length", (MAX_UPLOAD_BYTES + 1).to_string())
        .send(&h)
        .await;
    assert_eq!(declared.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(declared.error_code(), "payload_too_large");
    assert!(declared.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains(&MAX_UPLOAD_BYTES.to_string()));

    // ... and so does a body that actually exceeds it.
    let body = vec![b'x'; MAX_UPLOAD_BYTES + 1];
    let oversized = Upload::new(format!("session={session}&name=big.bin"))
        .body(body)
        .send(&h)
        .await;
    assert_eq!(oversized.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(oversized.error_code(), "payload_too_large");
    assert!(!attachment_dir(&h, &session).join("big.bin").exists());
}

#[tokio::test]
async fn form_encodings_are_refused_with_a_pointer_to_the_raw_body() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    for content_type in [
        "multipart/form-data; boundary=x",
        "application/x-www-form-urlencoded",
    ] {
        let response = Upload::new(format!("session={session}&name=notes.txt"))
            .body(b"--x--")
            .content_type(content_type)
            .send(&h)
            .await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{content_type}: {}",
            response.text()
        );
        assert!(response.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("raw request body"));
    }

    let bad_type = Upload::new(format!("session={session}&name=notes.txt"))
        .body(b"x")
        .content_type("not-a-mime")
        .send(&h)
        .await;
    assert_eq!(bad_type.status, StatusCode::BAD_REQUEST);
    assert!(!attachment_dir(&h, &session).join("notes.txt").exists());
}

#[tokio::test]
async fn upload_without_the_csrf_header_is_refused() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let session = create_session(&h).await;

    let response = Upload::new(format!("session={session}&name=notes.txt"))
        .body(b"x")
        .without_csrf()
        .send(&h)
        .await;

    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.error_code(), "permission_denied");
    assert!(!attachment_dir(&h, &session).join("notes.txt").exists());
}

// ---------------------------------------------------------------------------
// Project scoping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upload_follows_the_session_project_when_no_project_is_given() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    // A second project inside the default workspace, like the UI's rail.
    let project_dir = h.workspace_path().join("project-two");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_path = project_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let created = h
        .post(
            "/api/projects",
            json!({ "name": "Two", "path": project_path }),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let project_id = created.json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let session = h
        .post("/api/sessions", json!({ "project": project_id }))
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = Upload::new(format!("session={session}&name=notes.txt"))
        .body(b"project two")
        .send(&h)
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert_eq!(response.json()["workspace"].as_str().unwrap(), project_path);

    // Stored in the session's project, not in the default workspace.
    let stored = project_dir
        .join(".karucode/attachments")
        .join(&session)
        .join("notes.txt");
    assert_eq!(std::fs::read(&stored).unwrap(), b"project two");
    assert!(!h
        .workspace_path()
        .join(".karucode/attachments")
        .join(&session)
        .exists());

    // An explicit `project` overrides the session's project, and an unknown one
    // is a 404 instead of a silent fallback.
    let explicit = Upload::new(format!(
        "session={session}&name=default.txt&project={}",
        project_id
    ))
    .body(b"explicit")
    .send(&h)
    .await;
    assert_eq!(explicit.status, StatusCode::CREATED, "{}", explicit.text());

    let unknown = Upload::new(format!("session={session}&name=x.txt&project=p_nope"))
        .body(b"x")
        .send(&h)
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn upload_without_a_project_uses_the_recorded_control_workspace() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let created = h
        .post(
            "/api/control/sessions",
            json!({ "title": "Attachment scope" }),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let created = created.json();
    let session = created["session_id"].as_str().unwrap().to_string();
    let recorded = created["workspace"].as_str().unwrap().to_string();
    assert!(created["project_id"].is_null());
    assert_ne!(
        std::path::Path::new(&recorded)
            .canonicalize()
            .unwrap()
            .as_path(),
        h.workspace_path().canonicalize().unwrap().as_path()
    );

    let response = Upload::new(format!("session={session}&name=control.txt"))
        .body(b"control scratch")
        .send(&h)
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.text());
    assert_eq!(response.json()["workspace"].as_str().unwrap(), recorded);
    let stored = std::path::Path::new(&recorded)
        .join(".karucode/attachments")
        .join(&session)
        .join("control.txt");
    assert_eq!(std::fs::read(stored).unwrap(), b"control scratch");
    assert!(
        !h.workspace_path()
            .join(".karucode/attachments")
            .join(&session)
            .exists(),
        "a project-less session must never fall back to the default project"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_attachment_root_cannot_be_used_to_escape_the_workspace() {
    // Project-only confinement: the sibling `outside` directory below lives under
    // the system temp dir, which is a second allowed root by default (see the
    // `workspace_temp_dir_is_a_root_by_default` test in api.rs). Without
    // `strict_workspace` this asserts nothing, because the link target is inside a
    // legal root. The symlink must still be refused when the workspace is the only
    // root - that is the rule this test exists for.
    let h = HarnessBuilder::new(Vec::new())
        .strict_workspace()
        .build()
        .await;
    let session = create_session(&h).await;

    let outside = h.workspace_path().parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(h.workspace_path().join(".karucode")).unwrap();
    std::os::unix::fs::symlink(&outside, h.workspace_path().join(".karucode/attachments")).unwrap();

    let response = Upload::new(format!("session={session}&name=notes.txt"))
        .body(b"secret")
        .send(&h)
        .await;

    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.text()
    );
    assert_eq!(response.error_code(), "path_escape");
    assert!(!outside.join(&session).exists());
}

#[tokio::test]
async fn the_upload_route_only_accepts_post_and_keeps_the_json_404() {
    let h = HarnessBuilder::new(Vec::new()).build().await;

    // Reading the upload URL is a method error, not a silent 200 with the SPA.
    let read = h.get(UPLOAD_PATH).await;
    assert_eq!(read.status, StatusCode::METHOD_NOT_ALLOWED);

    // Adding a route must not swallow the `/api` 404 contract.
    let unknown = h.post("/api/workspace/nope", json!({})).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error_code(), "not_found");
}
