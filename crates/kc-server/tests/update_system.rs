//! End-to-end tests for the update system: the real HTTP routes, and a real
//! download from a real (localhost) HTTP server.
//!
//! The tests never install over the test binary - the apply path is exercised
//! against a temporary path in the unit tests beside `update.rs`. What is proved
//! here is the part that talks to the network: a manifest is fetched, an artifact
//! is streamed, hashed, executed and refused or staged for the right reason.

mod support;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use kc_server::update::{FixedSettings, Manifest, Updater, MAX_ARTIFACT_BYTES};
use kc_server::version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use support::HarnessBuilder;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "karucode-update-it-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A tiny release host: the manifest and the artifact, on a real socket.
struct ReleaseHost {
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ReleaseHost {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bind a port first, then let the caller build the manifest from that address -
/// a manifest that names the wrong port is a test that proves nothing.
async fn release_host<F>(make: F) -> ReleaseHost
where
    F: FnOnce(&str) -> (String, Vec<u8>),
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind release host");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://{addr}");
    let (manifest, artifact) = make(&base);
    let manifest = Arc::new(manifest);
    let artifact = Arc::new(artifact);
    let manifest_route = {
        let manifest = Arc::clone(&manifest);
        move || {
            let manifest = Arc::clone(&manifest);
            async move { manifest.as_str().to_string() }
        }
    };
    let artifact_route = {
        let artifact = Arc::clone(&artifact);
        move || {
            let artifact = Arc::clone(&artifact);
            async move { artifact.as_ref().clone() }
        }
    };
    let router = Router::new()
        .route("/updates.json", get(manifest_route))
        .route("/karucode", get(artifact_route));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    ReleaseHost { base, task }
}

fn manifest_value(base: &str, build: u64, artifact: &[u8]) -> Value {
    json!({
        "schema": 1,
        "build": build,
        "commit": "9c1de77a0b3c",
        "commit_date": "2026-05-31T09:12:40+00:00",
        "released_at": "2026-05-31T09:20:03Z",
        "url": "https://example.com/releases/tag/v1",
        "notes": "A newer build.",
        "targets": {
            version::TARGET: {
                "url": format!("{base}/karucode"),
                "sha256": sha256_hex(artifact),
                "size": artifact.len(),
                "tarball": format!("{base}/karucode.tar.gz")
            }
        }
    })
}

/// A fake build that answers `--version` like the real binary does.
fn fake_binary(build: u64) -> Vec<u8> {
    format!("#!/bin/sh\necho \"karucode {build} (abc123, 2026-05-31T09:12:40+00:00)\"\n")
        .into_bytes()
}

fn updates_config(manifest_url: String) -> kc_core::config::UpdatesConfig {
    kc_core::config::UpdatesConfig {
        check: true,
        check_secs: 3600,
        manifest_url,
        auto_apply: false,
    }
}

/// An updater pointed at a release host, with its own data directory.
fn updater_for(base: &str, data_dir: &std::path::Path) -> Arc<Updater> {
    Updater::new(
        Arc::new(FixedSettings(updates_config(format!(
            "{base}/updates.json"
        )))),
        data_dir.to_path_buf(),
    )
}

#[tokio::test]
async fn the_http_status_names_the_build_and_how_this_install_can_be_updated() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let response = h.get("/api/update").await;
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    assert_eq!(body["current"]["build"], version::BUILD);
    assert_eq!(body["current"]["version"], version::VERSION);
    assert_eq!(body["current"]["commit"], version::COMMIT);
    assert_eq!(
        body["install"]["kind"], "source",
        "tests run out of target/, which is a source tree: {body}"
    );
    assert_eq!(body["install"]["writable"], false);
    assert!(
        body["install"]["detail"]
            .as_str()
            .unwrap()
            .contains("packaging/update.sh"),
        "a source install must name the command that updates it: {body}"
    );
    assert_eq!(body["update_available"], false, "no check has run yet");
    assert_eq!(body["check"]["enabled"], true);
    assert!(body["check"]["manifest_url"]
        .as_str()
        .unwrap()
        .ends_with("updates.json"));
    assert_eq!(body["phase"], "idle");
    assert!(
        body["restart"]["apply_command"]
            .as_str()
            .unwrap()
            .contains("--data-dir"),
        "the privileged command has to carry the data directory: {body}"
    );
}

#[tokio::test]
async fn applying_from_a_source_tree_is_refused_with_the_command_that_works() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let response = h.post("/api/update/apply", json!({})).await;
    assert_eq!(
        response.status,
        StatusCode::CONFLICT,
        "a build tree is never replaced by a release artifact"
    );
    assert_eq!(response.error_code(), "bad_request");
    assert!(
        response
            .error_message()
            .contains("packaging/update.sh --no-pull"),
        "the refusal must name the working command: {}",
        response.error_message()
    );
}

