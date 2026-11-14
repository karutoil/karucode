//! `/api/skills`: the list, the body, and the three mutations.
//!
//! The store is the settings table and the source of the list is a filesystem
//! scan, so these tests drive both halves: a real `SKILL.md` under the harness
//! workspace for the discovered half, and `POST /api/skills` for the half that
//! lives in the config.
//!
//! They never assert on the *whole* list: `auto_detect` also scans the user's
//! home directory, so a machine with `~/.claude/skills` populated would make an
//! exact-length assertion a lie. Every assertion is about the skill under test.

mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use support::{Harness, HarnessBuilder};

/// Write `<workspace>/.karucode/skills/<name>/SKILL.md` and return its path.
fn write_skill(h: &Harness, name: &str, contents: &str) -> std::path::PathBuf {
    let path = h
        .workspace_path()
        .join(".karucode/skills")
        .join(name)
        .join("SKILL.md");
    std::fs::create_dir_all(path.parent().expect("skill dir")).expect("skill dir");
    std::fs::write(&path, contents).expect("skill file");
    path
}

fn skill_by_id<'a>(payload: &'a Value, id: &str) -> Option<&'a Value> {
    payload["skills"]
        .as_array()?
        .iter()
        .find(|skill| skill["id"] == json!(id))
}

async fn listed(h: &Harness, id: &str) -> Option<Value> {
    let response = h.get("/api/skills").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let payload = response.json();
    skill_by_id(&payload, id).cloned()
}

const RELEASE: &str = "---\n\
name: release\n\
description: Cut a release: bump versions, run the gates, tag, push.\n\
---\n\
\n\
# Cutting a release\n\
\n\
1. `cargo test --workspace`\n\
";

