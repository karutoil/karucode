//! Tests run against real temporary workspaces and the real `/bin/sh`, `rg` and
//! `git` binaries. Nothing is mocked except approval and settings.

use crate::command::BoundedOutput;
use crate::file_ops;
use crate::search::{self, SearchEngine, SearchQuery};
use crate::*;
use async_trait::async_trait;
use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use kc_core::config::{ApprovalMode, ApprovalPolicy, Limits, ModelConfig, SubagentConfig};
use kc_core::events::{Event, EventSink};
use kc_core::ids::{RunId, SessionId};
use kc_core::paths::Workspace;
use kc_core::settings::Settings;
use kc_core::tool::{ApprovalGate, ApprovalHandle, ToolCtx};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------- harness ---

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(tag: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kc-tools-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, rel: &str, content: &str) {
        self.write_bytes(rel, content.as_bytes());
    }

    fn write_bytes(&self, rel: &str, bytes: &[u8]) {
        let path = self.path.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path.join(rel)).unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        self.path.join(rel).exists()
    }

    fn entries(&self) -> Vec<String> {
        std::fs::read_dir(&self.path)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect()
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct TestSettings {
    limits: Limits,
    policy: ApprovalPolicy,
}

#[async_trait]
impl Settings for TestSettings {
    async fn model(&self) -> ModelConfig {
        ModelConfig::default()
    }
    async fn limits(&self) -> Limits {
        self.limits.clone()
    }
    async fn approval_policy(&self) -> ApprovalPolicy {
        self.policy.clone()
    }
    async fn subagent_config(&self) -> SubagentConfig {
        SubagentConfig::default()
    }
}

struct RecordingGate {
    decision: ApprovalDecision,
    requests: Mutex<Vec<ApprovalRequest>>,
}

#[async_trait]
impl ApprovalGate for RecordingGate {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        self.requests.lock().unwrap().push(request);
        self.decision
    }
}

struct TestEnv {
    tmp: TempWorkspace,
    ws: Arc<Workspace>,
    ctx: ToolCtx,
    gate: Arc<RecordingGate>,
    events: Arc<Mutex<Vec<Event>>>,
}

fn ask() -> ApprovalPolicy {
    ApprovalPolicy::default()
}

fn read_only() -> ApprovalPolicy {
    ApprovalPolicy {
        mode: ApprovalMode::ReadOnly,
        ..Default::default()
    }
}

fn env_with(
    tag: &str,
    policy: ApprovalPolicy,
    limits: Limits,
    decision: ApprovalDecision,
) -> TestEnv {
    let tmp = TempWorkspace::new(tag);
    let ws = Arc::new(Workspace::new(tmp.path()).unwrap());
    let gate = Arc::new(RecordingGate {
        decision,
        requests: Mutex::new(Vec::new()),
    });
    let events: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = events.clone();
    let sink: EventSink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    let ctx = ToolCtx {
        workspace: ws.clone(),
        cancel: CancellationToken::new(),
        approvals: ApprovalHandle::new(gate.clone(), SessionId::new(), RunId::new(), None),
        events: sink,
        settings: Arc::new(TestSettings { limits, policy }),
        depth: 0,
    };
    TestEnv {
        tmp,
        ws,
        ctx,
        gate,
        events,
    }
}

fn env(tag: &str) -> TestEnv {
    env_with(tag, ask(), Limits::default(), ApprovalDecision::AllowOnce)
}

async fn call(tool: &dyn Tool, ctx: &ToolCtx, args: serde_json::Value) -> ToolOutcome {
    tool.invoke(
        kc_core::tool::ToolCallRequest {
            call_id: "call_test".into(),
            name: tool.spec().name.clone(),
            arguments: args,
        },
        ctx.clone(),
    )
    .await
}

// ---------------------------------------------------------------- file ops ---

