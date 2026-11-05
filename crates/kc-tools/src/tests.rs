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

/// One journal capture the tools asked for.
#[derive(Debug, Clone, PartialEq)]
struct Captured {
    path: String,
    tool: String,
    /// The bytes the file had when it was captured, so a test can prove the
    /// capture happened *before* the write.
    before: String,
}

/// A journal stand-in that records what the tools asked to have captured.
struct RecordingJournal {
    /// Root used to read the before-image, mirroring what the real journal does.
    root: PathBuf,
    captures: Mutex<Vec<Captured>>,
    /// When false, `capture_path` reports failure so a caller that must refuse
    /// the write can be tested.
    succeed: bool,
}

impl RecordingJournal {
    fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            captures: Mutex::new(Vec::new()),
            succeed: true,
        })
    }

    fn captured(&self) -> Vec<Captured> {
        self.captures.lock().unwrap().clone()
    }
}

#[async_trait]
impl kc_core::tool::JournalHandle for RecordingJournal {
    async fn record(&self, rel: &str, tool: &str) {
        let before = std::fs::read_to_string(self.root.join(rel)).unwrap_or_default();
        self.captures.lock().unwrap().push(Captured {
            path: rel.to_string(),
            tool: tool.to_string(),
            before,
        });
    }

    async fn capture_path(&self, rel: &str, tool: &str) -> bool {
        self.record(rel, tool).await;
        self.succeed
    }
}

#[async_trait]
impl ApprovalGate for RecordingGate {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        self.requests.lock().unwrap().push(request);
        self.decision
    }
}

struct EditDuringApproval {
    root: PathBuf,
}

#[async_trait]
impl ApprovalGate for EditDuringApproval {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        assert_eq!(request.kind, ApprovalKind::FileEdit);
        std::fs::write(self.root.join("a.txt"), "alpha\nCONCURRENT\nomega\n").unwrap();
        ApprovalDecision::AllowOnce
    }
}

struct CancelDuringApproval {
    token: CancellationToken,
}

#[async_trait]
impl ApprovalGate for CancelDuringApproval {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        self.token.cancel();
        ApprovalDecision::AllowOnce
    }
}

