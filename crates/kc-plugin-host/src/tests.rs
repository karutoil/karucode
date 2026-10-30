//! Unit tests for discovery, manifests and the hooks/permission policy.
//!
//! End-to-end tests that spawn a real (fixture) plugin process live in
//! `tests/host.rs`.

use super::*;
use kc_core::plugin::{PluginManifest, PluginPanel, PluginToolDef, PANEL_SOURCE_STATIC};

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

    // Fail-closed: no configured permissions grants nothing, whatever the
    // manifest asks for. This is the `[[plugins]] name = … command = …` entry.
    let (granted, ignored) = effective_permissions(&c, Some(&manifest));
    assert!(granted.is_empty(), "{granted:?}");
    assert_eq!(ignored, vec!["files.read".to_string(), "shell".to_string()]);

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
fn an_explicit_wildcard_or_trust_opt_in_honours_the_manifest() {
    let manifest = PluginManifest {
        name: "x".into(),
        version: "1".into(),
        permissions: vec!["files.read".into(), "shell".into()],
        ..Default::default()
    };

    // `permissions = ["*"]` is the documented escape hatch.
    let mut c = cfg("x");
    c.permissions = vec!["*".into()];
    let (granted, ignored) = effective_permissions(&c, Some(&manifest));
    assert_eq!(granted, vec!["files.read".to_string(), "shell".to_string()]);
    assert!(ignored.is_empty());
    // The wildcard itself is never reported as a granted permission.
    assert!(!granted.iter().any(|p| p == "*"));

    // `settings.trust = true` is the equivalent.
    let mut c = cfg("x");
    c.settings.insert("trust".into(), serde_json::json!(true));
    let (granted, ignored) = effective_permissions(&c, Some(&manifest));
    assert_eq!(granted, vec!["files.read".to_string(), "shell".to_string()]);
    assert!(ignored.is_empty());

    // `trust = false` is not an opt-in.
    let mut c = cfg("x");
    c.settings.insert("trust".into(), serde_json::json!(false));
    let (granted, _) = effective_permissions(&c, Some(&manifest));
    assert!(granted.is_empty(), "{granted:?}");
}

#[test]
fn hook_output_is_capped_per_contribution_and_in_total() {
    // Under the cap: untouched, no marker.
    let (text, cut) = cap_text("short", MAX_HOOK_NOTE_BYTES);
    assert_eq!(text, "short");
    assert!(!cut);

    // Over the per-contribution cap: cut on a char boundary with a marker.
    let big = "é".repeat(MAX_HOOK_NOTE_BYTES);
    let (text, cut) = cap_text(&big, MAX_HOOK_NOTE_BYTES);
    assert!(cut);
    assert!(
        text.ends_with(TRUNCATION_MARKER),
        "{}",
        &text[text.len() - 40..]
    );
    assert!(text.len() <= MAX_HOOK_NOTE_BYTES + TRUNCATION_MARKER.len());
    // The cut never splits a multi-byte character.
    assert!(text.is_char_boundary(text.len() - TRUNCATION_MARKER.len()));

    // A cap smaller than one character still produces valid UTF-8.
    let (text, cut) = cap_text("é", 1);
    assert!(cut);
    assert_eq!(text, TRUNCATION_MARKER);
}

