//! End-to-end tests for `GET /api/projects/:id/favicon`, the icon the rail draws
//! on each project square.

mod support;

use axum::http::{header, StatusCode};
use serde_json::json;
use support::{Harness, HarnessBuilder};

/// A minimal, real ICO header; the endpoint only sniffs the magic bytes.
const ICO: &[u8] = b"\x00\x00\x01\x00\x01\x00\x10\x10\x00\x00\x01\x00\x20\x00";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR";

fn write(harness: &Harness, rel: &str, bytes: &[u8]) {
    let path = harness.workspace_path().join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(path, bytes).expect("write");
}

/// The default project's id, which the bootstrap seeds for the workspace.
async fn default_project_id(h: &Harness) -> String {
    let listed = h.get("/api/projects").await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    listed.json()["projects"][0]["id"]
        .as_str()
        .expect("project id")
        .to_string()
}

/// A project with no favicon is the normal case. It must not be a 4xx: every
/// browser logs those as `Failed to load resource` in the console, so an expected
/// absence looked like a broken deployment. `204 No Content` says the same thing
/// quietly, and the rail draws initials.
#[tokio::test]
async fn a_project_without_an_icon_is_an_empty_204() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let id = default_project_id(&h).await;

    let response = h.get(&format!("/api/projects/{id}/favicon")).await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert!(response.body.is_empty(), "204 must carry no body");
    assert!(
        response.headers.get(header::CONTENT_TYPE).is_none(),
        "204 must not claim a content type"
    );
}

#[tokio::test]
async fn an_unknown_project_is_a_404_too() {
    // Unlike "this project ships no icon", an unknown id is a caller mistake, so
    // it keeps the documented 404 rather than the quiet 204.
    let h = HarnessBuilder::new(vec![]).build().await;
    let response = h.get("/api/projects/p_nope/favicon").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.error_code(), "not_found");
}

/// The rail renders each icon from an object URL (`blob:`). The page CSP must
/// allow that scheme, or every icon is fetched successfully and then blocked by
/// the browser — which is exactly what happened when `img-src` was `'self' data:`
/// only. This test pins the scheme so it cannot be dropped again.
#[tokio::test]
async fn the_page_csp_allows_blob_images_for_the_rail() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let response = h.get("/api/health").await;
    let csp = response.headers[axum::http::header::CONTENT_SECURITY_POLICY]
        .to_str()
        .expect("CSP header")
        .to_string();

    let img_src = csp
        .split(';')
        .map(str::trim)
        .find(|directive| directive.starts_with("img-src"))
        .unwrap_or_else(|| panic!("no img-src directive in: {csp}"));
    for allowed in ["'self'", "data:", "blob:"] {
        assert!(
            img_src.contains(allowed),
            "`{allowed}` must stay allowed in `{img_src}`"
        );
    }
    // Remote images stay blocked: a prompt-injected Markdown image must not be
    // able to phone home, which is what this directive is for.
    for forbidden in ["http:", "https:", "*"] {
        assert!(
            !img_src.split_whitespace().any(|token| token == forbidden),
            "`{forbidden}` must not be allowed in `{img_src}`"
        );
    }
}

#[tokio::test]
async fn the_projects_own_icon_is_served_with_its_sniffed_type() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let id = default_project_id(&h).await;
    write(&h, "web/public/favicon.ico", ICO);

    let response = h.get(&format!("/api/projects/{id}/favicon")).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    assert_eq!(
        response.headers.get(header::CONTENT_TYPE).unwrap(),
        "image/x-icon"
    );
    // `nosniff` matters because the type came from the bytes, not from a
    // configuration the operator controls.
    assert_eq!(
        response
            .headers
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .unwrap(),
        "nosniff"
    );
    assert!(response.headers.get(header::CACHE_CONTROL).is_some());
    assert_eq!(response.body.as_ref(), ICO);
}

#[tokio::test]
async fn a_png_named_favicon_ico_is_served_as_png() {
    // Plenty of real `favicon.ico` files are PNGs; serving them as `image/x-icon`
    // is what makes a browser refuse to decode them.
    let h = HarnessBuilder::new(vec![]).build().await;
    let id = default_project_id(&h).await;
    write(&h, "favicon.ico", PNG);

    let response = h.get(&format!("/api/projects/{id}/favicon")).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.headers.get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
}

