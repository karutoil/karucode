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

/// The recovery step has to live in the tool's own description: that is where an
/// agent looks when it is already inside the tool, and it has to be *after* the
/// first sentence, because the first sentence is all the system prompt shows.
#[test]
fn tool_descriptions_teach_the_recovery_after_the_first_sentence() {
    let cases: [(&str, String, &str); 4] = [
        (
            "read_file",
            ReadFile.spec().description,
            "continue with offset=X",
        ),
        (
            "search_files",
            SearchFiles.spec().description,
            "Narrow before you widen",
        ),
        ("list_dir", ListDir.spec().description, "narrow `path`"),
        (
            "glob",
            Glob.spec().description,
            "narrow `path` or `pattern`",
        ),
    ];
    for (name, description, needle) in cases {
        let first_sentence_end = description
            .find(". ")
            .map(|index| index + 2)
            .unwrap_or(description.len());
        assert!(
            description[first_sentence_end..].contains(needle),
            "{name}'s description must teach the recovery step after the first sentence: \
             {description}"
        );
        assert!(
            !description[..first_sentence_end].contains(needle),
            "{name}'s recovery step must not eat the line the prompt shows: {description}"
        );
    }
}

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

/// The measured failure: a read whose content exceeds `limits.max_tool_output_bytes`
/// used to be spliced head+tail, its `lines A-B` header naming a range it did not
/// carry and no offset that resumes it. Now the read returns whole numbered lines
/// and names the exact `offset=` that continues them - and asking for that offset
/// really returns the next line of the file.
#[tokio::test]
async fn a_read_cut_by_the_output_budget_names_an_offset_that_resumes_it() {
    let limits = Limits {
        max_tool_output_bytes: 1_200,
        ..Default::default()
    };
    let env = env_with("read-budget", ask(), limits, ApprovalDecision::AllowOnce);
    let body: String = (1..=300)
        .map(|i| format!("line {i} has enough padding to make the byte budget bite\n"))
        .collect();
    env.tmp.write("big.txt", &body);

    let out = call(&ReadFile, &env.ctx, json!({"path": "big.txt"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.truncated);
    assert!(
        out.content.len() <= 1_200,
        "the call's own budget must hold: {} bytes\n{}",
        out.content.len(),
        out.content
    );
    // One truncation marker at most: the read fits itself, so the generic
    // head+tail marker must not make a second appearance.
    assert!(
        !out.content.contains("[truncated:"),
        "a budgeted read needs no head+tail splice:\n{}",
        out.content
    );
    let offset = resume_offset(&out.content).unwrap_or_else(|| {
        panic!(
            "the result must name the call that continues it:\n{}",
            out.content
        )
    });
    assert!(offset > 1, "no lines were shown: {}", out.content);

    // Nothing was shown twice and nothing was hidden: the last numbered line is
    // exactly the line before the reported resume offset.
    let shown = numbered_lines(&out.content);
    assert_eq!(*shown.last().unwrap(), offset - 1, "{}", out.content);
    assert_eq!(shown.first(), Some(&1), "{}", out.content);

    // The header must describe what was really returned, not what was requested.
    let header = out.content.lines().next().unwrap();
    assert!(
        header.contains(&format!("lines 1-{} of 300", offset - 1)),
        "header lies about the returned range: {header}"
    );

    // The reported offset lands inside the unread region and returns it.
    let next = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "big.txt", "offset": offset}),
    )
    .await;
    assert!(next.ok, "{}", next.content);
    let file_lines: Vec<&str> = body.lines().collect();
    assert!(
        next.content
            .contains(&format!("{:>5}\t{}", offset, file_lines[offset - 1])),
        "offset={offset} must resume at line {offset}:\n{}",
        next.content
    );
    assert_eq!(numbered_lines(&next.content).first(), Some(&offset));

    // The meta agrees with the text, so an agent reading either sees the same range.
    let meta = out.meta.unwrap();
    assert_eq!(meta["lines_from"], json!(1));
    assert_eq!(meta["lines_to"], json!(offset - 1));
}

/// A single line longer than the whole output budget: the head+tail splice used to
/// cut it mid-line with no way forward. The result now says which line it is and
/// what to use instead, because no `offset` reaches the rest of one line.
#[tokio::test]
async fn a_read_of_one_oversized_line_says_what_can_reach_it() {
    let limits = Limits {
        max_tool_output_bytes: 900,
        ..Default::default()
    };
    let env = env_with("read-long-line", ask(), limits, ApprovalDecision::AllowOnce);
    let long = format!("MINIFIED={}", "a".repeat(20_000));
    env.tmp.write(
        "bundle.js",
        &format!("short first line\n{long}\nshort last line\n"),
    );

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "bundle.js", "offset": 2, "limit": 1}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.len() <= 900, "{} bytes", out.content.len());
    assert!(
        out.content
            .contains(&format!("line 2 is {} bytes", long.len())),
        "the note must name the offending line: {}",
        out.content
    );
    assert!(
        out.content.contains("search_files"),
        "and the tool that can reach into it: {}",
        out.content
    );
    assert!(
        out.content.contains("continue with offset=3"),
        "the next whole line is still reachable: {}",
        out.content
    );
}