fn with_approval_gate(env: &TestEnv, gate: Arc<dyn ApprovalGate>) -> ToolCtx {
    let mut ctx = env.ctx.clone();
    ctx.approvals = ApprovalHandle::new(gate, SessionId::new(), RunId::new(), None);
    ctx
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
        supports_images: false,
        checkpoints: None,
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

/// A `ToolCtx` for a workspace a test built itself (a nested repository, say)
/// rather than the one `TempWorkspace` owns.
fn ctx_for(ws: &Workspace) -> ToolCtx {
    ToolCtx {
        workspace: Arc::new(ws.clone()),
        cancel: CancellationToken::new(),
        approvals: ApprovalHandle::new(
            Arc::new(RecordingGate {
                decision: ApprovalDecision::AllowOnce,
                requests: Mutex::new(Vec::new()),
            }),
            SessionId::new(),
            RunId::new(),
            None,
        ),
        events: kc_core::events::noop_sink(),
        settings: Arc::new(TestSettings {
            limits: Limits::default(),
            policy: ask(),
        }),
        depth: 0,
        supports_images: false,
        checkpoints: None,
    }
}

/// Like [`env`], but the workspace also allows `extra_roots` - the shape the
/// server builds by default, with the system temp dir on the list.
fn env_with_extra_roots(tag: &str, extra_roots: Vec<PathBuf>) -> TestEnv {
    let tmp = TempWorkspace::new(tag);
    let ws = Arc::new(Workspace::with_extra_roots(tmp.path(), extra_roots).unwrap());
    let gate = Arc::new(RecordingGate {
        decision: ApprovalDecision::AllowOnce,
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
        settings: Arc::new(TestSettings {
            limits: Limits::default(),
            policy: ask(),
        }),
        depth: 0,
        supports_images: false,
        checkpoints: None,
    };
    TestEnv {
        tmp,
        ws,
        ctx,
        gate,
        events,
    }
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
    // An ELF header, not an image: image magic bytes are their own case below.
    env.tmp
        .write_bytes("blob.bin", &[0x7f, b'E', b'L', b'F', 0x00, 0x01, 0x02]);

    let out = call(&ReadFile, &env.ctx, json!({"path": "blob.bin"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("binary file"), "{}", out.content);
    assert!(out.content.contains("blob.bin"), "{}", out.content);
}

/// FIX 6(b): the NUL sniff used to look at the first 8 KiB only, so a binary file
/// whose NUL came later was rendered as text and the U+0000 reached the model's
/// context. The sniff now covers the window that is actually returned.
#[tokio::test]
async fn a_nul_past_the_old_sniff_window_is_still_refused() {
    let env = env("read-nul-late");
    let mut bytes = vec![b'a'; 20_000];
    bytes.push(0);
    bytes.extend_from_slice(b"tail\n");
    env.tmp.write_bytes("late.bin", &bytes);

    let out = call(&ReadFile, &env.ctx, json!({"path": "late.bin"})).await;
    assert!(!out.ok, "a NUL at offset 20000 must be refused");
    assert!(
        out.content.contains("NUL byte at offset 20000"),
        "the refusal must name the real offset: {}",
        out.content
    );
    assert!(
        !out.content.contains('\u{0}'),
        "no NUL may reach the model: {:?}",
        out.content
    );

    // The same file below the cap is refused for the same reason.
    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "late.bin", "offset": 1, "limit": 2000}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
}

/// The 8-byte PNG signature plus a truncated chunk: enough for the magic-byte
/// sniffer to classify it, and deliberately not decodable (nothing here decodes
/// images, so a real one would only be a bigger fixture).
const PNG_BYTES: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R',
];

#[tokio::test]
async fn read_file_attaches_an_image_for_a_vision_model() {
    let env = env("read-image");
    env.tmp.write_bytes("assets/shot.png", PNG_BYTES);
    let mut ctx = env.ctx.clone();
    ctx.supports_images = true;

    let out = call(&ReadFile, &ctx, json!({"path": "assets/shot.png"})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.images.len(), 1, "{}", out.content);
    assert_eq!(out.images[0].media_type, "image/png");
    assert_eq!(out.images[0].name.as_deref(), Some("assets/shot.png"));
    assert_eq!(out.images[0].bytes, PNG_BYTES.len() as u64);
    assert!(!out.images[0].data.is_empty());
    // The text tells the model there is no text to read, and names the file.
    assert!(out.content.contains("assets/shot.png"), "{}", out.content);
    assert!(out.content.contains("attached"), "{}", out.content);
    assert_eq!(out.meta.unwrap()["image"], json!(true));
}

#[tokio::test]
async fn a_text_only_model_is_told_it_cannot_see_images() {
    let env = env("read-image-no-vision");
    env.tmp.write_bytes("shot.png", PNG_BYTES);

    let out = call(&ReadFile, &env.ctx, json!({"path": "shot.png"})).await;
    assert!(!out.ok, "a refusal, not an empty success");
    assert!(
        out.images.is_empty(),
        "no pixels for a model that cannot see"
    );
    assert!(out.content.contains("cannot see images"), "{}", out.content);
    assert!(
        out.content.contains("model.supports_images"),
        "the refusal must name the knob: {}",
        out.content
    );
    assert!(
        out.content.contains("Tell the user"),
        "the model is told what to say: {}",
        out.content
    );
}

#[tokio::test]
async fn an_image_format_vision_cannot_take_is_named_with_the_fix() {
    let env = env("read-image-bmp");
    env.tmp
        .write_bytes("scan.bmp", b"BM\x36\x00\x00\x00\x00\x00\x00\x00");
    let mut ctx = env.ctx.clone();
    ctx.supports_images = true;

    let out = call(&ReadFile, &ctx, json!({"path": "scan.bmp"})).await;
    assert!(!out.ok);
    assert!(out.content.contains("image/bmp"), "{}", out.content);
    assert!(out.content.contains("run_command"), "{}", out.content);
    assert!(out.content.contains("image/png"), "{}", out.content);
}

#[tokio::test]
async fn an_over_limit_image_is_refused_and_names_the_cap() {
    let limits = Limits {
        max_image_read_bytes: 8,
        ..Default::default()
    };
    let env = env_with("read-image-cap", ask(), limits, ApprovalDecision::AllowOnce);
    env.tmp.write_bytes("big.png", PNG_BYTES);
    let mut ctx = env.ctx.clone();
    ctx.supports_images = true;

    let out = call(&ReadFile, &ctx, json!({"path": "big.png"})).await;
    assert!(!out.ok);
    assert!(
        out.content.contains("max_image_read_bytes"),
        "{}",
        out.content
    );
    assert!(out.content.contains("big.png"), "{}", out.content);
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

/// The point of the whole extra-root feature: a scratch directory outside the
/// project is a first-class path for every file tool, not a path error.
#[tokio::test]
async fn a_shared_temp_dir_is_usable_by_the_file_tools() {
    // Stands in for `/tmp`: a directory that is not the workspace.
    let shared = TempWorkspace::new("shared");
    let env = env_with_extra_roots("extra-root", vec![shared.path().to_path_buf()]);
    env.tmp.write("inside.txt", "project file");

    // write_file creates a file in the shared dir (and its parents).
    let abs = shared.path().join("nested/scratch.txt");
    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": abs.to_str().unwrap(), "content": "from the temp dir"}),
    )
    .await;
    assert!(out.ok, "write into the temp dir failed: {}", out.content);
    assert_eq!(
        std::fs::read_to_string(&abs).unwrap(),
        "from the temp dir",
        "the bytes must really land in the temp dir"
    );

    // read_file reads it back, and also reads a file something else dropped
    // there (the "files appear in /tmp automatically" case).
    std::fs::write(shared.path().join("dropped.log"), "auto\n").unwrap();
    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": shared.path().join("dropped.log").to_str().unwrap()}),
    )
    .await;
    assert!(out.ok, "read from the temp dir failed: {}", out.content);
    assert!(out.content.contains("auto"), "{}", out.content);

    // edit_file works there too: it resolves through the same check.
    let out = call(
        &EditFile,
        &env.ctx,
        json!({
            "path": abs.to_str().unwrap(),
            "old_string": "temp dir",
            "new_string": "scratch space",
        }),
    )
    .await;
    assert!(out.ok, "edit in the temp dir failed: {}", out.content);
    assert_eq!(
        std::fs::read_to_string(&abs).unwrap(),
        "from the scratch space"
    );

    // The project itself still works, and a path outside *both* roots is still
    // refused - the allowlist did not turn into "any absolute path".
    let outside = call(&ReadFile, &env.ctx, json!({"path": "/etc/hostname"})).await;
    assert!(!outside.ok, "{}", outside.content);
    assert!(
        outside.content.contains("escapes workspace root"),
        "{}",
        outside.content
    );
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
    // The walk now stops at the cap instead of walking the whole tree and
    // throwing it away, so the total is a *lower bound* (`5+`), not a count the
    // call never established.
    assert!(
        rendered.text.contains("showing the first 5 of 5+ entries"),
        "{}",
        rendered.text
    );
    assert_eq!(rendered.meta["shown"], json!(5));
    assert_eq!(rendered.meta["entries_capped"], json!(true));
    // What is shown stays sorted (directories first, then by path). The *set*
    // that fits under the cap is whatever `readdir` yielded first - the walk no
    // longer reads the whole tree to pick the alphabetically-first 5 - but the
    // rows that are shown are ordered.
    let rows: Vec<&str> = rendered
        .text
        .lines()
        .skip(1)
        .filter(|line| !line.starts_with('['))
        .collect();
    assert_eq!(rows.len(), 5, "{}", rendered.text);
    let mut sorted = rows.clone();
    sorted.sort_unstable();
    assert_eq!(rows, sorted, "{}", rendered.text);

    // A listing that fits has an exact count and no truncation note.
    let roomy = env_with(
        "list-cap-roomy",
        ask(),
        Limits::default(),
        ApprovalDecision::AllowOnce,
    );
    for i in 0..3 {
        roomy.tmp.write(&format!("f{i}.txt"), "x");
    }
    let rendered = file_ops::list_dir(&roomy.ws, ".", 1, 5).unwrap();
    assert!(!rendered.truncated, "{}", rendered.text);
    assert!(
        rendered
            .text
            .starts_with("Listing . (depth 1, 3 entries, 3 shown)"),
        "{}",
        rendered.text
    );
    assert_eq!(rendered.meta["entries"], json!(3));
    assert_eq!(rendered.meta["entries_capped"], json!(false));
    assert!(!rendered.text.contains("truncated"), "{}", rendered.text);
}

/// FIX 6(a): the walk stops at the cap instead of materialising the whole tree.
///
/// The observable part is that the entry count is a lower bound (`N+`) for a
/// truncated listing, and the number of rows equals the cap - a walk that kept
/// going would still print `max_entries` rows, so the assertion that matters is
/// that the *count* is a bound and the metadata says it was capped. The timing
/// guard below fails if the walk ever goes back to reading every entry.
#[tokio::test]
async fn list_dir_stops_walking_at_the_cap() {
    let env = env("list-walk-cap");
    // 20 000 files: enough that a full walk is measurably slower than 500.
    for i in 0..20_000 {
        env.tmp.write(&format!("d{:02}/f{i:05}.txt", i % 20), "x");
    }
    let started = Instant::now();
    let rendered = file_ops::list_dir(&env.ws, "d00", 1, 20).unwrap();
    let elapsed = started.elapsed();
    assert!(rendered.truncated, "{}", rendered.text);
    assert_eq!(rendered.meta["shown"], json!(20));
    assert_eq!(
        rendered.meta["entries_capped"],
        json!(true),
        "{}",
        rendered.text
    );
    assert!(
        rendered.meta["entries"].as_u64().unwrap() <= 21,
        "the walk must stop at the cap (one entry past it at most), not collect the \
         tree: {}",
        rendered.text
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "a capped listing must not walk the whole tree; took {elapsed:?}"
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
async fn edit_file_revalidates_after_approval_and_never_overwrites_a_concurrent_edit() {
    let env = env("edit-stale-approval");
    env.tmp.write("a.txt", "alpha\nomega\n");
    let gate = Arc::new(EditDuringApproval {
        root: env.tmp.path().to_path_buf(),
    });
    let ctx = with_approval_gate(&env, gate);

    let out = call(
        &EditFile,
        &ctx,
        json!({"path": "a.txt", "old_string": "omega", "new_string": "replacement"}),
    )
    .await;
    assert!(!out.ok, "stale edit was applied: {}", out.content);
    assert!(
        out.content.contains("changed") && out.content.contains("approval"),
        "{}",
        out.content
    );
    assert_eq!(env.tmp.read("a.txt"), "alpha\nCONCURRENT\nomega\n");
}

#[tokio::test]
async fn cancellation_during_approval_prevents_both_file_tools_from_writing() {
    let write_env = env("cancel-write-approval");
    let mut write_ctx = write_env.ctx.clone();
    let write_gate = Arc::new(CancelDuringApproval {
        token: write_ctx.cancel.clone(),
    });
    write_ctx = with_approval_gate(&write_env, write_gate);
    let written = call(
        &WriteFile,
        &write_ctx,
        json!({"path": "created.txt", "content": "must not land"}),
    )
    .await;
    assert!(
        !written.ok && written.content.contains("cancelled"),
        "{}",
        written.content
    );
    assert!(!write_env.tmp.exists("created.txt"));

    let edit_env = env("cancel-edit-approval");
    edit_env.tmp.write("a.txt", "before\n");
    let edit_gate = Arc::new(CancelDuringApproval {
        token: edit_env.ctx.cancel.clone(),
    });
    let edit_ctx = with_approval_gate(&edit_env, edit_gate);
    let edited = call(
        &EditFile,
        &edit_ctx,
        json!({"path": "a.txt", "old_string": "before", "new_string": "after"}),
    )
    .await;
    assert!(
        !edited.ok && edited.content.contains("cancelled"),
        "{}",
        edited.content
    );
    assert_eq!(edit_env.tmp.read("a.txt"), "before\n");
}

// ------------------------------------------------------- rewind journal ---
//
// The journal is what makes a rewind able to put the workspace back, so these
// tests assert the *shape* of what the file tools record: the path, which tool
// asked, and - the part that matters - that the captured bytes are the ones the
// file had *before* the write, not after it.

/// An environment whose file tools have a journal attached.
fn env_with_journal(tag: &str) -> (TestEnv, Arc<RecordingJournal>) {
    let mut env = env(tag);
    let journal = RecordingJournal::new(env.tmp.path().to_path_buf());
    env.ctx.checkpoints = Some(journal.clone());
    (env, journal)
}

#[cfg(unix)]
#[tokio::test]
async fn unix_backslash_paths_stay_literal_in_results_and_checkpoints() {
    let (env, journal) = env_with_journal("literal-backslash-journal");
    let path = r"dir\file.txt";
    env.tmp.write(path, "before\n");
    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": path, "old_string": "before", "new_string": "after"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(env.tmp.read(path), "after\n");
    assert_eq!(out.meta.as_ref().unwrap()["path"], json!(path));
    let captures = journal.captured();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].path, path);
    assert_eq!(captures[0].before, "before\n");
}

#[tokio::test]
async fn a_failed_checkpoint_refuses_write_and_edit_without_mutating_files() {
    let write_env = env("checkpoint-write-fails");
    write_env.tmp.write("a.txt", "write before\n");
    let mut write_ctx = write_env.ctx.clone();
    write_ctx.checkpoints = Some(Arc::new(RecordingJournal {
        root: write_env.tmp.path().to_path_buf(),
        captures: Mutex::new(Vec::new()),
        succeed: false,
    }));
    let write = call(
        &WriteFile,
        &write_ctx,
        json!({"path": "a.txt", "content": "write after\n"}),
    )
    .await;
    assert!(
        !write.ok && write.content.contains("checkpoint") && write.content.contains("unchanged"),
        "{}",
        write.content
    );
    assert_eq!(write_env.tmp.read("a.txt"), "write before\n");

    let edit_env = env("checkpoint-edit-fails");
    edit_env.tmp.write("a.txt", "edit before\n");
    let mut edit_ctx = edit_env.ctx.clone();
    edit_ctx.checkpoints = Some(Arc::new(RecordingJournal {
        root: edit_env.tmp.path().to_path_buf(),
        captures: Mutex::new(Vec::new()),
        succeed: false,
    }));
    let edit = call(
        &EditFile,
        &edit_ctx,
        json!({"path": "a.txt", "old_string": "edit before", "new_string": "edit after"}),
    )
    .await;
    assert!(
        !edit.ok && edit.content.contains("checkpoint") && edit.content.contains("unchanged"),
        "{}",
        edit.content
    );
    assert_eq!(edit_env.tmp.read("a.txt"), "edit before\n");
}

#[tokio::test]
async fn write_file_records_the_bytes_it_overwrites() {
    let (env, journal) = env_with_journal("journal-write-overwrite");
    env.tmp.write("src/lib.rs", "fn before() {}\n");

    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "src/lib.rs", "content": "fn after() {}\n"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(env.tmp.read("src/lib.rs"), "fn after() {}\n");

    let captures = journal.captured();
    assert_eq!(captures.len(), 1, "one record per file operation");
    assert_eq!(captures[0].path, "src/lib.rs");
    assert_eq!(captures[0].tool, "write_file");
    assert_eq!(
        captures[0].before, "fn before() {}\n",
        "the journal must hold the before-image, not the bytes just written"
    );
}

#[tokio::test]
async fn write_file_records_a_creation_as_an_empty_before_image() {
    let (env, journal) = env_with_journal("journal-write-create");

    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "brand/new.txt", "content": "hello\n"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);

    let captures = journal.captured();
    assert_eq!(captures.len(), 1, "a create is undoable, so it is recorded");
    assert_eq!(captures[0].path, "brand/new.txt");
    assert_eq!(
        captures[0].before, "",
        "there was no file, which is what the undo restores"
    );
}