#[tokio::test]
async fn read_file_returns_header_and_numbered_lines() {
    let env = env("read-happy");
    env.tmp.write("src/a.txt", "alpha\nbeta\ngamma\n");

    let out = call(&ReadFile, &env.ctx, json!({"path": "src/a.txt"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content
            .starts_with("src/a.txt — lines 1-3 of 3 (17 bytes)"),
        "{}",
        out.content
    );
    assert!(out.content.contains("    1\talpha"), "{}", out.content);
    assert!(out.content.contains("    3\tgamma"), "{}", out.content);
    assert_eq!(out.meta.unwrap()["lines_total"], json!(3));
    // Read-only tools never ask for approval.
    assert!(env.gate.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn read_file_honours_offset_and_limit() {
    let env = env("read-offset");
    let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    env.tmp.write("ten.txt", &body);

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "ten.txt", "offset": 3, "limit": 2}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("lines 3-4 of 10"), "{}", out.content);
    assert!(out.content.contains("    3\tline3"));
    assert!(out.content.contains("    4\tline4"));
    assert!(!out.content.contains("line5"));

    // Past the end is a note, not a crash.
    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "ten.txt", "offset": 99}),
    )
    .await;
    assert!(out.ok);
    assert!(
        out.content.contains("past the end of the file"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn read_file_refuses_binary_content() {
    let env = env("read-binary");
    env.tmp
        .write_bytes("blob.bin", &[0x89, b'P', b'N', b'G', 0x00, 0x01, 0x02]);

    let out = call(&ReadFile, &env.ctx, json!({"path": "blob.bin"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("binary file"), "{}", out.content);
    assert!(out.content.contains("blob.bin"), "{}", out.content);
}

#[tokio::test]
async fn read_file_notes_the_byte_limit() {
    let limits = Limits {
        max_file_read_bytes: 16,
        ..Default::default()
    };
    let env = env_with("read-limits", ask(), limits, ApprovalDecision::AllowOnce);
    let body: String = (1..=50).map(|i| format!("row {i}\n")).collect();
    env.tmp.write("big.txt", &body);

    let out = call(&ReadFile, &env.ctx, json!({"path": "big.txt"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("max_file_read_bytes"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("of 341 bytes were read"),
        "{}",
        out.content
    );
    assert!(out.truncated);
}

#[tokio::test]
async fn read_file_output_is_capped_by_the_tool_limit() {
    let limits = Limits {
        max_tool_output_bytes: 120,
        ..Default::default()
    };
    let env = env_with("read-cap", ask(), limits, ApprovalDecision::AllowOnce);
    let body: String = (1..=50).map(|i| format!("row {i}\n")).collect();
    env.tmp.write("many.txt", &body);

    let out = call(&ReadFile, &env.ctx, json!({"path": "many.txt"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.truncated);
    assert!(out.content.contains("[truncated:"), "{}", out.content);
    assert!(
        out.content.len() < 400,
        "output should be capped: {:?}",
        out.content
    );
}

#[tokio::test]
async fn read_file_rejects_directory_and_missing_path() {
    let env = env("read-shape");
    std::fs::create_dir_all(env.tmp.path().join("adir")).unwrap();

    let out = call(&ReadFile, &env.ctx, json!({"path": "adir"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("is a directory"), "{}", out.content);

    let out = call(&ReadFile, &env.ctx, json!({"path": "nope.txt"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("does not exist"), "{}", out.content);

    let out = call(&ReadFile, &env.ctx, json!({})).await;
    assert!(!out.ok);
    assert!(
        out.content.contains("missing string argument `path`"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn traversal_and_outside_absolute_paths_are_rejected() {
    let env = env("escape");
    env.tmp.write("inside.txt", "ok");

    for bad in ["../outside.txt", "/etc/passwd", "sub/../../etc/shadow"] {
        let read = call(&ReadFile, &env.ctx, json!({"path": bad})).await;
        assert!(!read.ok, "read of {bad} should fail");
        assert!(
            read.content.contains("escapes workspace root")
                || read.content.contains("does not exist"),
            "unexpected message for {bad}: {}",
            read.content
        );

        let write = call(&WriteFile, &env.ctx, json!({"path": bad, "content": "x"})).await;
        assert!(!write.ok, "write of {bad} should fail: {}", write.content);
    }
    // No file was created outside, and no approval was requested for escapes.
    assert!(!Path::new("/tmp/kc-tools-escape-outside.txt").exists());
    assert!(env.gate.requests.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected_for_read_and_write() {
    let env = env("symlink");
    let outside = TempWorkspace::new("symlink-outside");
    outside.write("secret.txt", "top secret");
    std::os::unix::fs::symlink(outside.path(), env.tmp.path().join("link")).unwrap();

    let read = call(&ReadFile, &env.ctx, json!({"path": "link/secret.txt"})).await;
    assert!(!read.ok, "symlink read should fail: {}", read.content);
    assert!(!read.content.contains("top secret"));

    let write = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "link/planted.txt", "content": "x"}),
    )
    .await;
    assert!(!write.ok, "symlink write should fail: {}", write.content);
    assert!(!outside.exists("planted.txt"));
}

#[tokio::test]
async fn list_dir_sorts_dirs_first_and_skips_noise() {
    let env = env("list");
    env.tmp.write("a.txt", "hello");
    env.tmp.write("z.txt", "x");
    env.tmp.write("src/main.rs", "fn main() {}\n");
    env.tmp.write(".git/config", "[core]\n");
    env.tmp.write("node_modules/pkg/index.js", "1\n");
    env.tmp.write(".gitignore", "ignored.txt\nignored-dir/\n");
    env.tmp.write("ignored.txt", "should not be listed");
    env.tmp
        .write("ignored-dir/secret.txt", "should not be listed");

    let out = call(&ListDir, &env.ctx, json!({"path": "."})).await;
    assert!(out.ok, "{}", out.content);
    let text = &out.content;
    assert!(text.contains("src/"), "{text}");
    assert!(text.contains("a.txt (5 B)"), "{text}");
    assert!(!text.contains(".git/"), "{text}");
    assert!(!text.contains("node_modules"), "{text}");
    assert!(
        !text.contains("ignored.txt"),
        ".gitignore must apply:\n{text}"
    );
    assert!(
        !text.contains("ignored-dir"),
        ".gitignore must apply:\n{text}"
    );
    let dir_pos = text.find("src/").unwrap();
    let file_pos = text.find("a.txt").unwrap();
    assert!(dir_pos < file_pos, "directories must come first:\n{text}");

    // depth is clamped and a file path is refused.
    let out = call(&ListDir, &env.ctx, json!({"path": "a.txt"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("use read_file"), "{}", out.content);

    let deep = call(&ListDir, &env.ctx, json!({"path": "src", "depth": 9})).await;
    assert!(deep.ok, "{}", deep.content);
    assert!(deep.content.contains("depth 3"), "{}", deep.content);
}

#[tokio::test]
async fn list_dir_truncates_at_entry_cap() {
    let env = env("list-cap");
    for i in 0..20 {
        env.tmp.write(&format!("file{i:02}.txt"), "x");
    }
    // Exercise the public helper directly with a small cap.
    let rendered = file_ops::list_dir(&env.ws, ".", 3, 5).unwrap();
    assert!(rendered.truncated);
    assert!(
        rendered.text.contains("showing the first 5 of 20"),
        "{}",
        rendered.text
    );
}

#[tokio::test]
async fn write_file_creates_parents_and_reports_bytes() {
    let env = env("write-happy");

    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "new/dir/file.txt", "content": "hi\nthere\n"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("wrote 9 bytes (2 lines)"),
        "{}",
        out.content
    );
    assert!(out.content.contains("created new file"), "{}", out.content);
    assert_eq!(env.tmp.read("new/dir/file.txt"), "hi\nthere\n");

    {
        let requests = env.gate.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.kind, ApprovalKind::FileWrite);
        assert_eq!(req.title, "Write new/dir/file.txt");
        assert!(
            req.detail.contains("create new file, 9 bytes"),
            "{}",
            req.detail
        );
        assert_eq!(req.payload["bytes"], json!(9));
        assert_eq!(req.payload["overwrite"], json!(false));
    }

    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "new/dir/file.txt", "content": "x"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("overwrote existing file"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn write_file_is_denied_by_the_user() {
    let env = env_with(
        "write-deny",
        ask(),
        Limits::default(),
        ApprovalDecision::Deny,
    );
    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "blocked.txt", "content": "x"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.content.contains("denied by user"), "{}", out.content);
    assert!(!env.tmp.exists("blocked.txt"));
}

#[tokio::test]
async fn read_only_mode_refuses_mutations_and_shell_without_asking() {
    let env = env_with(
        "read-only",
        read_only(),
        Limits::default(),
        ApprovalDecision::AllowOnce,
    );

    let write = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "a.txt", "content": "x"}),
    )
    .await;
    assert!(!write.ok);
    assert!(write.content.contains("read_only"), "{}", write.content);
    assert!(!env.tmp.exists("a.txt"));

    env.tmp.write("a.txt", "hello\n");
    let edit = call(
        &EditFile,
        &env.ctx,
        json!({"path": "a.txt", "old_string": "hello", "new_string": "bye"}),
    )
    .await;
    assert!(!edit.ok);
    assert!(edit.content.contains("read_only"), "{}", edit.content);

    let shell = call(&RunCommand, &env.ctx, json!({"command": "echo hi"})).await;
    assert!(!shell.ok);
    assert!(shell.content.contains("read_only"), "{}", shell.content);

    assert!(
        env.gate.requests.lock().unwrap().is_empty(),
        "read_only must not prompt the user"
    );
}

#[tokio::test]
async fn write_file_leaves_no_temp_files() {
    let env = env("write-atomic");
    for i in 0..5 {
        let out = call(
            &WriteFile,
            &env.ctx,
            json!({"path": format!("out/f{i}.txt"), "content": "content"}),
        )
        .await;
        assert!(out.ok, "{}", out.content);
    }
    let leftovers: Vec<String> = env
        .tmp
        .entries()
        .into_iter()
        .filter(|n| n.contains("kc-tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
    let out_dir = std::fs::read_dir(env.tmp.path().join("out")).unwrap();
    for entry in out_dir {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.contains("kc-tmp"), "temp file {name} left behind");
    }
}

#[tokio::test]
async fn write_file_refuses_oversized_payloads() {
    let env = env("write-huge");
    let huge = "x".repeat(file_ops::MAX_FILE_WRITE_BYTES + 1);
    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "huge.txt", "content": huge}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.content.contains("capped at 2 MiB"), "{}", out.content);
    assert!(!env.tmp.exists("huge.txt"));
}

#[tokio::test]
async fn edit_file_replaces_exactly_once() {
    let env = env("edit-happy");
    env.tmp
        .write("src/lib.rs", "fn main() {\n    let x = 1;\n}\n");

    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "src/lib.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("1 replacement(s) at line(s) 2"),
        "{}",
        out.content
    );
    assert!(out.content.contains("-    let x = 1;"), "{}", out.content);
    assert!(out.content.contains("+    let x = 2;"), "{}", out.content);
    assert_eq!(
        env.tmp.read("src/lib.rs"),
        "fn main() {\n    let x = 2;\n}\n"
    );

    let requests = env.gate.requests.lock().unwrap();
    assert_eq!(requests[0].kind, ApprovalKind::FileEdit);
    assert_eq!(requests[0].title, "Edit src/lib.rs");
    assert!(requests[0].payload["diff"]
        .as_str()
        .unwrap()
        .contains("+    let x = 2;"));
}

#[tokio::test]
async fn edit_file_reports_missing_old_string() {
    let env = env("edit-missing");
    env.tmp.write("a.txt", "hello world\n");

    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "a.txt", "old_string": "goodbye", "new_string": "x"}),
    )
    .await;
    assert!(!out.ok);
    assert!(
        out.content.contains("old_string not found"),
        "{}",
        out.content
    );
    assert!(out.content.contains("goodbye"), "{}", out.content);
    assert_eq!(env.tmp.read("a.txt"), "hello world\n");
    assert!(
        env.gate.requests.lock().unwrap().is_empty(),
        "no approval before validation"
    );
}

#[tokio::test]
async fn edit_file_refuses_ambiguous_match_without_replace_all() {
    let env = env("edit-ambiguous");
    env.tmp.write("a.txt", "foo\nbar\nfoo\n");

    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "a.txt", "old_string": "foo", "new_string": "baz"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.content.contains("matches 2 places"), "{}", out.content);
    assert!(out.content.contains("lines 1, 3"), "{}", out.content);
    assert_eq!(env.tmp.read("a.txt"), "foo\nbar\nfoo\n");

    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "a.txt", "old_string": "foo", "new_string": "baz", "replace_all": true}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("2 replacement(s) at line(s) 1, 3"),
        "{}",
        out.content
    );
    assert_eq!(env.tmp.read("a.txt"), "baz\nbar\nbaz\n");
}

#[tokio::test]
async fn edit_file_refuses_binary_targets() {
    let env = env("edit-binary");
    env.tmp.write_bytes("b.bin", &[0x00, 0x01, 0x02]);
    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "b.bin", "old_string": "a", "new_string": "b"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.content.contains("binary"), "{}", out.content);
}

// ----------------------------------------------------------------- command ---

#[tokio::test]
async fn run_command_streams_output_and_events() {
    let env = env("cmd-happy");
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "printf 'hello\\n'; printf 'oops\\n' 1>&2"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("exit code: 0"), "{}", out.content);
    assert!(
        out.content.contains("--- stdout (6 bytes) ---"),
        "{}",
        out.content
    );
    assert!(out.content.contains("hello"), "{}", out.content);
    assert!(
        out.content.contains("--- stderr (5 bytes) ---"),
        "{}",
        out.content
    );
    assert!(out.content.contains("oops"), "{}", out.content);

    let events = env.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CommandStarted { command, .. } if command.contains("hello"))));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CommandOutput { stream, .. } if stream == "stdout")));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CommandFinished { exit_code, .. } if *exit_code == Some(0))));
}