/// Batched reads: every section is a whole-line window with its own resumable
/// offset, instead of one head+tail splice over the concatenation.
#[tokio::test]
async fn a_batched_read_keeps_every_section_whole_and_resumable() {
    let limits = Limits {
        max_tool_output_bytes: 3_000,
        ..Default::default()
    };
    let env = env_with(
        "read-batch-budget",
        ask(),
        limits,
        ApprovalDecision::AllowOnce,
    );
    for name in ["a.txt", "b.txt"] {
        let body: String = (1..=200)
            .map(|i| format!("{name} row {i} padded padded padded padded\n"))
            .collect();
        env.tmp.write(name, &body);
    }

    let out = call(&ReadFile, &env.ctx, json!({"paths": ["a.txt", "b.txt"]})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.len() <= 3_000, "{} bytes", out.content.len());
    assert!(
        !out.content.contains("[truncated:"),
        "each section fits its share:\n{}",
        out.content
    );
    // Each section keeps its own "continue with offset" - a batch is not one
    // anonymous splice.
    assert_eq!(
        out.content.matches("continue with offset=").count(),
        2,
        "{}",
        out.content
    );
    for path in ["a.txt", "b.txt"] {
        let section = section_of(&out.content, path);
        let offset = resume_offset(section)
            .unwrap_or_else(|| panic!("{path} has no resume offset:\n{section}"));
        let next = call(&ReadFile, &env.ctx, json!({"path": path, "offset": offset})).await;
        assert!(next.ok, "{}", next.content);
        assert_eq!(
            numbered_lines(&next.content).first(),
            Some(&offset),
            "{} offset={offset}\n{}",
            path,
            next.content
        );
        // Whole lines only: every line in the section is a full file line.
        let body = env.tmp.read(path);
        for (number, text) in numbered_pairs(section) {
            assert_eq!(
                body.lines().nth(number - 1).unwrap(),
                text,
                "{path} line {number} was spliced"
            );
        }
    }
}

/// The `offset=` a `[next N lines omitted; continue with offset=X]` hint names.
fn resume_offset(text: &str) -> Option<usize> {
    let tail = text.rsplit("continue with offset=").next()?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// 1-based numbers of the `%5d\t` numbered lines in a rendered read.
fn numbered_lines(text: &str) -> Vec<usize> {
    numbered_pairs(text)
        .into_iter()
        .map(|(number, _)| number)
        .collect()
}

/// `(number, text)` for every numbered line in a rendered read.
fn numbered_pairs(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .filter_map(|line| {
            let (number, rest) = line.split_once('\t')?;
            let number: usize = number.trim().parse().ok()?;
            Some((number, rest.to_string()))
        })
        .collect()
}

/// The `--- path ---` section of a batched read.
fn section_of<'a>(text: &'a str, path: &str) -> &'a str {
    let start = text
        .find(&format!("--- {path} ---"))
        .unwrap_or_else(|| panic!("no section for {path}:\n{text}"));
    let rest = &text[start..];
    match rest[1..].find("--- ") {
        Some(next) => &rest[..next + 1],
        None => rest,
    }
}

/// The measured failure at the tool boundary: a file over
/// `limits.max_file_read_bytes` must still be readable past the byte cap, so the
/// model does not shell out to `sed -n` for one window.
/// Argument count is a cap too: more than 32 reads per call are not attempted,
/// and the result says how many were dropped rather than looking complete.
#[tokio::test]
async fn a_read_batch_over_the_call_cap_says_how_many_it_dropped() {
    let env = env("read-batch-dropped");
    for i in 0..40 {
        env.tmp
            .write(&format!("f{i:02}.txt"), &format!("body {i}\n"));
    }
    let paths: Vec<String> = (0..40).map(|i| format!("f{i:02}.txt")).collect();
    let out = call(&ReadFile, &env.ctx, json!({"paths": paths})).await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("8 more read(s) were not attempted"),
        "the drop must be stated: {}",
        &out.content[out.content.len().saturating_sub(300)..]
    );
    assert_eq!(out.meta.as_ref().unwrap()["dropped_count"], json!(8));
    assert!(out.truncated);
}

/// A `limit` beyond the hard cap is clamped, and the header shows the clamp so the
/// model is not left believing it received the 5 000 lines it asked for.
#[tokio::test]
async fn a_read_limit_over_the_hard_cap_is_visible_in_the_header() {
    // A budget large enough that the 2000-line hard cap, not the output budget,
    // is what ends the window.
    let limits = Limits {
        max_tool_output_bytes: 1_000_000,
        ..Default::default()
    };
    let env = env_with("read-limit-cap", ask(), limits, ApprovalDecision::AllowOnce);
    let body: String = (1..=2_500).map(|i| format!("row {i}\n")).collect();
    env.tmp.write("long.txt", &body);

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "long.txt", "limit": 5_000}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.starts_with("long.txt — lines 1-2000 of 2500"),
        "the header must report the clamped window: {}",
        out.content.lines().next().unwrap()
    );
    assert!(
        out.content.contains("continue with offset=2001"),
        "and the next call: {}",
        &out.content[out.content.len().saturating_sub(200)..]
    );
}