#[tokio::test]
async fn the_updates_section_is_readable_and_patchable_through_config() {
    let h = HarnessBuilder::new(Vec::new()).build().await;
    let config = h.get("/api/config").await.json();
    assert_eq!(config["updates"]["check"], true);
    assert_eq!(config["updates"]["auto_apply"], false);
    assert_eq!(config["updates"]["check_secs"], 21_600);
    assert_eq!(
        config["updates"]["default_manifest_url"],
        kc_core::config::DEFAULT_UPDATE_MANIFEST_URL
    );

    let patched = h
        .patch(
            "/api/config",
            json!({"updates": {"check": false, "auto_apply": true, "check_secs": 60}}),
        )
        .await;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.text());
    let body = patched.json();
    assert_eq!(body["updates"]["check"], false);
    assert_eq!(body["updates"]["auto_apply"], true);
    assert_eq!(
        body["updates"]["check_secs"], 300,
        "60 is below the documented minimum, so the patch is clamped, not rejected"
    );
    // And it is persisted: a second read sees the same values.
    let again = h.get("/api/config").await.json();
    assert_eq!(again["updates"]["check"], false);
    assert_eq!(again["updates"]["auto_apply"], true);
}

#[tokio::test]
async fn checking_reads_a_real_manifest_and_reports_the_newer_build() {
    let host = release_host(|base| {
        let artifact = fake_binary(version::BUILD + 5);
        let manifest = manifest_value(base, version::BUILD + 5, &artifact).to_string();
        (manifest, artifact)
    })
    .await;

    let data_dir = temp_dir("check");
    let updater = updater_for(&host.base, &data_dir);
    let manifest = updater.check().await.expect("the manifest should load");
    assert_eq!(manifest.build, version::BUILD + 5);
    assert_eq!(manifest.targets.len(), 1);

    let status = updater.status();
    assert!(status.update_available);
    let latest = status.latest.expect("a check leaves the latest release");
    assert_eq!(latest.build, version::BUILD + 5);
    assert!(latest.has_target_for_this_build);
    assert_eq!(latest.notes.as_deref(), Some("A newer build."));
    assert!(status.check.checked_at.is_some());
    assert!(status.check.error.is_none(), "{:?}", status.check.error);

    // Drop the host: the cached state must survive on its own.
    drop(host);
    // A second updater over the same data directory reports the cached release
    // without touching the network (its URL points nowhere).
    let restarted = Updater::new(
        Arc::new(FixedSettings(updates_config(
            "http://127.0.0.1:1/updates.json".into(),
        ))),
        data_dir.clone(),
    );
    assert!(restarted.status().update_available);
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn staging_downloads_hashes_and_proves_the_artifact() {
    let host = release_host(|base| {
        let artifact = fake_binary(version::BUILD + 5);
        let manifest = manifest_value(base, version::BUILD + 5, &artifact).to_string();
        (manifest, artifact)
    })
    .await;
    let expected_sha = sha256_hex(&fake_binary(version::BUILD + 5));

    let data_dir = temp_dir("stage");
    let updater = updater_for(&host.base, &data_dir);
    // stage() is allowed with no prior check: it fetches the manifest itself.
    let staged = updater.stage().await.expect("the download should stage");
    assert_eq!(staged.build, version::BUILD + 5);
    assert_eq!(staged.sha256, expected_sha);
    assert_eq!(staged.size, fake_binary(version::BUILD + 5).len() as u64);
    assert!(
        std::path::Path::new(&staged.path).is_file(),
        "the staged binary should be on disk"
    );
    assert_eq!(
        updater.status().staged.as_ref().map(|s| s.build),
        Some(version::BUILD + 5),
        "the stage is reported so a later apply does not download again"
    );
    // It really runs and names the promised build.
    let output = std::process::Command::new(&staged.path)
        .arg("--version")
        .output()
        .expect("the staged artifact should execute");
    assert_eq!(
        version::parse_version_line(String::from_utf8_lossy(&output.stdout).trim()),
        Some(version::BUILD + 5)
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn an_artifact_that_lies_about_its_build_is_refused() {
    // The manifest says BUILD+5; the artifact's own `--version` says BUILD+9.
    let host = release_host(|base| {
        let artifact = fake_binary(version::BUILD + 9);
        let manifest = manifest_value(base, version::BUILD + 5, &artifact).to_string();
        (manifest, artifact)
    })
    .await;

    let data_dir = temp_dir("lying-artifact");
    let updater = updater_for(&host.base, &data_dir);
    let error = updater
        .stage()
        .await
        .expect_err("a mismatched build is refused");
    assert!(
        error.message.contains("reports build") && error.message.contains("promised"),
        "the message must name both builds: {}",
        error.message
    );
    assert!(
        updater.staged().is_none(),
        "a refused artifact must not be recorded as staged"
    );
    let staged_dir = data_dir.join("updates").join("staged");
    let leftovers: Vec<_> = std::fs::read_dir(&staged_dir)
        .map(|entries| entries.flatten().collect())
        .unwrap_or_default();
    assert!(
        leftovers
            .iter()
            .all(|entry| !entry.path().join("karucode").exists()),
        "a refused download must be deleted, not left for an apply"
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn a_checksum_mismatch_is_refused_before_anything_runs() {
    let host = release_host(|base| {
        let artifact = fake_binary(version::BUILD + 5);
        let mut manifest = manifest_value(base, version::BUILD + 5, &artifact);
        manifest["targets"][version::TARGET]["sha256"] = json!("0".repeat(64));
        (manifest.to_string(), artifact)
    })
    .await;

    let data_dir = temp_dir("bad-checksum");
    let updater = updater_for(&host.base, &data_dir);
    let error = updater
        .stage()
        .await
        .expect_err("a bad checksum is refused");
    assert!(
        error.message.contains("sha256"),
        "the refusal must name the check that failed: {}",
        error.message
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn a_release_without_this_platform_is_refused_with_the_targets_it_has() {
    let host = release_host(|base| {
        let artifact = fake_binary(version::BUILD + 5);
        let mut manifest = manifest_value(base, version::BUILD + 5, &artifact);
        let targets = manifest["targets"].as_object_mut().expect("targets");
        let entry = targets.remove(version::TARGET).expect("the current target");
        targets.insert("aarch64-unknown-linux-gnu".into(), entry);
        (manifest.to_string(), artifact)
    })
    .await;

    let data_dir = temp_dir("wrong-target");
    let updater = updater_for(&host.base, &data_dir);
    let error = updater
        .stage()
        .await
        .expect_err("no artifact for this platform");
    assert!(
        error.message.contains(version::TARGET)
            && error.message.contains("aarch64-unknown-linux-gnu"),
        "the refusal must name both platforms: {}",
        error.message
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn a_manifest_that_is_not_json_is_an_upstream_error_not_a_crash() {
    let host = release_host(|_base| ("<html>404</html>".to_string(), Vec::new())).await;
    let data_dir = temp_dir("bad-manifest");
    let updater = updater_for(&host.base, &data_dir);
    let error = updater.check().await.expect_err("html is not a manifest");
    assert!(
        error.message.contains("not valid JSON"),
        "{}",
        error.message
    );
    // The failure is visible in the status rather than only in a log.
    let status = updater.status();
    assert!(status.check.error.is_some());
    assert!(status.check.checked_at.is_some());
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn checking_is_refused_when_it_is_turned_off() {
    let data_dir = temp_dir("check-off");
    let updater = Updater::new(
        Arc::new(FixedSettings(updates_config(String::new()))),
        data_dir.clone(),
    );
    let error = updater.check().await.expect_err("checking is off");
    assert!(
        error.message.contains("update checking is off"),
        "the refusal must say checking is off: {}",
        error.message
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn a_manifest_that_promises_a_too_large_artifact_is_refused_unread() {
    let host = release_host(|base| {
        let artifact = fake_binary(version::BUILD + 5);
        let mut manifest = manifest_value(base, version::BUILD + 5, &artifact);
        manifest["targets"][version::TARGET]["size"] = json!(MAX_ARTIFACT_BYTES + 1);
        (manifest.to_string(), artifact)
    })
    .await;

    let data_dir = temp_dir("too-large");
    let updater = updater_for(&host.base, &data_dir);
    let error = updater.check().await.expect_err("the manifest is refused");
    assert!(
        error.message.contains("download limit"),
        "{}",
        error.message
    );
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[test]
fn the_manifest_shape_the_release_script_writes_parses() {
    // The exact document `scripts/release_manifest.py` emits, so the producer and
    // the consumer cannot drift without this test failing.
    let text = r#"{
      "schema": 1,
      "build": 108,
      "commit": "9c1de77a0b3c",
      "commit_date": "2026-05-31T09:12:40+00:00",
      "released_at": "2026-05-31T09:20:03Z",
      "url": "https://github.com/karutoil/karucode/releases/tag/v108",
      "notes": "Faster startup.",
      "targets": {
        "x86_64-unknown-linux-gnu": {
          "url": "https://github.com/karutoil/karucode/releases/download/v108/karucode-108-x86_64-unknown-linux-gnu",
          "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "size": 18432120,
          "tarball": "https://github.com/karutoil/karucode/releases/download/v108/karucode-108-x86_64-unknown-linux-gnu.tar.gz"
        }
      }
    }"#;
    let manifest = Manifest::parse(text).expect("the release script's shape must parse");
    assert_eq!(manifest.build, 108);
    assert_eq!(manifest.schema, 1);
}