#[tokio::test]
async fn run_command_propagates_exit_code() {
    let env = env("cmd-exit");
    let out = call(&RunCommand, &env.ctx, json!({"command": "exit 7"})).await;
    // A nonzero exit is a failed *command*: the tool result must say so (ok=false)
    // while still showing the exit code to the model.
    assert!(!out.ok, "{}", out.content);
    assert!(out.content.contains("exit code: 7"), "{}", out.content);
    assert_eq!(out.meta.unwrap()["exit_code"], json!(7));

    // Non-zero stderr is reported too, and the call still fails.
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "echo boom 1>&2; exit 3"}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
    assert!(out.content.contains("exit code: 3"), "{}", out.content);
    assert!(out.content.contains("boom"), "{}", out.content);
}

#[tokio::test]
async fn run_command_timeout_kills_the_process_group() {
    let env = env("cmd-timeout");
    let started = Instant::now();
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "sleep 5", "timeout_secs": 1}),
    )
    .await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(3),
        "timeout not enforced: {elapsed:?}"
    );
    assert!(!out.ok, "{}", out.content);
    assert!(
        out.content.contains("timed out after 1s"),
        "{}",
        out.content
    );
    let meta = out.meta.unwrap();
    assert_eq!(meta["timed_out"], json!(true));
}