/// With default limits the output budget, not `limit`, ends a wide read - and the
/// header and hint describe the window that was really returned.
#[tokio::test]
async fn a_read_window_ends_on_a_line_the_header_and_the_hint_agree_on() {
    let env = env("read-window-agree");
    let body: String = (1..=2_500).map(|i| format!("row {i}\n")).collect();
    env.tmp.write("long.txt", &body);

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "long.txt", "limit": 5_000}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.len() <= 24_000, "{} bytes", out.content.len());
    let last = *numbered_lines(&out.content).last().unwrap();
    assert!(
        out.content
            .starts_with(&format!("long.txt — lines 1-{last} of 2500")),
        "header and body disagree: {}",
        out.content.lines().next().unwrap()
    );
    assert_eq!(resume_offset(&out.content), Some(last + 1));
}

#[tokio::test]
async fn read_file_reaches_a_line_past_the_byte_cap() {
    let env = env("read-past-cap");
    let lines = 30_000usize;
    let body: String = (1..=lines).map(|i| format!("row {i}\n")).collect();
    assert!(body.len() > 200_000);
    env.tmp.write("big.txt", &body);

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "big.txt", "offset": 28_000, "limit": 3}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content
            .starts_with("big.txt — lines 28000-28002 of 30000"),
        "{}",
        out.content
    );
    assert!(out.content.contains("28000\trow 28000"), "{}", out.content);
    assert!(!out.content.contains("raise limits.max_file_read_bytes"));
    let meta = out.meta.unwrap();
    assert_eq!(meta["lines_total"], json!(lines));
    assert_eq!(meta["byte_limited"], json!(false));
}

/// Several windows of one file in a single call: the shape the model used to
/// build with `sed -n '1,40p;800,840p'`.
#[tokio::test]
async fn read_file_reads_several_ranges_in_one_call() {
    let env = env("read-ranges");
    let body: String = (1..=100).map(|i| format!("line{i}\n")).collect();
    env.tmp.write("a.txt", &body);

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "a.txt", "ranges": [{"offset": 1, "limit": 2}, {"offset": 50, "limit": 1}]}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("--- a.txt ---"), "{}", out.content);
    assert!(out.content.contains("lines 1-2 of 100"), "{}", out.content);
    assert!(out.content.contains("    1\tline1"), "{}", out.content);
    assert!(
        out.content.contains("lines 50-50 of 100"),
        "{}",
        out.content
    );
    assert!(out.content.contains("   50\tline50"), "{}", out.content);
    assert!(!out.content.contains("line3"), "{}", out.content);
    let meta = out.meta.unwrap();
    assert_eq!(meta["read_count"], json!(2));
    assert_eq!(meta["failed_count"], json!(0));
    assert_eq!(meta["reads"][1]["lines_from"], json!(50));

    // A malformed range keeps its place and does not discard the good one.
    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "a.txt", "ranges": [{"offset": 5, "limit": 1}, {"offset": 9, "bogus": 1}, "nope"]}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("    5\tline5"), "{}", out.content);
    assert!(
        out.content.contains("[error:") && out.content.contains("unknown key `bogus`"),
        "{}",
        out.content
    );
    let meta = out.meta.unwrap();
    assert_eq!(meta["read_count"], json!(3));
    assert_eq!(meta["failed_count"], json!(2));
}

/// Several files in one call, where one path is bad: the good reads still come
/// back and the failure is reported in its own place.
#[tokio::test]
async fn read_file_reads_several_paths_and_reports_one_missing_inline() {
    let env = env("read-paths");
    env.tmp.write("a.txt", "alpha\nbeta\n");
    env.tmp.write("b.txt", "gamma\n");

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"paths": ["a.txt", "missing.txt", "b.txt"]}),
    )
    .await;
    assert!(
        out.ok,
        "one bad path must not discard the good ones: {}",
        out.content
    );
    assert!(out.content.contains("--- a.txt ---"), "{}", out.content);
    assert!(out.content.contains("    1\talpha"), "{}", out.content);
    assert!(out.content.contains("--- b.txt ---"), "{}", out.content);
    assert!(out.content.contains("    1\tgamma"), "{}", out.content);
    assert!(
        out.content.contains("--- missing.txt ---"),
        "the bad path keeps its place: {}",
        out.content
    );
    assert!(
        out.content.contains("[error:") && out.content.contains("does not exist"),
        "{}",
        out.content
    );
    let meta = out.meta.unwrap();
    assert_eq!(meta["read_count"], json!(3));
    assert_eq!(meta["failed_count"], json!(1));
    assert_eq!(meta["reads"][1]["path"], json!("missing.txt"));
    assert!(meta["reads"][1]["error"].is_string());

    // Every read failing is a failed call, not a success full of notes.
    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"paths": ["nope1.txt", "nope2.txt"]}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
}