#[tokio::test]
async fn a_git_ignored_icon_is_not_used() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let id = default_project_id(&h).await;
    write(&h, ".gitignore", b"build/\n");
    write(&h, "build/favicon.ico", ICO);

    let response = h.get(&format!("/api/projects/{id}/favicon")).await;
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "an ignored favicon is build output, not the project's icon"
    );

    // An unignored one elsewhere wins, and the shallowest of several wins.
    write(&h, "static/favicon.ico", ICO);
    write(&h, "favicon.ico", ICO);
    let response = h.get(&format!("/api/projects/{id}/favicon")).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body.as_ref(), ICO);
}

#[tokio::test]
async fn the_icon_is_scoped_to_the_project_asked_about() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let default_id = default_project_id(&h).await;

    // A second project *below* the workspace, which is the only shape the default
    // project roots allow.
    let second_dir = h.workspace_path().join("second");
    std::fs::create_dir_all(&second_dir).unwrap();
    let second_path = second_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let created = h
        .post("/api/projects", json!({ "path": second_path }))
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.text());
    let second_id = created.json()["project"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The default project's icon sits at the workspace root, which is *outside*
    // the nested project. A search never climbs, so the nested project must not
    // pick it up: two projects never draw the same icon by accident.
    write(&h, "favicon.ico", ICO);
    assert_eq!(
        h.get(&format!("/api/projects/{default_id}/favicon"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        h.get(&format!("/api/projects/{second_id}/favicon"))
            .await
            .status,
        StatusCode::NO_CONTENT,
        "a nested project must not borrow its parent's icon"
    );

    // Its own icon is found once it has one.
    write(&h, "second/favicon.ico", PNG);
    let nested = h.get(&format!("/api/projects/{second_id}/favicon")).await;
    assert_eq!(nested.status, StatusCode::OK);
    assert_eq!(nested.body.as_ref(), PNG);
    // ...and the default project still answers with the root icon, not the
    // nested one: the root is the shallower match.
    let outer = h.get(&format!("/api/projects/{default_id}/favicon")).await;
    assert_eq!(outer.status, StatusCode::OK);
    assert_eq!(outer.body.as_ref(), ICO);
}

#[tokio::test]
async fn max_bytes_caps_what_the_rail_will_draw() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let id = default_project_id(&h).await;
    write(&h, "favicon.ico", ICO);

    let ok = h
        .get(&format!("/api/projects/{id}/favicon?max_bytes=1024"))
        .await;
    assert_eq!(ok.status, StatusCode::OK);

    // Half an icon does not render, so a file above the caller's cap answers
    // "nothing" rather than a truncated image.
    let too_small = h
        .get(&format!("/api/projects/{id}/favicon?max_bytes=4"))
        .await;
    assert_eq!(too_small.status, StatusCode::NO_CONTENT);

    let nonsense = h
        .get(&format!("/api/projects/{id}/favicon?max_bytes=abc"))
        .await;
    assert_eq!(nonsense.status, StatusCode::BAD_REQUEST);
    assert_eq!(nonsense.error_code(), "bad_request");
    assert!(nonsense.json()["error"]["message"]
        .as_str()
        .unwrap()
        .contains("max_bytes"));

    // A zero cap would make every project iconless by accident.
    let zero = h
        .get(&format!("/api/projects/{id}/favicon?max_bytes=0"))
        .await;
    assert_eq!(zero.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_icon_route_needs_the_bearer_token_when_one_is_configured() {
    let h = HarnessBuilder::new(vec![])
        .auth_token("s3cret-token-1234567890")
        .build()
        .await;
    let id = default_project_id_authorized(&h).await;
    write(&h, "favicon.ico", ICO);

    // `?token=` is accepted only for `/api/events`, so the icon is unreachable
    // without the header: this is exactly why the rail fetches the bytes through
    // the API client instead of an `<img src>`.
    let anonymous = h
        .get(&format!(
            "/api/projects/{id}/favicon?token=s3cret-token-1234567890"
        ))
        .await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    let authorized = h
        .get_with(
            &format!("/api/projects/{id}/favicon"),
            &[("Authorization", "Bearer s3cret-token-1234567890")],
        )
        .await;
    assert_eq!(authorized.status, StatusCode::OK);
    assert_eq!(authorized.body.as_ref(), ICO);
}

/// The project id with the bearer token attached (the plain helper would 401).
async fn default_project_id_authorized(h: &Harness) -> String {
    let listed = h
        .get_with(
            "/api/projects",
            &[("Authorization", "Bearer s3cret-token-1234567890")],
        )
        .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    listed.json()["projects"][0]["id"]
        .as_str()
        .expect("project id")
        .to_string()
}