#[test]
fn the_hook_budget_is_smaller_than_a_context_window() {
    assert_eq!(MAX_HOOK_NOTE_BYTES, 4 * 1024);
    assert_eq!(MAX_HOOK_NOTES_BYTES, 16 * 1024);
    const { assert!(MAX_HOOK_NOTE_BYTES <= MAX_HOOK_NOTES_BYTES) };
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

fn panel(id: &str, title: &str) -> PluginPanel {
    PluginPanel {
        id: id.into(),
        title: title.into(),
        ..Default::default()
    }
}

fn tool_def(name: &str) -> PluginToolDef {
    PluginToolDef {
        name: name.into(),
        description: String::new(),
        parameters: serde_json::json!({}),
        class: None,
    }
}

#[test]
fn a_manifest_without_panels_yields_none_and_no_error() {
    let manifest = PluginManifest {
        name: "x".into(),
        version: "1".into(),
        ..Default::default()
    };
    let panels = rpc::parse_panels("x", &manifest, &[]).unwrap();
    assert!(panels.is_empty());
}

#[test]
fn panels_are_namespaced_and_titled_for_the_ui() {
    let manifest = PluginManifest {
        name: "kc-todo".into(),
        version: "1".into(),
        panels: vec![
            PluginPanel {
                id: "todo".into(),
                title: "Todo".into(),
                icon: Some("check".into()),
                source: PANEL_SOURCE_STATIC.into(),
                tool: None,
                refresh_ms: 5_000,
            },
            // No title: the id is the label so the tab is never blank.
            panel("board", ""),
        ],
        ..Default::default()
    };
    let panels = rpc::parse_panels("kc-todo", &manifest, &[]).unwrap();
    assert_eq!(panels.len(), 2);
    assert_eq!(panels[0].title, "Todo");
    assert_eq!(panels[1].title, "board");

    let summary = PanelSummary::new("kc-todo", panels[0].clone());
    assert_eq!(summary.hosted_id, "plugin__kc-todo__todo");
    assert_eq!(summary.id, "todo");
    assert_eq!(summary.icon.as_deref(), Some("check"));
    assert_eq!(summary.refresh_ms, 5_000);
}

#[test]
fn a_panel_naming_an_unknown_tool_is_a_discovery_error() {
    let manifest = PluginManifest {
        name: "x".into(),
        panels: vec![PluginPanel {
            id: "todo".into(),
            title: "Todo".into(),
            tool: Some("todo_list".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let err = rpc::parse_panels("x", &manifest, &[]).unwrap_err();
    assert!(err.contains("todo_list"), "{err}");
    assert!(err.contains("tools/list"), "{err}");

    // The same declaration is fine once the tool exists.
    let panels = rpc::parse_panels("x", &manifest, &[tool_def("todo_list")]).unwrap();
    assert_eq!(panels.len(), 1);
}

#[test]
fn bad_panel_declarations_are_refused_with_the_reason() {
    let with = |panels: Vec<PluginPanel>| PluginManifest {
        name: "x".into(),
        panels,
        ..Default::default()
    };
    let tools = [tool_def("todo_list")];

    // A collision after namespacing.
    let err = rpc::parse_panels(
        "x",
        &with(vec![panel("todo", "Todo"), panel("todo", "Other")]),
        &tools,
    )
    .unwrap_err();
    assert!(err.contains("twice"), "{err}");

    // An id that cannot travel in a URL or a DOM id.
    for bad in ["", "To do", "a/b"] {
        let err = rpc::parse_panels("x", &with(vec![panel(bad, "T")]), &tools).unwrap_err();
        assert!(err.contains("panel id"), "{err}");
    }

    // An unknown source.
    let mut wrong = panel("todo", "Todo");
    wrong.source = "scripts".into();
    let err = rpc::parse_panels("x", &with(vec![wrong]), &tools).unwrap_err();
    assert!(err.contains("scripts"), "{err}");
    assert!(err.contains("static"), "{err}");

    // `static` panels need no tool at all.
    let mut stat = panel("todo", "Todo");
    stat.source = PANEL_SOURCE_STATIC.into();
    assert_eq!(
        rpc::parse_panels("x", &with(vec![stat]), &[])
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_plugin_cannot_ask_the_ui_to_poll_faster_than_once_a_second() {
    let mut quick = panel("todo", "Todo");
    quick.refresh_ms = 5;
    let manifest = PluginManifest {
        name: "x".into(),
        panels: vec![quick],
        ..Default::default()
    };
    let panels = rpc::parse_panels("x", &manifest, &[]).unwrap();
    assert_eq!(panels[0].refresh_ms, 1_000);
}

#[test]
fn hosts_without_ready_plugins_contribute_no_panels() {
    let dir = temp_dir("nopanels");
    let host = PluginHost::discover(&[cfg("alpha")], &dir);
    assert!(host.panels().is_empty());
    assert!(host.panels_handle().panels().is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn reading_a_panel_from_an_unknown_or_unready_plugin_is_a_clear_error() {
    let dir = temp_dir("panelerrors");
    // `sh -c "exit 0"` never completes the handshake, so the slot ends `failed`
    // with a reason: reading its panel must say so instead of hanging.
    let mut host = PluginHost::discover(&[cfg("alpha")], &dir);
    host.start_all().await;
    let panels = host.panels_handle();
    let ctx = ToolCallContext::default();

    let unknown = panels.panel("nope", "todo", &ctx).await.unwrap_err();
    assert_eq!(unknown.code(), "not_found");
    assert!(unknown.to_string().contains("nope"), "{unknown}");

    let unready = panels.panel("alpha", "todo", &ctx).await.unwrap_err();
    assert!(matches!(unready, kc_core::Error::Plugin(_)), "{unready:?}");
    assert!(unready.to_string().contains("alpha"), "{unready}");

    // A disabled plugin is a configuration problem, not a broken one.
    let dir2 = temp_dir("panelerrors-disabled");
    let mut off = cfg("beta");
    off.enabled = false;
    let host = PluginHost::discover(&[off], &dir2);
    let panels = host.panels_handle();
    let disabled = panels.panel("beta", "todo", &ctx).await.unwrap_err();
    assert!(
        matches!(disabled, kc_core::Error::Config(_)),
        "{disabled:?}"
    );
    assert!(disabled.to_string().contains("disabled"), "{disabled}");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&dir2).ok();
}