#[tokio::test]
async fn run_command_respects_a_pre_cancelled_token() {
    let env = env("cmd-cancel");
    env.ctx.cancel.cancel();
    let started = Instant::now();
    let out = call(&RunCommand, &env.ctx, json!({"command": "sleep 5"})).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!out.ok);
    assert!(out.content.contains("cancelled"), "{}", out.content);
    assert!(
        !env.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, Event::CommandStarted { .. })),
        "a cancelled run must not start a process"
    );
}

#[tokio::test]
async fn run_command_output_is_bounded_head_and_tail() {
    let limits = Limits {
        max_command_output_bytes: 256,
        ..Default::default()
    };
    let env = env_with("cmd-trunc", ask(), limits, ApprovalDecision::AllowOnce);
    let out = call(&RunCommand, &env.ctx, json!({"command": "seq 1 3000"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.truncated, "{}", out.content);
    assert!(out.content.contains("bytes omitted"), "{}", out.content);
    let meta = out.meta.unwrap();
    assert!(meta["stdout_bytes"].as_u64().unwrap() > 256);
}

#[tokio::test]
async fn run_command_does_not_kill_silent_commands_early() {
    // Regression: a command that redirects both streams away closes the pipes
    // immediately; it must still be allowed to finish within its timeout.
    let env = env("cmd-silent");
    let started = Instant::now();
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "sleep 1 > /dev/null 2>&1"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(out.content.contains("exit code: 0"), "{}", out.content);
    assert!(!out.content.contains("timed out"), "{}", out.content);
}

#[tokio::test]
async fn run_command_clamps_the_requested_timeout() {
    let limits = Limits {
        max_command_timeout_secs: 2,
        ..Default::default()
    };
    let env = env_with("cmd-clamp", ask(), limits, ApprovalDecision::AllowOnce);
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "true", "timeout_secs": 9999}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.meta.unwrap()["timeout_secs"], json!(2));
}

