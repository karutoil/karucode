//! End-to-end `karucode plugin install`, over the same entry points the CLI
//! uses (`plugin_install::cli` / `::install`), against real files:
//!
//! * a local plugin collection directory (the layout documented in the
//!   `plugins` topic) with a real Python plugin and a real `[install]` build;
//! * a fake codeload GitHub server serving a tarball, so the remote path is
//!   exercised without network access.
//!
//! The point of these tests is the whole pipeline: resolve, copy, build,
//! rewrite the command, write `install.json`, refuse politely when the target
//! exists, and never write a manifest the host would later reject.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use kc_server::plugin_install::{
    cli, install, parse_plugin_install, preview, PluginInstallOptions, DEFAULT_REF,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn tmp_tag(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kc-plugin-install-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A plugin collection shaped like the documented repository format: an
/// `index.toml` and one directory per plugin.
fn write_collection(root: &Path) {
    std::fs::write(
        root.join("index.toml"),
        "# One entry per plugin in this repository.\n\
         [[plugins]]\n\
         name = \"wordy\"\n\
         path = \"wordy\"\n\
         description = \"Counts words\"\n\n\
         [[plugins]]\n\
         name = \"pytools\"\n\
         path = \"pytools\"\n\
         description = \"Python demo tools\"\n",
    )
    .unwrap();

    let wordy = root.join("wordy");
    std::fs::create_dir_all(wordy.join("src")).unwrap();
    std::fs::write(
        wordy.join("plugin.toml"),
        "name = \"wordy\"\n\
         description = \"Counts words\"\n\
         command = [\"./wordy.sh\"]\n\
         enabled = true\n\
         permissions = [\"files.read\"]\n\n\
         [install]\n\
         build = [\"cargo\", \"build\", \"--release\"]\n\
         build_timeout_secs = 300\n",
    )
    .unwrap();
    std::fs::write(
        wordy.join("src").join("main.rs"),
        "fn main() { println!(\"hello from wordy\"); }\n",
    )
    .unwrap();
    std::fs::write(
        wordy.join("Cargo.toml"),
        "[package]\nname = \"wordy\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    // A shell entry point with the exec bit: the relative-command rewrite
    // must map it into the install and keep it executable.
    let script = wordy.join("wordy.sh");
    std::fs::write(&script, "#!/bin/sh\necho wordy\n").unwrap();
    make_exec(&script);

    let pytools = root.join("pytools");
    std::fs::create_dir_all(&pytools).unwrap();
    std::fs::write(
        pytools.join("plugin.toml"),
        "name = \"pytools\"\n\
         command = [\"python3\", \"tool.py\"]\n\
         enabled = true\n\
         permissions = [\"files.read\"]\n",
    )
    .unwrap();
    std::fs::write(pytools.join("tool.py"), "# demo python plugin\n").unwrap();

    // A directory with no plugin.toml: the auto-scan must skip it.
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs").join("README.md"), "# docs\n").unwrap();
}

#[cfg(unix)]
fn make_exec(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(not(unix))]
fn make_exec(_path: &Path) {}

/// A minimal `.tar.gz` of the collection tree, one leading directory, exactly
/// what codeload serves.
fn tar_gz_of_tree(root: &Path, top: impl AsRef<str>) -> Vec<u8> {
    let top = top.as_ref();
    let mut builder = tar::Builder::new(Vec::new());
    let mut paths: Vec<PathBuf> = Vec::new();
    collect(root, &mut paths);
    paths.sort();
    for path in paths {
        if path.is_dir() {
            continue; // parents are created on extract; dirs carry no data
        }
        let rel = path.strip_prefix(root).unwrap();
        let contents = std::fs::read(&path).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        let mode = if rel.to_string_lossy().ends_with(".sh") {
            0o755
        } else {
            0o644
        };
        header.set_mode(mode);
        header.set_cksum();
        let name = format!("{}/{}", top, rel.to_string_lossy());
        builder
            .append_data(&mut header, name, std::io::Cursor::new(contents))
            .unwrap();
    }
    let tar_bytes = builder.into_inner().unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar_bytes).unwrap();
    encoder.finish().unwrap()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.push(path.clone());
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// A local axum server that answers one codeload-shaped URL with a tarball of
/// the collection, plus a Content-Disposition naming a commit sha.
async fn fake_codeload(tarball: Vec<u8>, sha: &str) -> (String, tokio::task::JoinHandle<()>) {
    use axum::response::IntoResponse as _;
    let tarball: &'static [u8] = Box::leak(tarball.into_boxed_slice());
    let leaked: &'static str = Box::leak(sha.to_string().into_boxed_str());
    let sha = leaked;
    let app = axum::Router::new().route(
        "/:owner/:repo/tar.gz/:reference",
        axum::routing::get(
            move |axum::extract::Path((owner, repo, reference)): axum::extract::Path<(
                String,
                String,
                String,
            )>| {
                let tarball = tarball;
                let sha = sha;
                async move {
                    if reference != DEFAULT_REF && reference != "main" {
                        return (
                            axum::http::StatusCode::NOT_FOUND,
                            format!("no ref {reference} in {owner}/{repo}"),
                        )
                            .into_response();
                    }
                    let mut response =
                        axum::response::Response::new(axum::body::Body::from(tarball));
                    response.headers_mut().insert(
                        "content-disposition",
                        axum::http::HeaderValue::from_str(&format!(
                            "attachment; filename=repo-{sha}.tar.gz"
                        ))
                        .unwrap(),
                    );
                    response
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), handle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn options(words: &[&str]) -> PluginInstallOptions {
    // `parse_plugin_install` takes the words after `plugin install`, so the
    // test helper must not include the command itself.
    let words: Vec<String> = words.iter().map(|w| w.to_string()).collect();
    parse_plugin_install(&words).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_a_local_collection_into_the_data_dir() {
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = tmp_tag("data");

    let opts = options(&[
        collection.to_str().unwrap(),
        "--data-dir",
        data_dir.to_str().unwrap(),
    ]);
    install(&opts, &data_dir, &kc_server::plugin_install::NullReporter)
        .await
        .unwrap();

    // Both plugins are under <data_dir>/plugins with their files, and the
    // commands point inside the install directories.
    let wordy = data_dir.join("plugins").join("wordy");
    assert!(wordy.join("plugin.toml").is_file());
    assert!(wordy.join("install.json").is_file());
    assert!(wordy.join("wordy.sh").is_file());
    let manifest = std::fs::read_to_string(wordy.join("plugin.toml")).unwrap();
    assert!(
        manifest.contains("command = [\""),
        "the command line was rewritten in place:\n{manifest}"
    );
    assert!(
        !manifest.contains("./wordy.sh"),
        "the relative command path was mapped into the install:\n{manifest}"
    );
    let rewritten_command = manifest
        .lines()
        .find(|l| l.starts_with("command"))
        .unwrap()
        .to_string();
    assert!(rewritten_command.contains("wordy.sh"));
    // The command names a file that actually exists there.
    let command_path = rewritten_command
        .trim_start_matches("command = [\"")
        .trim_end_matches("\"]")
        .to_string();
    assert!(Path::new(&command_path).is_file(), "{rewritten_command}");
    make_exec_check(Path::new(&command_path));

    let pytools = data_dir.join("plugins").join("pytools");
    assert!(pytools.join("plugin.toml").is_file());
    let py_manifest = std::fs::read_to_string(pytools.join("plugin.toml")).unwrap();
    assert!(py_manifest.contains("python3"));
    let tool = py_manifest
        .lines()
        .find(|l| l.starts_with("command"))
        .unwrap();
    let tool_path = tool
        .trim_start_matches("command = [\"python3\", \"")
        .trim_end_matches("\"]")
        .to_string();
    assert!(Path::new(&tool_path).is_file(), "{tool}");

    // install.json records the local provenance.
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(wordy.join("install.json")).unwrap())
            .unwrap();
    assert_eq!(record["kind"], "local");
    assert!(record["source"].as_str().unwrap().starts_with("local:"));

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
}

fn make_exec_check(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the exec bit survived the install");
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_to_overwrite_without_force_and_replaces_with_it() {
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = tmp_tag("data");

    let base = [
        collection.to_str().unwrap(),
        "--data-dir",
        data_dir.to_str().unwrap(),
    ];
    install(
        &options(&base),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await
    .unwrap();

    // Second install without --force is refused and changes nothing.
    let err = install(
        &options(&base),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await
    .unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("--force"), "{chain}");

    // Touch the source so a forced reinstall is visible, then go again.
    std::fs::write(collection.join("wordy").join("marker.txt"), "regenerated\n").unwrap();
    let mut forced = base.to_vec();
    forced.push("--force");
    let result = install(
        &options(&forced),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await;
    if let Err(error) = &result {
        // The full chain, so a failure here is diagnosable from the output.
        panic!("forced reinstall failed: {error:#}",);
    }

    let wordy = data_dir.join("plugins").join("wordy");
    assert!(
        wordy.join("marker.txt").is_file(),
        "the forced reinstall copied fresh sources"
    );
    // The old directory was moved aside, not leaked in place.
    assert!(
        !data_dir.join("plugins").join("wordy.old").exists(),
        "the old install was removed by the successful swap"
    );

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn list_shows_what_a_source_offers() {
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = tmp_tag("data");

    let words = [
        collection.to_str().unwrap(),
        "--list",
        "--data-dir",
        data_dir.to_str().unwrap(),
    ];
    // `--list` is a preview: data out, nothing installed.
    let result = preview(&options(&words), &data_dir).await.unwrap();
    assert!(!data_dir.join("plugins").exists());
    let names: Vec<&str> = result.plugins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["wordy", "pytools"]);
    // The preview describes what an install review needs: which plugin, what
    // it asks to touch, whether it builds.
    assert_eq!(result.kind, "local");
    assert_eq!(result.plugins[0].permissions, vec!["files.read"]);
    assert!(result.plugins[0].builds, "wordy has [install].build");
    assert!(!result.plugins[1].builds, "pytools is a plain script");

    // A second preview against a data dir that already holds wordy marks it.
    install(
        &options(&[
            collection.to_str().unwrap(),
            "pytools",
            "--data-dir",
            data_dir.to_str().unwrap(),
        ]),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await
    .unwrap();
    let result = preview(&options(&words), &data_dir).await.unwrap();
    let by_name = |name: &str| result.plugins.iter().find(|p| p.name == name).unwrap();
    assert!(by_name("pytools").already_installed);
    assert!(!by_name("wordy").already_installed);

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn selects_named_plugins_and_refuses_unknown_names() {
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = tmp_tag("data");

    let words = [
        collection.to_str().unwrap(),
        "pytools",
        "--data-dir",
        data_dir.to_str().unwrap(),
    ];
    install(
        &options(&words),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await
    .unwrap();
    assert!(data_dir.join("plugins").join("pytools").is_file_is_dir());
    assert!(!data_dir.join("plugins").join("wordy").exists());

    let bad = [
        collection.to_str().unwrap(),
        "nope",
        "--data-dir",
        data_dir.to_str().unwrap(),
    ];
    let err = install(
        &options(&bad),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("not `nope`"), "{err}");

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
}

/// Small helper so the assertion reads naturally.
trait IsDir {
    fn is_file_is_dir(&self) -> bool;
}

impl IsDir for PathBuf {
    fn is_file_is_dir(&self) -> bool {
        self.is_dir()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_from_a_remote_codeload_tarball() {
    let collection = tmp_tag("collection-src");
    write_collection(&collection);
    let sha = "deadbeef1234567890abcdefdeadbeef12345678";
    let tarball = tar_gz_of_tree(&collection, format!("repo-{sha}"));
    let (base, _server) = fake_codeload(tarball, sha).await;

    // The remote pipeline against the fake codeload: download (with the
    // Content-Disposition sha pinned) → extract → resolve → install. The
    // download runs through the same `download_tarball` the real pipeline
    // uses, against a local server standing in for codeload.github.com; the
    // URL construction itself is pinned in the unit tests
    // (`codeload_url_is_the_anonymous_tarball_endpoint`).
    let url = format!("{base}/karucode/kc-plugins/tar.gz/{DEFAULT_REF}");
    let (bytes, resolved) =
        kc_server::plugin_install::download_tarball(&url, &kc_server::plugin_install::NullReporter)
            .await
            .unwrap();
    assert_eq!(resolved.as_deref(), Some(sha));
    assert!(!bytes.is_empty());

    let data_dir = tmp_tag("data");
    let extracted = tmp_tag("extracted");
    kc_server::plugin_install::extract_tarball(&bytes, &extracted).unwrap();
    assert!(extracted.join("index.toml").is_file());
    assert!(extracted.join("wordy").join("plugin.toml").is_file());
    // The codeload top directory was stripped.
    assert!(!extracted.join(format!("repo-{sha}")).exists());

    // Install through the public entry point with the extracted tree as the
    // source: this runs the exact build/rewrite/swap code the remote path
    // runs after extraction. The source is a directory on disk, so the
    // record's `kind` is `local` here; the remote fields (`kind` = remote,
    // `resolved`, `tarball_sha256`) are carried by `Repository` and are
    // pinned by the `resolved` assertion above.
    let opts = options(&[
        extracted.to_str().unwrap(),
        "--data-dir",
        data_dir.to_str().unwrap(),
    ]);
    install(&opts, &data_dir, &kc_server::plugin_install::NullReporter)
        .await
        .unwrap();

    let wordy = data_dir.join("plugins").join("wordy");
    assert!(wordy.join("plugin.toml").is_file());
    assert!(wordy.join("install.json").is_file());
    assert!(wordy.join("wordy.sh").is_file());
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(wordy.join("install.json")).unwrap())
            .unwrap();
    assert!(record["source"].is_string());
    assert!(record["command"].is_array());

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
    std::fs::remove_dir_all(&extracted).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_only_keys_are_stripped_from_the_written_manifest() {
    // A repository manifest declaring an operator-only grant installs anyway,
    // with the grant removed from what lands on disk — the same fail-closed
    // behaviour the host applies to a disk manifest, not a refusal that would
    // leave the operator without the plugin.
    let collection = tmp_tag("operator-only");
    let plugin = collection.join("wordy");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        "name = \"wordy\"\ncommand = [\"true\"]\npermissions = [\"*\"]\n\n\
         [settings]\ntrust = true\nread_only_tools = [\"count\"]\n",
    )
    .unwrap();
    let data_dir = tmp_tag("data");
    install(
        &options(&[
            collection.to_str().unwrap(),
            "--data-dir",
            data_dir.to_str().unwrap(),
        ]),
        &data_dir,
        &kc_server::plugin_install::NullReporter,
    )
    .await
    .unwrap();

    let installed = data_dir.join("plugins").join("wordy");
    assert!(installed.join("plugin.toml").is_file());
    let manifest = std::fs::read_to_string(installed.join("plugin.toml")).unwrap();
    assert!(!manifest.contains('*'), "{manifest}");
    assert!(!manifest.contains("trust"), "{manifest}");
    assert!(!manifest.contains("read_only_tools"), "{manifest}");
    // It still parses, and the command survived.
    assert!(manifest.contains("command"));
    let parsed: kc_server::plugin_install::PluginFileConfig = toml::from_str(&manifest).unwrap();
    assert!(parsed.permissions.is_empty());

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cli_wrapper_resolves_the_data_dir_from_config() {
    // `cli` reads the config file; point it at a temp config with a temp
    // data dir and confirm the plugin lands there.
    let collection = tmp_tag("collection");
    write_collection(&collection);
    let data_dir = tmp_tag("data");
    let config_dir = tmp_tag("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("config.toml");
    std::fs::write(&config, format!("data_dir = \"{}\"\n", data_dir.display())).unwrap();

    let mut opts = parse_plugin_install(&[
        "wordy-only".to_string(),
        "--config".to_string(),
        config.to_str().unwrap().to_string(),
    ])
    .unwrap();
    opts.source = collection.to_string_lossy().to_string();
    opts.names = vec!["wordy".into()];
    cli(opts).await.unwrap();

    assert!(data_dir
        .join("plugins")
        .join("wordy")
        .join("plugin.toml")
        .is_file());

    std::fs::remove_dir_all(&collection).ok();
    std::fs::remove_dir_all(&data_dir).ok();
    std::fs::remove_dir_all(&config_dir).ok();
}

#[test]
fn parse_flags_and_refuse_bad_combinations() {
    let opts = parse_plugin_install(&[
        "owner/repo".to_string(),
        "wordy".to_string(),
        "--ref".to_string(),
        "v2".to_string(),
        "--force".to_string(),
        "--build-timeout-secs=120".to_string(),
    ])
    .unwrap();
    assert_eq!(opts.source, "owner/repo");
    assert_eq!(opts.names, vec!["wordy"]);
    assert_eq!(opts.git_ref.as_deref(), Some("v2"));
    assert!(opts.force);
    assert_eq!(opts.build_timeout_secs, Some(120));

    assert!(parse_plugin_install(&[]).is_err());
    assert!(parse_plugin_install(&["--no-build".into()]).is_err());
    assert!(
        parse_plugin_install(&["x".to_string(), "--list".to_string(), "--force".to_string(),])
            .is_err()
    );
    assert!(parse_plugin_install(&["x".to_string(), "--bogus".to_string()]).is_err());

    // Unknown-name refusal names the offered plugins (covered end-to-end
    // elsewhere); the parse itself accepts any names.
    let opts =
        parse_plugin_install(&["a/b".to_string(), "one".to_string(), "two".to_string()]).unwrap();
    assert_eq!(opts.names.len(), 2);
}