#[tokio::test]
async fn edit_file_records_the_pre_edit_content() {
    let (env, journal) = env_with_journal("journal-edit");
    env.tmp.write("a.rs", "let x = 1;\n");

    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "a.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(env.tmp.read("a.rs"), "let x = 2;\n");

    let captures = journal.captured();
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0].tool, "edit_file");
    assert_eq!(
        captures[0].before, "let x = 1;\n",
        "an undo of the edit must restore the pre-edit bytes"
    );
}

#[tokio::test]
async fn a_refused_write_records_nothing() {
    let (mut env, journal) = env_with_journal("journal-denied");
    env.ctx = ToolCtx {
        approvals: ApprovalHandle::new(
            Arc::new(RecordingGate {
                decision: ApprovalDecision::Deny,
                requests: Mutex::new(Vec::new()),
            }),
            SessionId::new(),
            RunId::new(),
            None,
        ),
        ..env.ctx.clone()
    };

    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "blocked.txt", "content": "x"}),
    )
    .await;
    assert!(!out.ok);
    assert!(
        journal.captured().is_empty(),
        "a denied write changed nothing, so there is nothing to undo"
    );
    assert!(!env.tmp.exists("blocked.txt"));
}

#[tokio::test]
async fn a_write_without_a_journal_still_works() {
    // A tool invoked directly (a test harness, an embedding) has no journal; the
    // write must not start failing because a rewind could not be recorded.
    let env = env("journal-absent");
    assert!(env.ctx.checkpoints.is_none());
    let out = call(
        &WriteFile,
        &env.ctx,
        json!({"path": "plain.txt", "content": "ok"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(env.tmp.read("plain.txt"), "ok");
}

#[tokio::test]
async fn read_only_tools_do_not_touch_the_journal() {
    let (env, journal) = env_with_journal("journal-readonly");
    env.tmp.write("a.txt", "body\n");

    assert!(call(&ReadFile, &env.ctx, json!({"path": "a.txt"})).await.ok);
    assert!(call(&ListDir, &env.ctx, json!({"path": "."})).await.ok);
    assert!(
        call(
            &SearchFiles,
            &env.ctx,
            json!({"pattern": "body", "path": "."})
        )
        .await
        .ok
    );

    assert!(
        journal.captured().is_empty(),
        "reading a file never changes it, so it is never journaled"
    );
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

/// FIX 3: a command that backgrounds a child and exits 0 must be reported as a
/// success. The child inherits the output pipes, so waiting for EOF turned a
/// command that finished in milliseconds into `ok: false, timed_out: true` once
/// the child outlived the timeout.
#[tokio::test]
async fn a_backgrounded_child_does_not_turn_success_into_a_timeout() {
    let limits = Limits {
        max_command_timeout_secs: 30,
        ..Default::default()
    };
    let env = env_with("cmd-background", ask(), limits, ApprovalDecision::AllowOnce);
    // Exits immediately; `sleep 3` keeps the stdout/stderr pipes open for 3 s.
    let started = Instant::now();
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "sleep 3 & echo started; exit 0", "timeout_secs": 2}),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(out.ok, "{}", out.content);
    let meta = out.meta.clone().unwrap();
    assert_eq!(
        meta["timed_out"],
        json!(false),
        "a successful command must not be reported as a timeout just because a \
         background child holds the pipes: {}",
        out.content
    );
    assert_eq!(meta["exit_code"], json!(0));
    assert_eq!(meta["background_output"], json!(true));
    assert!(
        out.content.contains("started"),
        "the output written before the leader exited must be kept: {}",
        out.content
    );
    assert!(
        out.content.contains("background"),
        "the result must say the output may still be being produced: {}",
        out.content
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "the call must not wait for the background child: {elapsed:?}"
    );
}

/// The doc comment on `GroupGuard` states the guarantee the drain grace must not
/// weaken: "a surviving grandchild cannot keep the pipes open past this call". The
/// success path now returns while a background child still holds the pipes, so
/// prove the guard still kills it instead of leaving an orphan behind.
#[tokio::test]
async fn a_backgrounded_child_does_not_outlive_the_call() {
    let env = env("cmd-background-reaped");
    let pid_file = env.tmp.path().join("bg.pid");
    let started = Instant::now();
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({
            "command": format!("sleep 30 & echo $! > '{}'; echo started; exit 0", pid_file.display()),
            "timeout_secs": 60,
        }),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.meta.clone().unwrap()["timed_out"], json!(false));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the call must return promptly, took {:?}",
        started.elapsed()
    );

    let pid: u32 = std::fs::read_to_string(&pid_file)
        .expect("the child should have written its pid")
        .trim()
        .parse()
        .expect("pid");
    // The group guard is what stops it; give the signal a moment to land.
    let mut gone = false;
    for _ in 0..100 {
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        gone,
        "background child {pid} survived the call: the group guard must still kill \
         what the pipe drain left behind"
    );
}