#[tokio::test]
async fn run_command_uses_the_requested_working_directory() {
    let env = env("cmd-workdir");
    env.tmp.write("sub/here.txt", "x");
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "ls", "cwd": "sub"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("cwd: sub"), "{}", out.content);
    assert!(out.content.contains("here.txt"), "{}", out.content);
}

#[tokio::test]
async fn run_command_requires_a_directory_cwd_inside_the_workspace() {
    let env = env("cmd-cwd");
    env.tmp.write("f.txt", "x");

    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "pwd", "cwd": "f.txt"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.content.contains("not a directory"), "{}", out.content);

    let out = call(&RunCommand, &env.ctx, json!({"command": "pwd", "cwd": "/"})).await;
    assert!(!out.ok);
    assert!(
        out.content.contains("escapes workspace root"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn bounded_output_keeps_head_and_tail() {
    let mut buf = BoundedOutput::new(100);
    for _ in 0..10 {
        buf.push(&[b'a'; 50]);
    }
    let finished = buf.finish();
    assert!(finished.truncated);
    assert_eq!(finished.total_bytes, 500);
    assert!(
        finished.text.contains("400 of 500 bytes omitted"),
        "{}",
        finished.text
    );
    assert!(finished.text.starts_with("aaaa"));
    assert!(finished.text.ends_with("aaaa"));
    assert!(finished.text.len() < 300);
}

// ------------------------------------------------------------------ search ---

fn rg_available() -> bool {
    std::process::Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn search_files_finds_matches_and_reports_engine() {
    let env = env("search-happy");
    env.tmp.write("src/a.rs", "fn alpha() {}\nlet beta = 1;\n");
    env.tmp.write("src/b.txt", "alpha\n");
    env.tmp.write(".gitignore", "vendor/\n");
    env.tmp.write("vendor/skip.rs", "alpha\n");

    let out = call(&SearchFiles, &env.ctx, json!({"pattern": "alpha"})).await;
    assert!(out.ok, "{}", out.content);
    let expected_engine = if rg_available() {
        "engine: ripgrep"
    } else {
        "engine: builtin-walker"
    };
    assert!(out.content.contains(expected_engine), "{}", out.content);
    assert!(out.content.contains("src/a.rs:1:"), "{}", out.content);
    assert!(out.content.contains("src/b.txt:1:"), "{}", out.content);
    assert!(
        !out.content.contains("vendor/"),
        "gitignore must apply: {}",
        out.content
    );

    // Glob filter narrows to one file.
    let out = call(
        &SearchFiles,
        &env.ctx,
        json!({"pattern": "alpha", "glob": "*.rs"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("src/a.rs"), "{}", out.content);
    assert!(!out.content.contains("src/b.txt"), "{}", out.content);
}

#[tokio::test]
async fn search_files_caps_results() {
    let limits = Limits {
        search_max_results: 2,
        ..Default::default()
    };
    let env = env_with("search-cap", ask(), limits, ApprovalDecision::AllowOnce);
    let body: String = (1..=50).map(|i| format!("needle {i}\n")).collect();
    env.tmp.write("many.txt", &body);

    let out = call(&SearchFiles, &env.ctx, json!({"pattern": "needle"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("2 match(es)"), "{}", out.content);
    assert!(out.content.contains("cap"), "{}", out.content);
    assert!(out.truncated);
}

#[tokio::test]
async fn search_fallback_walks_gitignore_and_globs() {
    let env = env("search-fallback");
    env.tmp.write("src/a.rs", "let Foo = 1;\n");
    env.tmp.write("src/b.txt", "foo\n");
    env.tmp.write(".gitignore", "ignored/\n");
    env.tmp.write("ignored/c.rs", "foo\n");

    let query = SearchQuery {
        pattern: "foo".into(),
        path: ".".into(),
        glob: Some("*.rs".into()),
        case_insensitive: true,
        max_results: 10,
    };
    let result = search::search_fallback(&env.ws, &query).await.unwrap();
    assert_eq!(result.engine, SearchEngine::Builtin);
    assert_eq!(result.matches.len(), 1, "{:?}", result.matches);
    assert_eq!(result.matches[0].path, "src/a.rs");
    assert_eq!(result.matches[0].line, 1);
    assert!(result.matches[0].text.contains("let Foo = 1;"));

    // Case sensitivity is honoured (the glob still limits us to *.rs).
    let strict = SearchQuery {
        case_insensitive: false,
        ..query.clone()
    };
    let result = search::search_fallback(&env.ws, &strict).await.unwrap();
    assert!(result.matches.is_empty(), "{:?}", result.matches);

    // Without the glob, the case-sensitive search finds the lowercase copy.
    let unglobbing = SearchQuery {
        glob: None,
        ..strict
    };
    let result = search::search_fallback(&env.ws, &unglobbing).await.unwrap();
    let paths: Vec<&str> = result.matches.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, vec!["src/b.txt"], "{:?}", result.matches);
}

#[tokio::test]
async fn search_falls_back_when_ripgrep_is_missing() {
    let env = env("search-norg");
    env.tmp.write("src/a.rs", "needle here\n");
    let query = SearchQuery {
        pattern: "needle".into(),
        path: ".".into(),
        glob: None,
        case_insensitive: false,
        max_results: 10,
    };
    let cancel = CancellationToken::new();

    // The ENOENT path: a binary name that cannot exist.
    let missing =
        search::search_with_program("kc-tools-no-such-rg-binary", &env.ws, &query, &cancel)
            .await
            .unwrap();
    assert_eq!(missing.engine, SearchEngine::Builtin);
    assert_eq!(missing.matches.len(), 1);
    assert_eq!(missing.matches[0].path, "src/a.rs");

    // And `search()` selects the same engine when rg is unavailable.
    let direct = search::search_with_rg("kc-tools-no-such-rg-binary", &env.ws, &query, &cancel)
        .await
        .unwrap();
    assert!(direct.is_none(), "missing binary must signal the fallback");
}

#[tokio::test]
async fn search_fallback_reports_invalid_regex() {
    let env = env("search-bad-regex");
    env.tmp.write("a.txt", "x\n");
    let query = SearchQuery {
        pattern: "(".into(),
        path: ".".into(),
        glob: None,
        case_insensitive: false,
        max_results: 10,
    };
    let err = search::search_fallback(&env.ws, &query).await.unwrap_err();
    assert!(err.to_string().contains("invalid regex"), "{err}");
}

// --------------------------------------------------------------------- git ---

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn git_status_reports_non_repo() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    let env = env("git-norepo");
    let out = call(&GitStatus, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("is_repo: false"), "{}", out.content);
    assert_eq!(out.meta.unwrap()["is_repo"], json!(false));
}

#[tokio::test]
async fn git_status_and_diff_work_in_a_temp_repo() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    let env = env("git-repo");
    env.tmp.write("tracked.txt", "one\n");
    git(env.tmp.path(), &["init", "-q", "-b", "main"]);
    git(env.tmp.path(), &["add", "."]);
    git(env.tmp.path(), &["commit", "-qm", "init"]);

    // Clean tree first.
    let out = call(&GitStatus, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("branch: main"), "{}", out.content);
    assert!(
        out.content.contains("working tree clean"),
        "{}",
        out.content
    );

    env.tmp.write("tracked.txt", "two\n");
    env.tmp.write("untracked.txt", "new\n");

    let out = call(&GitStatus, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("tracked.txt"), "{}", out.content);
    assert!(out.content.contains("?? untracked.txt"), "{}", out.content);
    let meta = out.meta.unwrap();
    assert_eq!(meta["is_repo"], json!(true));
    assert_eq!(meta["branch"], json!("main"));

    let out = call(&GitDiff, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("-one"), "{}", out.content);
    assert!(out.content.contains("+two"), "{}", out.content);
    assert_eq!(out.meta.unwrap()["empty"], json!(false));

    // Staged vs unstaged.
    git(env.tmp.path(), &["add", "tracked.txt"]);
    let staged = call(&GitDiff, &env.ctx, json!({"staged": true})).await;
    assert!(staged.ok, "{}", staged.content);
    assert!(staged.content.contains("+two"), "{}", staged.content);

    let unstaged = call(&GitDiff, &env.ctx, json!({"path": "tracked.txt"})).await;
    assert!(unstaged.ok, "{}", unstaged.content);
    assert!(
        unstaged.content.contains("No unstaged changes"),
        "{}",
        unstaged.content
    );

    // A missing ref is an actionable error, not a panic.
    let bad = call(&GitDiff, &env.ctx, json!({"base": "does-not-exist"})).await;
    assert!(!bad.ok);
    assert!(bad.content.contains("does-not-exist"), "{}", bad.content);
}

// --------------------------------------------------------------- registry ---

#[tokio::test]
async fn builtin_tools_are_complete_and_read_only_by_default() {
    let tools = builtin_tools();
    assert_eq!(tools.len(), 8);
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert_eq!(
        names,
        vec![
            "read_file",
            "list_dir",
            "search_files",
            "write_file",
            "edit_file",
            "run_command",
            "git_status",
            "git_diff"
        ]
    );
    for tool in &tools {
        let spec = tool.spec();
        assert!(
            !spec.description.is_empty(),
            "{} has no description",
            spec.name
        );
        assert_eq!(spec.parameters["type"], json!("object"));
    }
    let get = |n: &str| tools.iter().find(|t| t.spec().name == n).unwrap().class();
    assert_eq!(get("read_file"), kc_core::tool::ToolClass::ReadOnly);
    assert_eq!(get("write_file"), kc_core::tool::ToolClass::Mutating);
    assert_eq!(get("edit_file"), kc_core::tool::ToolClass::Mutating);
    assert_eq!(get("run_command"), kc_core::tool::ToolClass::Shell);
    assert_eq!(get("git_diff"), kc_core::tool::ToolClass::ReadOnly);
}

#[test]
fn auto_decision_maps_policy_to_classes() {
    use kc_core::approval::AutoDecision;
    use kc_core::tool::ToolClass;

    let ro = read_only();
    assert_eq!(
        auto_decision(&ro, ToolClass::ReadOnly, "x"),
        AutoDecision::Allow
    );
    assert_eq!(
        auto_decision(&ro, ToolClass::Mutating, "x"),
        AutoDecision::Deny
    );
    assert_eq!(
        auto_decision(&ro, ToolClass::Shell, "ls"),
        AutoDecision::Deny
    );

    let auto = ApprovalPolicy {
        mode: ApprovalMode::Auto,
        ..Default::default()
    };
    assert_eq!(
        auto_decision(&auto, ToolClass::Shell, "rm -rf /"),
        AutoDecision::Allow
    );
    assert_eq!(
        auto_decision(&auto, ToolClass::Mutating, "a"),
        AutoDecision::Allow
    );

    let allowlisted = ApprovalPolicy {
        mode: ApprovalMode::Ask,
        allow_commands: vec!["cargo test".into()],
        ..Default::default()
    };
    assert_eq!(
        auto_decision(&allowlisted, ToolClass::Shell, "cargo test --all"),
        AutoDecision::Allow
    );
    assert_eq!(
        auto_decision(&allowlisted, ToolClass::Shell, "rm -rf /"),
        AutoDecision::Ask
    );
    assert_eq!(
        auto_decision(&ask(), ToolClass::Mutating, "a"),
        AutoDecision::Ask
    );
}

#[test]
fn diff_preview_is_compact_for_scattered_edits() {
    let mut old: Vec<String> = (1..=200).map(|i| format!("line {i}")).collect();
    let mut new = old.clone();
    new[1] = "changed a".into();
    new[150] = "changed b".into();
    old.remove(0); // keep `old` mutable-used
    old.insert(0, "line 1".to_string());

    let old_refs: Vec<&str> = old.iter().map(|s| s.as_str()).collect();
    let new_refs: Vec<&str> = new.iter().map(|s| s.as_str()).collect();
    let diff = file_ops::render_diff(&old_refs, &new_refs, 80);
    assert!(diff.contains("+changed a"), "{diff}");
    assert!(diff.contains("+changed b"), "{diff}");
    assert!(
        diff.lines().count() <= 82,
        "diff too long: {}",
        diff.lines().count()
    );
    assert!(diff.contains("@@ "), "{diff}");
}