#[tokio::test]
async fn a_workspace_skill_is_listed_with_its_file_and_source() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let path = write_skill(&h, "release", RELEASE);

    let response = h.get("/api/skills").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    let payload = response.json();

    // The switches are reported next to the list, so the UI can show what a
    // discovery run actually used.
    assert_eq!(payload["enabled"], json!(true));
    assert_eq!(payload["auto_detect"], json!(true));
    assert_eq!(payload["prompt"], json!(true));
    assert!(payload["prompt_max_chars"].as_u64().unwrap() >= 200);
    assert!(payload["max_skills"].as_u64().unwrap() >= 1);
    assert_eq!(payload["truncated"], json!(false));

    let skill = skill_by_id(&payload, "release").expect("release is listed");
    assert_eq!(skill["name"], json!("release"));
    assert_eq!(
        skill["description"],
        json!("Cut a release: bump versions, run the gates, tag, push.")
    );
    assert_eq!(skill["source"], json!("workspace"));
    assert_eq!(skill["enabled"], json!(true));
    assert_eq!(skill["custom"], json!(false));
    assert_eq!(
        skill["path"].as_str().expect("path"),
        path.to_string_lossy()
    );

    // The root that yielded it is reported as existing, with a count, so
    // "auto-detect found nothing" is explainable rather than a mystery.
    let workspace_root = payload["roots"]
        .as_array()
        .expect("roots")
        .iter()
        .find(|root| {
            root["path"]
                == json!(h
                    .workspace_path()
                    .join(".karucode/skills")
                    .to_string_lossy())
        })
        .expect("workspace root is listed");
    assert_eq!(workspace_root["source"], json!("workspace"));
    assert_eq!(workspace_root["exists"], json!(true));
    assert_eq!(workspace_root["configured"], json!(false));
    assert!(workspace_root["count"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn a_posted_skill_is_stored_and_listed_as_custom() {
    let h = HarnessBuilder::new(vec![]).build().await;

    let created = h
        .post(
            "/api/skills",
            json!({
                "name": "Cut a Release",
                "description": "Bump, gate, tag, push.",
                "body": "# Cutting a release\n\n1. run the gates\n",
            }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let created = created.json();
    // The id is the slug of the name, so a display name is enough to add one.
    assert_eq!(created["id"], json!("cut-a-release"));
    assert_eq!(created["name"], json!("Cut a Release"));
    assert_eq!(created["source"], json!("custom"));
    assert_eq!(created["custom"], json!(true));
    assert_eq!(created["enabled"], json!(true));
    assert_eq!(created["path"], Value::Null);

    let skill = listed(&h, "cut-a-release").await.expect("listed");
    assert_eq!(skill["description"], json!("Bump, gate, tag, push."));

    // ...and the body comes back from the settings table, not from a file.
    let detail = h.get("/api/skills/cut-a-release").await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    let detail = detail.json();
    assert_eq!(
        detail["body"],
        json!("# Cutting a release\n\n1. run the gates")
    );
    assert_eq!(detail["body_truncated"], json!(false));

    // The same id may not be taken twice, whatever the source.
    let duplicate = h
        .post(
            "/api/skills",
            json!({ "id": "cut-a-release", "body": "other text" }),
        )
        .await;
    assert_eq!(
        duplicate.status,
        StatusCode::CONFLICT,
        "{}",
        duplicate.text()
    );

    // A body is required: a skill with no text is not a skill.
    let empty = h
        .post("/api/skills", json!({ "id": "blank", "body": "   " }))
        .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST, "{}", empty.text());
    assert_eq!(empty.error_code(), "bad_request");
    assert!(empty.error_message().contains("body"));
}

#[tokio::test]
async fn a_posted_skill_without_a_description_uses_its_first_line() {
    let h = HarnessBuilder::new(vec![]).build().await;

    let created = h
        .post(
            "/api/skills",
            json!({
                "id": "migrate",
                "body": "# Migrations\n\nRun them in filename order, never alphabetically.\n",
            }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    // A skill with no description is invisible in the prompt, so the first
    // real line of the body is used instead - the same rule files follow.
    assert_eq!(
        created.json()["description"],
        json!("Run them in filename order, never alphabetically.")
    );
    assert_eq!(created.json()["name"], json!("migrate"));
}

#[tokio::test]
async fn disabling_a_skill_keeps_it_listed_but_not_enabled() {
    let h = HarnessBuilder::new(vec![]).build().await;
    write_skill(&h, "release", RELEASE);

    let patched = h
        .patch("/api/skills/release", json!({ "enabled": false }))
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let patched = patched.json();
    assert_eq!(patched["id"], json!("release"));
    assert_eq!(patched["enabled"], json!(false));

    // Still discovered, still listed - just off. That is what lets the UI show
    // it and offer to turn it back on.
    let skill = listed(&h, "release").await.expect("still listed");
    assert_eq!(skill["enabled"], json!(false));

    let back_on = h
        .patch("/api/skills/release", json!({ "enabled": true }))
        .await;
    assert_eq!(back_on.status, StatusCode::OK, "{}", back_on.text());
    assert_eq!(back_on.json()["enabled"], json!(true));
    assert_eq!(
        listed(&h, "release").await.expect("listed")["enabled"],
        json!(true)
    );
}

#[tokio::test]
async fn a_custom_skill_can_be_edited_in_place() {
    let h = HarnessBuilder::new(vec![]).build().await;

    let created = h
        .post(
            "/api/skills",
            json!({ "id": "house-style", "description": "Old.", "body": "old body" }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    let patched = h
        .patch(
            "/api/skills/house-style",
            json!({
                "name": "House style",
                "description": "How we name things.",
                "body": "new body",
            }),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let patched = patched.json();
    assert_eq!(patched["name"], json!("House style"));
    assert_eq!(patched["description"], json!("How we name things."));

    let detail = h.get("/api/skills/house-style").await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    assert_eq!(detail.json()["body"], json!("new body"));
}

#[tokio::test]
async fn a_file_backed_skill_cannot_be_rewritten_or_deleted() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let path = write_skill(&h, "release", RELEASE);

    let patched = h
        .patch("/api/skills/release", json!({ "body": "replaced" }))
        .await;
    assert_eq!(
        patched.status,
        StatusCode::BAD_REQUEST,
        "{}",
        patched.text()
    );
    assert_eq!(patched.error_code(), "bad_request");
    let message = patched.error_message();
    assert!(
        message.contains(&path.to_string_lossy().to_string()),
        "the refusal must name the file: {message}"
    );
    assert!(
        message.contains("off"),
        "the refusal must offer turning it off: {message}"
    );
    // The file is untouched.
    assert_eq!(std::fs::read_to_string(&path).expect("file"), RELEASE);

    let deleted = h.delete("/api/skills/release").await;
    assert_eq!(
        deleted.status,
        StatusCode::BAD_REQUEST,
        "{}",
        deleted.text()
    );
    let message = deleted.error_message();
    assert!(
        message.contains(&path.to_string_lossy().to_string()),
        "the refusal must name the file: {message}"
    );
    assert!(
        message.contains("PATCH /api/skills/release"),
        "the refusal must point at the route that does work: {message}"
    );
    // ...and the skill is still on disk and still listed.
    assert!(listed(&h, "release").await.is_some());
}

#[tokio::test]
async fn deleting_a_custom_skill_removes_it() {
    let h = HarnessBuilder::new(vec![]).build().await;

    let created = h
        .post(
            "/api/skills",
            json!({ "id": "throwaway", "body": "temporary" }),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert!(listed(&h, "throwaway").await.is_some());

    let deleted = h.delete("/api/skills/throwaway").await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.text());
    assert!(listed(&h, "throwaway").await.is_none());

    // Deleting it again is a 404, not a second success.
    let again = h.delete("/api/skills/throwaway").await;
    assert_eq!(again.status, StatusCode::NOT_FOUND, "{}", again.text());
}

#[tokio::test]
async fn the_body_is_returned_with_the_frontmatter_stripped() {
    let h = HarnessBuilder::new(vec![]).build().await;
    write_skill(&h, "release", RELEASE);

    let detail = h.get("/api/skills/release").await;
    assert_eq!(detail.status, StatusCode::OK, "{}", detail.text());
    let detail = detail.json();
    let body = detail["body"].as_str().expect("body");
    assert!(body.starts_with("\n# Cutting a release"), "{body:?}");
    assert!(!body.contains("description:"), "{body:?}");
    assert!(body.contains("cargo test --workspace"), "{body:?}");
    assert_eq!(detail["body_truncated"], json!(false));

    // The URL id is slugged the same way the ids are, so a looser spelling
    // reaches the same skill.
    let loose = h.get("/api/skills/Release").await;
    assert_eq!(loose.status, StatusCode::OK, "{}", loose.text());
}

#[tokio::test]
async fn an_unknown_skill_is_a_404_with_the_documented_code() {
    let h = HarnessBuilder::new(vec![]).build().await;

    let detail = h.get("/api/skills/nope").await;
    assert_eq!(detail.status, StatusCode::NOT_FOUND, "{}", detail.text());
    assert_eq!(detail.error_code(), "not_found");
    assert!(detail.error_message().contains("nope"));

    for response in [
        h.patch("/api/skills/nope", json!({ "enabled": false }))
            .await,
        h.delete("/api/skills/nope").await,
    ] {
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "{}",
            response.text()
        );
        assert_eq!(response.error_code(), "not_found");
    }
}

#[tokio::test]
async fn the_skills_settings_round_trip_through_the_config_routes() {
    let h = HarnessBuilder::new(vec![]).build().await;

    // A custom skill is stored where a config patch cannot reach - it must
    // survive a settings save untouched.
    let created = h
        .post("/api/skills", json!({ "id": "keep-me", "body": "text" }))
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let disabled = h
        .patch("/api/skills/keep-me", json!({ "enabled": false }))
        .await;
    assert_eq!(disabled.status, StatusCode::OK, "{}", disabled.text());

    let patched = h
        .patch(
            "/api/config",
            json!({
                "skills": {
                    "enabled": false,
                    "auto_detect": false,
                    "prompt": false,
                    "prompt_max_chars": 900,
                    "max_skills": 7,
                    "directories": ["/tmp/kc-skills-elsewhere", "relative/skills"],
                }
            }),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let skills = patched.json()["skills"].clone();
    assert_eq!(skills["enabled"], json!(false));
    assert_eq!(skills["auto_detect"], json!(false));
    assert_eq!(skills["prompt"], json!(false));
    assert_eq!(skills["prompt_max_chars"], json!(900));
    assert_eq!(skills["max_skills"], json!(7));
    assert_eq!(
        skills["directories"],
        json!(["/tmp/kc-skills-elsewhere", "relative/skills"])
    );
    // The per-skill rows are not part of the config document.
    assert!(skills.get("custom").is_none());
    assert!(skills.get("disabled").is_none());

    let config = h.get("/api/config").await;
    assert_eq!(config.status, StatusCode::OK, "{}", config.text());
    assert_eq!(config.json()["skills"], skills);

    // A partial patch leaves the rest alone.
    let partial = h
        .patch("/api/config", json!({ "skills": { "max_skills": 12 } }))
        .await;
    assert_eq!(partial.status, StatusCode::OK, "{}", partial.text());
    let partial = partial.json()["skills"].clone();
    assert_eq!(partial["max_skills"], json!(12));
    assert_eq!(partial["prompt_max_chars"], json!(900));

    // The stored skill and its off switch survived both patches.
    let listed = h.get("/api/skills").await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text());
    let skill = skill_by_id(&listed.json(), "keep-me").cloned();
    let skill = skill.expect("a config patch must not delete a stored skill");
    assert_eq!(skill["enabled"], json!(false));
}

#[tokio::test]
async fn a_locked_deployment_refuses_every_skill_mutation() {
    let h = HarnessBuilder::new(vec![]).lock_settings().build().await;
    write_skill(&h, "release", RELEASE);

    for response in [
        h.post("/api/skills", json!({ "id": "x", "body": "y" }))
            .await,
        h.patch("/api/skills/release", json!({ "enabled": false }))
            .await,
        h.delete("/api/skills/release").await,
    ] {
        assert_eq!(
            response.status,
            StatusCode::FORBIDDEN,
            "{}",
            response.text()
        );
        assert_eq!(response.error_code(), "permission_denied");
        assert!(
            response.error_message().contains("KARUCODE_LOCK_SETTINGS"),
            "{}",
            response.error_message()
        );
    }

    // Reading is not a mutation.
    assert_eq!(h.get("/api/skills").await.status, StatusCode::OK);
}

#[tokio::test]
async fn skill_mutations_need_the_admin_credential_when_one_is_configured() {
    let h = HarnessBuilder::new(vec![])
        .admin_token("s3cret")
        .build()
        .await;
    write_skill(&h, "release", RELEASE);

    let refused = h
        .post("/api/skills", json!({ "id": "note", "body": "text" }))
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
    assert_eq!(refused.error_code(), "permission_denied");
    assert!(
        refused.error_message().contains("X-Karucode-Admin-Token"),
        "{}",
        refused.error_message()
    );

    let wrong = h
        .send(
            axum::http::Method::POST,
            "/api/skills",
            Some(json!({ "id": "note", "body": "text" })),
            &[("x-karucode-admin-token", "not-it")],
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN, "{}", wrong.text());

    let created = h
        .send(
            axum::http::Method::POST,
            "/api/skills",
            Some(json!({ "id": "note", "body": "text" })),
            &[("x-karucode-admin-token", "s3cret")],
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());

    // The gate is on the mutations, not on the reads.
    assert_eq!(h.get("/api/skills").await.status, StatusCode::OK);
    assert_eq!(h.get("/api/skills/release").await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_configured_directory_reports_its_own_source_spelling() {
    let h = HarnessBuilder::new(vec![]).build().await;
    // A folder outside the workspace, added through the settings screen.
    let extra = h.root.join("extra-skills");
    std::fs::create_dir_all(extra.join("deploy")).expect("extra dir");
    std::fs::write(
        extra.join("deploy/SKILL.md"),
        "---\nname: deploy\ndescription: Ship it.\n---\n\n# Deploy\n",
    )
    .expect("skill file");

    let patched = h
        .patch(
            "/api/config",
            json!({ "skills": { "directories": [extra.to_string_lossy()] } }),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());

    let payload = h.get("/api/skills").await.json();
    let skill = skill_by_id(&payload, "deploy").expect("the configured skill is listed");
    // `SkillSource`'s own serde spelling, not `label()`'s "configured" and not
    // "added": the API and `docs/API.md` agree on these four words.
    assert_eq!(skill["source"], json!("config"));
    assert_eq!(skill["custom"], json!(false));

    let root = payload["roots"]
        .as_array()
        .expect("roots")
        .iter()
        .find(|root| root["configured"] == json!(true))
        .expect("the configured root is listed");
    assert_eq!(root["path"], json!(extra.to_string_lossy()));
    assert_eq!(root["source"], json!("config"));
    assert_eq!(root["exists"], json!(true));
    assert_eq!(root["count"], json!(1));

    // A file in the workspace keeps its own source: the fix that made
    // `read_skill` take the root's source rather than assuming `Workspace`.
    write_skill(&h, "release", RELEASE);
    let payload = h.get("/api/skills").await.json();
    assert_eq!(
        skill_by_id(&payload, "release").expect("listed")["source"],
        json!("workspace")
    );
}

#[tokio::test]
async fn an_unknown_project_is_refused_before_anything_is_scanned() {
    let h = HarnessBuilder::new(vec![]).build().await;

    let response = h.get("/api/skills?project=does-not-exist").await;
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "{}",
        response.text()
    );
    assert_eq!(response.error_code(), "not_found");
}