/// The counterpart: a *foreground* command that hangs is still a timeout, and the
/// group is still killed.
#[tokio::test]
async fn a_hanging_foreground_command_still_times_out() {
    let limits = Limits {
        max_command_timeout_secs: 30,
        ..Default::default()
    };
    let env = env_with("cmd-hang", ask(), limits, ApprovalDecision::AllowOnce);
    let started = Instant::now();
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "sleep 30", "timeout_secs": 1}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
    assert!(out.content.contains("timed out"), "{}", out.content);
    let meta = out.meta.unwrap();
    assert_eq!(meta["timed_out"], json!(true));
    assert_eq!(meta["background_output"], json!(false));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "a timed-out command must be killed promptly, took {:?}",
        started.elapsed()
    );
}

/// FIX 4: stderr used to vanish whenever stdout was long. Both streams were
/// concatenated into one string and truncated from the front, so a 31 KB stdout
/// pushed every stderr line past the cap (reproduced at the defaults: 50 stderr
/// lines, all absent). Each stream now has its own budget.
#[tokio::test]
async fn a_long_stdout_does_not_evict_stderr() {
    let env = env("cmd-stderr-survives");
    // ~31 KB of stdout, then a diagnostic on stderr.
    let out = call(
        &RunCommand,
        &env.ctx,
        // The sentinel is assembled from two printf arguments so the literal
        // does not also appear in the echoed `$ command` line of the header -
        // otherwise this assertion would pass on the broken code too.
        json!({"command": "seq 1 8000; printf 'STDERR-%s-42\\n' SENTINEL 1>&2"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.meta.as_ref().unwrap()["stdout_bytes"].as_u64().unwrap() > 30_000,
        "the fixture must exceed the 24 KB tool-output cap: {:?}",
        out.meta
    );
    assert!(
        out.content.contains("--- stderr"),
        "the stderr section must survive a long stdout:\n{}",
        out.content
    );
    assert!(
        out.content.contains("STDERR-SENTINEL-42"),
        "stderr must survive a long stdout:\n{}",
        out.content
    );
    // The truncation stays visible to the model.
    assert!(
        out.content.contains("bytes omitted"),
        "a truncated stream must still say so:\n{}",
        out.content
    );
    assert!(out.truncated);
}

#[tokio::test]
async fn command_result_retains_raw_stream_ends_inside_the_final_runtime_budget() {
    let limits = Limits {
        max_tool_output_bytes: 800,
        max_command_output_bytes: 1024,
        ..Default::default()
    };
    let env = env_with(
        "cmd-final-budget",
        ask(),
        limits,
        ApprovalDecision::AllowOnce,
    );
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "{ printf 'STDOUT-%s\\n' HEAD; seq 1 1000; printf 'STDOUT-%s\\n' TAIL; }; { seq 1 150 >&2; printf 'STDERR-%s\\n' TRAILING >&2; }"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.len() <= 800, "{} bytes", out.content.len());
    for sentinel in ["STDOUT-HEAD", "STDOUT-TAIL", "STDERR-TRAILING"] {
        assert!(
            out.content.contains(sentinel),
            "missing {sentinel}:\n{}",
            out.content
        );
    }
    assert!(
        out.content.ends_with("STDERR-TRAILING\n"),
        "{}",
        out.content
    );
    assert_eq!(
        out.content.matches("[truncated:").count(),
        0,
        "{}",
        out.content
    );

    // At the configured cap the runtime's own limiter is a no-op. A still smaller
    // final per-turn budget preserves the trailing error and adds one marker that
    // is itself inside that smaller cap.
    let (same, cut) = kc_core::tool::truncate_output(&out.content, 800);
    assert!(!cut && same == out.content);
    let (final_output, cut) = kc_core::tool::truncate_output(&out.content, 500);
    assert!(cut);
    assert!(final_output.len() <= 500);
    assert!(final_output.contains("STDERR-TRAILING"), "{}", final_output);
    assert_eq!(
        final_output.matches("[truncated:").count(),
        1,
        "{}",
        final_output
    );
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

#[cfg(unix)]
#[tokio::test]
async fn narrowed_git_status_and_diff_include_a_deleted_tracked_destination() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    let env = env("git-deleted-target");
    env.tmp.write("subtree/gone.txt", "deleted sentinel\n");
    env.tmp.write("other.txt", "one\n");
    git(env.tmp.path(), &["init", "-q", "-b", "main"]);
    git(env.tmp.path(), &["add", "."]);
    git(env.tmp.path(), &["commit", "-qm", "init"]);
    std::fs::remove_dir_all(env.tmp.path().join("subtree")).unwrap();
    env.tmp.write("other.txt", "unrelated change\n");

    let status = call(&GitStatus, &env.ctx, json!({"path": "subtree/gone.txt"})).await;
    assert!(status.ok, "{}", status.content);
    assert!(
        status.content.contains("D subtree/gone.txt"),
        "{}",
        status.content
    );
    assert!(!status.content.contains("other.txt"), "{}", status.content);

    let patch = call(&GitDiff, &env.ctx, json!({"path": "subtree/gone.txt"})).await;
    assert!(patch.ok, "{}", patch.content);
    assert!(
        patch.content.contains("deleted sentinel"),
        "{}",
        patch.content
    );
    assert!(
        !patch.content.contains("unrelated change"),
        "{}",
        patch.content
    );

    // Missing leaves are allowed only after existing-ancestor confinement. A
    // symlinked parent outside the workspace must still be rejected.
    let outside = TempWorkspace::new("git-deleted-outside");
    std::os::unix::fs::symlink(outside.path(), env.tmp.path().join("escape")).unwrap();
    let refused = call(&GitDiff, &env.ctx, json!({"path": "escape/missing.txt"})).await;
    assert!(!refused.ok, "{}", refused.content);
}

/// FIX 1: the workspace is a *subdirectory* of a bigger repository. git walks up
/// to find the repository, so the tools used to report that repository: a status
/// listing sibling paths outside the workspace and a diff printing their
/// contents - with no approval, since both tools are read-only.
#[tokio::test]
async fn git_tools_do_not_report_the_enclosing_repository() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    // `<outer>/` is the repository; `<outer>/inner/` is the workspace.
    let outer = TempWorkspace::new("git-nested");
    git(outer.path(), &["init", "-q", "-b", "main"]);
    outer.write("sibling.txt", "SIBLING-SECRET one\n");
    outer.write("inner/tracked.txt", "inner one\n");
    git(outer.path(), &["add", "."]);
    git(outer.path(), &["commit", "-qm", "init"]);
    // A change *outside* the workspace and one inside it.
    outer.write("sibling.txt", "SIBLING-SECRET two\n");
    outer.write("inner/tracked.txt", "inner two\n");

    let inner_root = outer.path().join("inner");
    let ws = Workspace::new(&inner_root).unwrap();
    let ctx = ctx_for(&ws);

    let status = call(&GitStatus, &ctx, json!({})).await;
    assert!(status.ok, "{}", status.content);
    assert!(
        !status.content.contains("sibling"),
        "git_status must not name a path outside the workspace:\n{}",
        status.content
    );
    assert!(
        status.content.contains("tracked.txt"),
        "the workspace's own change must still be reported:\n{}",
        status.content
    );
    assert_eq!(status.meta.unwrap()["confined"], json!(true));

    let patch = call(&GitDiff, &ctx, json!({})).await;
    assert!(patch.ok, "{}", patch.content);
    assert!(
        !patch.content.contains("SIBLING-SECRET") && !patch.content.contains("sibling"),
        "git_diff must not print the enclosing repository's diff:\n{}",
        patch.content
    );
    assert!(
        patch.content.contains("+inner two"),
        "the workspace's own change must still be shown:\n{}",
        patch.content
    );
    // The paths in the patch are workspace-relative, so the workspace name is
    // not a directory component either.
    assert!(
        patch.content.contains("a/tracked.txt"),
        "patch paths must be workspace-relative:\n{}",
        patch.content
    );

    // A path argument still works, and still cannot reach outside.
    let one = call(&GitDiff, &ctx, json!({"path": "tracked.txt"})).await;
    assert!(one.ok, "{}", one.content);
    assert!(one.content.contains("+inner two"), "{}", one.content);
    let refused = call(&GitDiff, &ctx, json!({"path": "../sibling.txt"})).await;
    assert!(!refused.ok, "{}", refused.content);
}

