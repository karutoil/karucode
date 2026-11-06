//! End-to-end coverage for the server-backed, per-project Workbench preference.

mod support;

use axum::http::StatusCode;
use serde_json::json;
use support::HarnessBuilder;

async fn default_project(h: &support::Harness) -> serde_json::Value {
    let response = h.get("/api/projects").await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.text());
    response.json()["projects"][0].clone()
}

#[tokio::test]
async fn result_panel_width_round_trips_through_project_preferences_and_listing() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let project = default_project(&h).await;
    let id = project["id"].as_str().expect("project id");
    assert!(
        project.get("result_panel_width").is_none(),
        "defaults stay compact"
    );

    let saved = h
        .patch(
            &format!("/api/projects/{id}/preferences"),
            json!({ "result_panel_width": 640 }),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());
    assert_eq!(saved.json()["project"]["result_panel_width"], 640);
    assert_eq!(default_project(&h).await["result_panel_width"], 640);

    // Renaming shares the project record's PATCH route but must not discard the
    // unrelated preference when it reconstructs the response.
    let renamed = h
        .patch(&format!("/api/projects/{id}"), json!({ "name": "Renamed" }))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    assert_eq!(renamed.json()["project"]["name"], "Renamed");
    assert_eq!(renamed.json()["project"]["result_panel_width"], 640);

    // Both inclusive server bounds are valid; the UI applies tighter geometry
    // constraints based on the actual space beside the conversation.
    for width in [280, 1600] {
        let response = h
            .patch(
                &format!("/api/projects/{id}/preferences"),
                json!({ "result_panel_width": width }),
            )
            .await;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "{}: {width}",
            response.text()
        );
        assert_eq!(response.json()["project"]["result_panel_width"], width);
    }
}

#[tokio::test]
async fn invalid_or_unavailable_project_preferences_fail_without_changing_the_saved_width() {
    let h = HarnessBuilder::new(vec![]).build().await;
    let project = default_project(&h).await;
    let id = project["id"].as_str().expect("project id");
    let saved = h
        .patch(
            &format!("/api/projects/{id}/preferences"),
            json!({ "result_panel_width": 520 }),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.text());

    for body in [
        json!({ "result_panel_width": 279 }),
        json!({ "result_panel_width": 1601 }),
        json!({ "result_panel_width": 480.5 }),
        json!({ "result_panel_width": "480" }),
        json!({ "result_panel_width": null }),
        json!({}),
        json!({ "result_panel_width": 480, "unknown": true }),
    ] {
        let response = h
            .patch(&format!("/api/projects/{id}/preferences"), body.clone())
            .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(response.error_code(), "bad_request", "{body}");
    }
    assert_eq!(default_project(&h).await["result_panel_width"], 520);

    let missing = h
        .patch(
            "/api/projects/p_missing/preferences",
            json!({ "result_panel_width": 480 }),
        )
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.error_code(), "not_found");
}
