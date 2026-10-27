//! Unit tests for discovery, manifests and the hooks/permission policy.
//!
//! End-to-end tests that spawn a real (fixture) plugin process live in
//! `tests/host.rs`.

use super::*;
use kc_core::plugin::PluginManifest;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-plugin-host-unit-{tag}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cfg(name: &str) -> PluginConfig {
    PluginConfig {
        name: name.into(),
        enabled: true,
        command: vec!["sh".into(), "-c".into(), "exit 0".into()],
        env: BTreeMap::new(),
        permissions: Vec::new(),
        settings: BTreeMap::new(),
    }
}

#[test]
fn explicit_configs_are_discovered_without_spawning() {
    let dir = temp_dir("explicit");
    let host = PluginHost::discover(&[cfg("alpha"), cfg("beta")], &dir);
    assert_eq!(host.plugin_count(), 2);
    let diags = host.diagnostics();
    assert_eq!(diags[0].name, "alpha");
    // Discovered but not started yet.
    assert_eq!(diags[0].status, PluginStatus::Stopped);
    assert!(host.tools().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_command_is_a_clear_failure_not_a_panic() {
    let dir = temp_dir("nocmd");
    let mut broken = cfg("broken");
    broken.command.clear();
    let host = PluginHost::discover(&[broken], &dir);
    let diags = host.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].status, PluginStatus::Failed);
    let err = diags[0].error.clone().unwrap();
    assert!(err.contains("no command"), "{err}");
    assert!(err.contains("broken"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn duplicate_names_fail_the_slot() {
    let dir = temp_dir("dup");
    let host = PluginHost::discover(&[cfg("same"), cfg("same")], &dir);
    // The duplicate is recorded once, as a failure, so names stay unique.
    assert_eq!(host.plugin_count(), 1);
    let diags = host.diagnostics();
    assert_eq!(diags[0].status, PluginStatus::Failed);
    assert!(diags[0]
        .error
        .as_deref()
        .unwrap()
        .contains("duplicate plugin name `same`"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn disk_manifests_are_merged_with_config_entries() {
    let dir = temp_dir("disk");
    let plugin_dir = dir.join("plugins/from-disk");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
name = "from-disk"
command = ["sh", "-c", "exit 0"]
enabled = true
permissions = ["files.read", "network"]

[env]
FOO = "bar"

[settings]
greeting = "hi"
"#,
    )
    .unwrap();
    let host = PluginHost::discover(&[cfg("from-config")], &dir);
    let names: Vec<String> = host.diagnostics().into_iter().map(|d| d.name).collect();
    assert_eq!(
        names,
        vec!["from-config".to_string(), "from-disk".to_string()]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn invalid_manifest_is_reported_with_the_path() {
    let dir = temp_dir("badmanifest");
    let plugin_dir = dir.join("plugins/broken");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "name = \"broken\"\ncommand = 3\n",
    )
    .unwrap();
    let host = PluginHost::discover(&[], &dir);
    let diags = host.diagnostics();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].name, "broken");
    assert_eq!(diags[0].status, PluginStatus::Failed);
    let err = diags[0].error.clone().unwrap();
    assert!(err.contains("plugin.toml"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn manifest_without_name_falls_back_to_the_directory() {
    let dir = temp_dir("noname");
    let plugin_dir = dir.join("plugins/dirnamed");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "command = [\"sh\", \"-c\", \"exit 0\"]\n",
    )
    .unwrap();
    let host = PluginHost::discover(&[], &dir);
    let diags = host.diagnostics();
    assert_eq!(diags[0].name, "dirnamed");
    assert_eq!(diags[0].status, PluginStatus::Stopped);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn disabled_plugins_are_disabled_and_not_started() {
    let dir = temp_dir("disabled");
    let mut c = cfg("off");
    c.enabled = false;
    let host = PluginHost::discover(&[c], &dir);
    assert_eq!(host.diagnostics()[0].status, PluginStatus::Disabled);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn effective_permissions_are_config_authoritative() {
    let manifest = PluginManifest {
        name: "x".into(),
        version: "1".into(),
        permissions: vec!["files.read".into(), "shell".into(), "shell".into()],
        ..Default::default()
    };
    let mut c = cfg("x");

    // No configured permissions: the manifest is the fallback.
    let (granted, ignored) = effective_permissions(&c, Some(&manifest));
    assert_eq!(granted, vec!["files.read".to_string(), "shell".to_string()]);
    assert!(ignored.is_empty());

    // Configured permissions win; manifest extras are reported as ignored.
    c.permissions = vec!["files.read".into()];
    let (granted, ignored) = effective_permissions(&c, Some(&manifest));
    assert_eq!(granted, vec!["files.read".to_string()]);
    assert_eq!(ignored, vec!["shell".to_string()]);

    c.permissions = vec!["shell".into(), "shell".into()];
    let (granted, ignored) = effective_permissions(&c, Some(&manifest));
    assert_eq!(granted, vec!["shell".to_string()]);
    assert_eq!(ignored, vec!["files.read".to_string()]);

    // The config may grant more than the manifest declared, and may do so
    // before any manifest has been received.
    c.permissions = vec!["network".into()];
    assert_eq!(
        effective_permissions(&c, None),
        (vec!["network".to_string()], Vec::new())
    );
    assert_eq!(
        effective_permissions(&c, Some(&manifest)),
        (
            vec!["network".to_string()],
            vec!["files.read".to_string(), "shell".to_string()]
        )
    );
}

#[test]
fn value_to_text_handles_the_documented_shapes() {
    assert_eq!(value_to_text(&serde_json::json!("hi")).unwrap(), "hi");
    assert_eq!(
        value_to_text(&serde_json::json!({"text": "hi"})).unwrap(),
        "hi"
    );
    assert!(value_to_text(&serde_json::json!({"other": 1}))
        .unwrap()
        .contains("other"));
    assert!(value_to_text(&Value::Null).is_none());
}

#[test]
fn hooks_module_lists_implemented_hooks() {
    assert!(hooks::IMPLEMENTED.contains(&hooks::BEFORE_PROMPT));
    assert!(hooks::IMPLEMENTED.contains(&hooks::ON_RUN_END));
    assert!(hooks::IMPLEMENTED.contains(&hooks::ON_TOOL_RESULT));
}

#[test]
fn default_timeouts_match_the_documented_budgets() {
    let t = PluginTimeouts::default();
    assert_eq!(t.handshake, Duration::from_secs(10));
    assert_eq!(t.call, Duration::from_secs(60));
    assert_eq!(t.hook, Duration::from_secs(5));
}

#[test]
fn manifest_protocol_defaults_are_accepted() {
    // The host validates the version reported by `initialize`.
    let m: PluginManifest = serde_json::from_str(r#"{"name":"x","version":"1"}"#).unwrap();
    assert_eq!(m.protocol, kc_core::plugin::PLUGIN_PROTOCOL_VERSION);
}