/// A batch still obeys the per-call output cap: it degrades, it is not refused,
/// and every path is represented.
#[tokio::test]
async fn a_wide_batch_is_shaped_by_the_tool_output_cap() {
    let limits = Limits {
        max_tool_output_bytes: 4_000,
        ..Default::default()
    };
    let env = env_with("read-batch-cap", ask(), limits, ApprovalDecision::AllowOnce);
    let body: String = (1..=400).map(|i| format!("row {i}\n")).collect();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        env.tmp.write(name, &body);
    }

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"paths": ["a.txt", "b.txt", "c.txt", "d.txt"]}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.len() <= 4_000, "{} bytes", out.content.len());
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        assert!(
            out.content.contains(&format!("--- {name} ---")),
            "{name} must keep its section: {}",
            out.content
        );
    }
    assert!(out.truncated, "a capped batch reports truncation");
}

/// The top-level shapes cannot be mixed: that is a usage failure with a
/// copyable example, not a half-performed batch.
#[tokio::test]
async fn read_file_refuses_mixed_shapes() {
    let env = env("read-mixed");
    env.tmp.write("a.txt", "alpha\n");

    let out = call(
        &ReadFile,
        &env.ctx,
        json!({"path": "a.txt", "paths": ["a.txt"]}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
    assert!(out.content.contains("not both"), "{}", out.content);

    let out = call(&ReadFile, &env.ctx, json!({"ranges": [{"offset": 1}]})).await;
    assert!(!out.ok, "{}", out.content);
    assert!(
        out.content.contains("`ranges` applies to one file"),
        "{}",
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
        out.content.contains("read_file needs `path`"),
        "the refusal must name the accepted shapes: {}",
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

/// A listing bigger than the call's output budget used to be head+tail spliced
/// while its header still claimed to show every entry. The budget is now applied
/// to the entries themselves: every row is a whole entry, the count is the real
/// number shown, and the note says how to narrow.
#[tokio::test]
async fn a_listing_cut_by_the_output_budget_reports_what_it_really_showed() {
    let limits = Limits {
        max_tool_output_bytes: 2_000,
        ..Default::default()
    };
    let env = env_with("list-budget", ask(), limits, ApprovalDecision::AllowOnce);
    for i in 0..300 {
        env.tmp
            .write(&format!("a-rather-long-file-name-{i:03}.txt"), "x");
    }

    let out = call(&ListDir, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.truncated);
    assert!(out.content.len() <= 2_000, "{} bytes", out.content.len());
    assert!(
        !out.content.contains("[truncated: 2"),
        "the listing must fit itself rather than be spliced:\n{}",
        out.content
    );
    let header = out.content.lines().next().unwrap();
    let shown: usize = header
        .rsplit(", ")
        .next()
        .and_then(|tail| tail.trim_end_matches(" shown)").parse().ok())
        .unwrap_or_else(|| panic!("no shown count in header: {header}"));
    let rows: Vec<&str> = out
        .content
        .lines()
        .skip(1)
        .filter(|line| !line.starts_with('['))
        .collect();
    assert_eq!(
        rows.len(),
        shown,
        "the header must report the rows that are really there:\n{}",
        out.content
    );
    assert!(shown < 300, "{}", out.content);
    // Whole rows only: no row is a fragment of an entry.
    for row in &rows {
        assert!(
            row.starts_with("a-rather-long-file-name-") && row.ends_with(".txt (1 B)"),
            "a spliced row reached the model: {row:?}"
        );
    }
    let meta = out.meta.unwrap();
    assert_eq!(meta["shown"], json!(shown));
    assert!(
        out.content.contains("narrow `path` or lower `depth`"),
        "the note must name the narrowing: {}",
        out.content
    );
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
async fn cancelling_a_running_command_kills_its_process_group() {
    // The pre-cancelled case proves a stop *before* the spawn starts nothing; this
    // one proves a stop *during* the run reaches the whole group. The shell starts a
    // background child that would write a file a second later: if the group outlived
    // the stop, that file appears after the call has already returned.
    let env = env("cmd-cancel-live");
    // Non-vacuity: prove the marker file would be visible where the assertion looks
    // if the child lived long enough to write it.
    let proof = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "echo alive > survived.txt"}),
    )
    .await;
    assert!(proof.ok, "{}", proof.content);
    assert!(
        env.tmp.exists("survived.txt"),
        "the fixture writes into the workspace"
    );
    std::fs::remove_file(env.tmp.path().join("survived.txt")).unwrap();
    let ctx = env.ctx.clone();
    let token = ctx.cancel.clone();
    let running = tokio::spawn(async move {
        call(
            &RunCommand,
            &ctx,
            json!({"command": "(sleep 1; echo alive > survived.txt) & sleep 30"}),
        )
        .await
    });

    // Let the shell start and its background child arm.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(env.tmp.path().exists(), "the fixture workspace exists");
    token.cancel();

    let out = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("a cancelled command must not keep the run waiting")
        .expect("the call task joins");
    assert!(
        !out.ok,
        "a cancelled command is a failed result: {}",
        out.content
    );
    assert!(
        out.content.contains("cancel"),
        "the result says it was cancelled: {}",
        out.content
    );

    // Past the moment the child needed: an alive group would have written the file.
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert!(
        !env.tmp.exists("survived.txt"),
        "the command's process group outlived the stop"
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

/// A command whose output exceeds `limits.max_command_output_bytes` used to say
/// only "N of M bytes omitted". The model could not tell whether to re-run, how,
/// or which key to name for the user. The marker now carries all three, and the
/// trailing stderr line still survives at default limits.
#[tokio::test]
async fn a_noisy_command_names_the_knob_and_the_way_to_read_the_rest() {
    // Default limits: 32 000 bytes captured per stream, 24 000 kept for the model.
    let env = env_with(
        "cmd-knob",
        ask(),
        Limits::default(),
        ApprovalDecision::AllowOnce,
    );
    let out = call(
        &RunCommand,
        &env.ctx,
        json!({"command": "seq 1 40000; printf 'STDERR-TRAILING\\n' >&2"}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.len() <= 24_000, "{} bytes", out.content.len());
    assert!(
        out.content.contains("STDERR-TRAILING"),
        "the trailing error must survive a noisy stdout:\n{}",
        out.content
    );
    assert!(
        out.content.contains("limits.max_command_output_bytes"),
        "the marker must name the key that bounds it:\n{}",
        out.content
    );
    assert!(
        out.content.contains("> out.txt 2>&1"),
        "the marker must give the recovery that works for a pipe:\n{}",
        out.content
    );
    assert!(
        out.content.contains("read_file"),
        "and the tool that pages the redirected file:\n{}",
        out.content
    );
    assert!(
        out.content.contains("bytes omitted"),
        "the amount dropped is stated:\n{}",
        out.content
    );
    // The recovery must also survive a *later* runtime-budget cut, which keeps the
    // head and the tail and drops the middle: the capture marker sits in the
    // middle, so the same sentence is repeated near the top.
    let head = &out.content[..out.content.len().min(1_200)];
    assert!(
        head.contains("cmd > out.txt 2>&1") && head.contains("read_file"),
        "the recovery must be in the head, where a budget cut keeps it:\n{head}"
    );
    // The header still reports the real size of each stream.
    assert!(
        out.content.contains("--- stdout (") && out.content.contains("--- stderr ("),
        "{}",
        out.content
    );
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

/// A result list bigger than the call's output budget used to be spliced
/// head+tail, so the model saw the first matches and the last matches with no
/// statement of how many were in between. The list is now rendered within the
/// budget and the note says how many of how many, and how to see the rest.
#[tokio::test]
async fn a_search_cut_by_the_output_budget_says_how_many_it_showed() {
    let limits = Limits {
        max_tool_output_bytes: 2_000,
        ..Default::default()
    };
    let env = env_with("search-budget", ask(), limits, ApprovalDecision::AllowOnce);
    // More files than `limits.search_max_results` (200), so both the match cap
    // and the output budget are exercised.
    for i in 0..250 {
        env.tmp
            .write(&format!("mod{i:03}.rs"), &format!("needle in module {i}\n"));
    }

    let out = call(&SearchFiles, &env.ctx, json!({"pattern": "needle"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.truncated);
    assert!(out.content.len() <= 2_000, "{} bytes", out.content.len());
    let note_start = out
        .content
        .find("[showing the first ")
        .unwrap_or_else(|| panic!("no shown-count note:\n{}", out.content));
    let note: &str = &out.content[note_start..];
    let note_end = note.find(']').unwrap();
    let note = &note[..=note_end];
    let shown: usize = note
        .trim_start_matches("[showing the first ")
        .split(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no count in {note}"));
    let body = &out.content[..note_start];
    let rows: Vec<&str> = body
        .lines()
        .skip(1)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(
        rows.len(),
        shown,
        "the note must report the rows that are really there:\n{}",
        out.content
    );
    for row in &rows {
        assert!(
            row.starts_with("mod") && row.contains(".rs:1: needle in module "),
            "a spliced match line reached the model: {row:?}"
        );
    }
    assert!(
        note.contains("limits.max_tool_output_bytes"),
        "the note must name the key: {note}"
    );
    assert!(
        out.content.contains("hit the 200-match cap"),
        "the match cap is still reported: {}",
        out.content
    );
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

/// The arguments the shell was being used for: `grep -C` context, `grep -l`
/// file lists, and a per-file cap so one noisy file cannot spend the budget.
#[tokio::test]
async fn search_files_context_files_only_and_per_file_cap() {
    let env = env("search-shapes");
    env.tmp.write(
        "a.txt",
        "line1\nMATCH one\nline3\nline4\nline5\nMATCH two\nline7\n",
    );
    env.tmp.write("b.txt", "MATCH b\n");
    env.tmp.write("clean.txt", "nothing\n");

    // Context is marked with `-` so a context line is not read as the match.
    let out = call(
        &SearchFiles,
        &env.ctx,
        json!({"pattern": "MATCH", "path": "a.txt", "context": 1}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("2 match(es)"), "{}", out.content);
    assert!(
        out.content.contains("1 line(s) of context"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("a.txt:2: MATCH one"),
        "{}",
        out.content
    );
    assert!(out.content.contains("a.txt:1- line1"), "{}", out.content);
    assert!(out.content.contains("a.txt:3- line3"), "{}", out.content);
    let meta = out.meta.unwrap();
    assert_eq!(meta["matches"], json!(2), "context lines are not matches");
    assert_eq!(meta["context"], json!(1));

    // files_only: one path per line, no line numbers, and no matches on `:`.
    let out = call(
        &SearchFiles,
        &env.ctx,
        json!({"pattern": "MATCH", "files_only": true}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("2 file(s)"), "{}", out.content);
    let body: Vec<&str> = out
        .content
        .lines()
        .skip(2)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(body.len(), 2, "{}", out.content);
    for line in &body {
        assert!(!line.contains(':'), "a files-only body is paths: {line}");
    }
    assert!(body.contains(&"a.txt"), "{:?}", body);
    assert!(body.contains(&"b.txt"), "{:?}", body);
    assert_eq!(out.meta.unwrap()["files"], json!(2));

    // A per-file cap keeps the second file reachable with a small global cap.
    let out = call(
        &SearchFiles,
        &env.ctx,
        json!({"pattern": "MATCH", "max_results": 3, "max_matches_per_file": 1}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    assert!(
        out.content.contains("max 1 match(es) per file"),
        "{}",
        out.content
    );
    assert!(out.content.contains("a.txt:2:"), "{}", out.content);
    assert!(
        out.content.contains("b.txt:1:"),
        "the quiet file must still appear: {}",
        out.content
    );
    assert!(!out.content.contains("a.txt:6:"), "{}", out.content);
    assert_eq!(out.meta.as_ref().unwrap()["max_matches_per_file"], json!(1));
    assert_eq!(out.meta.unwrap()["matches"], json!(2));
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
        context: 0,
        files_only: false,
        max_matches_per_file: 0,
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
        context: 0,
        files_only: false,
        max_matches_per_file: 0,
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
        context: 0,
        files_only: false,
        max_matches_per_file: 0,
    };
    let err = search::search_fallback(&env.ws, &query).await.unwrap_err();
    assert!(err.to_string().contains("invalid regex"), "{err}");
}

// --------------------------------------------------------------------- glob ---

#[tokio::test]
async fn glob_finds_names_honours_gitignore_and_marks_directories() {
    let env = env("glob-basic");
    env.tmp.write("src/a.rs", "fn a() {}\n");
    env.tmp.write("src/nested/b.rs", "fn b() {}\n");
    env.tmp.write(".gitignore", "ignored/\n");
    env.tmp.write("ignored/c.rs", "fn c() {}\n");

    let out = call(&Glob, &env.ctx, json!({"pattern": "*.rs"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("src/a.rs"), "{}", out.content);
    assert!(out.content.contains("src/nested/b.rs"), "{}", out.content);
    assert!(
        !out.content.contains("ignored/"),
        "the walk must honour .gitignore: {}",
        out.content
    );
    assert_eq!(out.meta.as_ref().unwrap()["matches"], json!(2));

    // A path-shaped pattern is matched as well as a bare name.
    let out = call(&Glob, &env.ctx, json!({"pattern": "src/**/*.rs"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("src/nested/b.rs"), "{}", out.content);

    // Directories carry a trailing "/".
    let out = call(&Glob, &env.ctx, json!({"pattern": "nested"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("src/nested/"), "{}", out.content);

    // The search root can be narrowed.
    let out = call(&Glob, &env.ctx, json!({"pattern": "*.rs", "path": "src"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("src/a.rs"), "{}", out.content);

    // A file is not a directory to walk.
    let out = call(
        &Glob,
        &env.ctx,
        json!({"pattern": "*.rs", "path": "src/a.rs"}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
}

/// A glob whose paths do not fit the call's budget keeps whole paths and reports
/// the real number shown, instead of a spliced list under a count it did not meet.
#[tokio::test]
async fn a_glob_cut_by_the_output_budget_reports_what_it_really_showed() {
    let limits = Limits {
        max_tool_output_bytes: 1_500,
        ..Default::default()
    };
    let env = env_with("glob-budget", ask(), limits, ApprovalDecision::AllowOnce);
    for i in 0..150 {
        env.tmp
            .write(&format!("a-fairly-long-module-name-{i:03}.rs"), "x\n");
    }

    let out = call(&Glob, &env.ctx, json!({"pattern": "*.rs"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.truncated);
    assert!(out.content.len() <= 1_500, "{} bytes", out.content.len());
    let rows: Vec<&str> = out
        .content
        .lines()
        .skip(1)
        .filter(|line| !line.starts_with('['))
        .collect();
    assert!(rows.len() < 150, "{}", out.content);
    for row in &rows {
        assert!(
            row.starts_with("a-fairly-long-module-name-") && row.ends_with(".rs"),
            "a spliced path reached the model: {row:?}"
        );
    }
    let meta = out.meta.unwrap();
    assert_eq!(meta["shown"], json!(rows.len()));
    assert!(
        out.content.contains(&format!("showing {} of", rows.len())),
        "the note must agree with the rows: {}",
        out.content
    );
    assert!(
        out.content.contains("narrow the pattern or the `path`"),
        "the note must name the narrowing: {}",
        out.content
    );
}

#[tokio::test]
async fn glob_stops_at_the_cap_and_says_so() {
    let env = env("glob-cap");
    for i in 0..40 {
        env.tmp.write(&format!("f{i:02}.txt"), "x\n");
    }

    let out = call(
        &Glob,
        &env.ctx,
        json!({"pattern": "*.txt", "max_results": 5}),
    )
    .await;
    assert!(out.ok, "{}", out.content);
    let meta = out.meta.as_ref().unwrap();
    assert_eq!(meta["capped"], json!(true));
    assert_eq!(meta["shown"], json!(5));
    assert!(out.truncated);
    assert!(
        out.content.contains("[truncated:"),
        "the cap must be visible: {}",
        out.content
    );
    assert_eq!(
        out.content.lines().filter(|l| l.starts_with("f")).count(),
        5,
        "{}",
        out.content
    );
}

// -------------------------------------------------------- argument caps ---
//
// Arguments are input tokens too: a pathological call must be refused with the
// cap named *before* the tool starts work, because by the time the request is
// parsed the bytes have already been paid for.

#[tokio::test]
async fn oversized_edit_strings_are_refused_before_the_file_is_touched() {
    let env = env("edit-arg-cap");
    // The path does not exist: the refusal below must come from the argument
    // guard, not from a failed read, which proves the guard runs first.
    let huge = "x".repeat(MAX_EDIT_ARG_BYTES + 1);
    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "missing.txt", "old_string": huge, "new_string": "y"}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
    assert!(
        out.content.contains("capped at 2048 KiB"),
        "the refusal must name the cap: {}",
        out.content
    );
    assert!(
        out.content.contains("Nothing was read or written"),
        "{}",
        out.content
    );
    assert!(
        !out.content.contains("does not exist"),
        "the guard must run before the file is read: {}",
        out.content
    );

    // Just under the cap is not refused by the guard (the file read reports next).
    let big = "x".repeat(MAX_EDIT_ARG_BYTES);
    let out = call(
        &EditFile,
        &env.ctx,
        json!({"path": "missing.txt", "old_string": big, "new_string": "y"}),
    )
    .await;
    assert!(!out.ok, "{}", out.content);
    assert!(
        !out.content.contains("capped at"),
        "a string at the cap is allowed through to the read: {}",
        out.content
    );
}

#[tokio::test]
async fn an_oversized_command_is_refused_with_the_cap_named() {
    let env = env("cmd-arg-cap");
    let huge = format!("echo {}", "a".repeat(MAX_COMMAND_BYTES));
    let out = call(&RunCommand, &env.ctx, json!({"command": huge})).await;
    assert!(!out.ok, "{}", out.content);
    assert!(
        out.content.contains(&format!(
            "run_command accepts at most {MAX_COMMAND_BYTES} bytes"
        )),
        "the refusal must name the cap: {}",
        out.content
    );
    assert!(out.content.contains("Nothing was run"), "{}", out.content);
    // No approval was requested for a call that never ran.
    assert!(env.gate.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_oversized_search_pattern_is_refused_with_the_cap_named() {
    let env = env("search-arg-cap");
    let huge = "a".repeat(MAX_PATTERN_BYTES + 1);
    let out = call(&SearchFiles, &env.ctx, json!({"pattern": huge})).await;
    assert!(!out.ok, "{}", out.content);
    assert!(
        out.content.contains(&format!(
            "search_files accepts at most {MAX_PATTERN_BYTES} bytes"
        )),
        "the refusal must name the cap: {}",
        out.content
    );
}

// ---------------------------------------------------------------- git ---

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

    // `git_log` says the same thing instead of failing the call.
    let out = call(&GitLog, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    assert!(out.content.contains("is_repo: false"), "{}", out.content);
    assert_eq!(out.meta.unwrap()["is_repo"], json!(false));
}

#[tokio::test]
async fn git_log_is_bounded_ordered_and_confined() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    let env = env("git-log");
    env.tmp.write("tracked.txt", "one\n");
    git(env.tmp.path(), &["init", "-q", "-b", "main"]);
    git(env.tmp.path(), &["add", "."]);
    git(env.tmp.path(), &["commit", "-qm", "first commit"]);
    for i in 2..=5 {
        env.tmp.write("tracked.txt", &format!("{i}\n"));
        git(env.tmp.path(), &["add", "."]);
        git(env.tmp.path(), &["commit", "-qm", &format!("commit {i}")]);
    }

    // Default: every commit, newest first, one line each.
    let out = call(&GitLog, &env.ctx, json!({})).await;
    assert!(out.ok, "{}", out.content);
    let meta = out.meta.as_ref().unwrap();
    assert_eq!(meta["commits"], json!(5));
    assert_eq!(meta["capped"], json!(false));
    assert!(!out.truncated);
    let body: Vec<&str> = out
        .content
        .lines()
        .filter(|line| !line.starts_with("is_repo") && !line.starts_with("commits"))
        .collect();
    assert_eq!(body.len(), 5, "{}", out.content);
    assert!(body[0].ends_with("commit 5"), "newest first: {}", body[0]);
    assert!(
        body[4].ends_with("first commit"),
        "oldest last: {}",
        body[4]
    );
    // `sha date subject`: a sha, an ISO date, then the subject.
    let mut parts = body[0].split(' ');
    assert!(!parts.next().unwrap().is_empty());
    let date = parts.next().unwrap();
    assert_eq!(date.len(), 10, "date is YYYY-MM-DD: {date}");
    assert!(date.as_bytes()[4] == b'-');

    // Bounded: `max_count` cuts the list and says that it did.
    let out = call(&GitLog, &env.ctx, json!({"max_count": 2})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.meta.as_ref().unwrap()["commits"], json!(2));
    assert_eq!(out.meta.as_ref().unwrap()["capped"], json!(true));
    assert!(out.truncated);
    assert!(
        out.content.contains("showing the latest 2 commits"),
        "{}",
        out.content
    );

    // A path narrows the history to commits that touch it.
    env.tmp.write("other.txt", "unrelated\n");
    git(env.tmp.path(), &["add", "."]);
    git(env.tmp.path(), &["commit", "-qm", "only other"]);
    let out = call(&GitLog, &env.ctx, json!({"path": "tracked.txt"})).await;
    assert!(out.ok, "{}", out.content);
    assert!(
        !out.content.contains("only other"),
        "a path narrows the log: {}",
        out.content
    );
    assert!(out.content.contains("commit 5"), "{}", out.content);

    // The ceiling is hard, not a suggestion.
    let out = call(&GitLog, &env.ctx, json!({"max_count": 100_000})).await;
    assert!(out.ok, "{}", out.content);
    assert_eq!(out.meta.as_ref().unwrap()["commits"], json!(6));
}

#[tokio::test]
async fn git_log_does_not_leak_the_enclosing_repository() {
    if !git_available() {
        eprintln!("git not installed; skipping");
        return;
    }
    // `<outer>/` is the repository; `<outer>/inner/` is the workspace.
    let outer = TempWorkspace::new("git-log-nested");
    git(outer.path(), &["init", "-q", "-b", "main"]);
    outer.write("sibling.txt", "SIBLING-SECRET\n");
    outer.write("inner/tracked.txt", "inner one\n");
    git(outer.path(), &["add", "."]);
    git(outer.path(), &["commit", "-qm", "init both"]);
    outer.write("sibling.txt", "SIBLING-SECRET two\n");
    git(outer.path(), &["add", "."]);
    git(outer.path(), &["commit", "-qm", "OUTSIDE-ONLY subject"]);
    outer.write("inner/tracked.txt", "inner two\n");
    git(outer.path(), &["add", "."]);
    git(outer.path(), &["commit", "-qm", "INSIDE-ONLY subject"]);

    let inner_root = outer.path().join("inner");
    let ws = Workspace::new(&inner_root).unwrap();
    let ctx = ctx_for(&ws);

    let log = call(&GitLog, &ctx, json!({})).await;
    assert!(log.ok, "{}", log.content);
    assert!(
        !log.content.contains("OUTSIDE-ONLY"),
        "a commit that touches nothing inside the workspace must not be listed:\n{}",
        log.content
    );
    assert!(
        log.content.contains("INSIDE-ONLY"),
        "the workspace's own commit must still be listed:\n{}",
        log.content
    );
    assert_eq!(log.meta.as_ref().unwrap()["confined"], json!(true));
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
    assert_eq!(tools.len(), 11);
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert_eq!(
        names,
        vec![
            "read_file",
            "list_dir",
            "search_files",
            "glob",
            "write_file",
            "edit_file",
            "run_command",
            "git_status",
            "git_diff",
            "git_log",
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
        // A usage failure is answered with the call the model should have written,
        // built from the schema; a tool with no usable example gives no hint.
        assert!(
            spec.example_arguments()
                .as_object()
                .is_some_and(|o| !o.is_empty()),
            "{} needs at least one argument in its usage hint",
            spec.name
        );
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