/// The single-repo case must be untouched: no confinement note, and the same
/// paths and patch headers as before.
#[tokio::test]
async fn git_tools_in_a_plain_repo_are_unchanged() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    let env = env("git-plain");
    env.tmp.write("a.txt", "one\n");
    git(env.tmp.path(), &["init", "-q", "-b", "main"]);
    git(env.tmp.path(), &["add", "."]);
    git(env.tmp.path(), &["commit", "-qm", "init"]);
    env.tmp.write("a.txt", "two\n");

    let status = call(&GitStatus, &env.ctx, json!({})).await;
    assert!(status.ok, "{}", status.content);
    assert_eq!(status.meta.unwrap()["confined"], json!(false));
    assert!(
        !status.content.contains("confined"),
        "a plain repository needs no confinement note:\n{}",
        status.content
    );
    assert!(status.content.contains(" M a.txt"), "{}", status.content);

    let patch = call(&GitDiff, &env.ctx, json!({})).await;
    assert!(patch.ok, "{}", patch.content);
    assert!(patch.content.contains("a/a.txt"), "{}", patch.content);
    assert!(patch.content.contains("+two"), "{}", patch.content);
    assert!(
        !patch.content.contains("confined"),
        "a plain repository needs no confinement note:\n{}",
        patch.content
    );
}

// --------------------------------------------------------------- registry ---

#[tokio::test]
async fn builtin_tools_are_complete_and_read_only_by_default() {
    let tools = builtin_tools();
    assert_eq!(tools.len(), 9);
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
            "git_diff",
            "karucode_docs"
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
    assert_eq!(get("karucode_docs"), kc_core::tool::ToolClass::ReadOnly);
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
