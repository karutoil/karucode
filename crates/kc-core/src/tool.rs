use crate::approval::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use crate::config::Limits;
use crate::error::Result;
use crate::events::{Event, EventSink};
use crate::ids::{AgentId, RunId, SessionId};
use crate::paths::Workspace;
use crate::settings::Settings;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    /// Inspects state and never changes the workspace.
    ReadOnly,
    /// Creates or modifies files inside the workspace.
    Mutating,
    /// Runs arbitrary commands as the server user. Not sandboxed.
    Shell,
    /// Spawns or controls subagents.
    Delegate,
    /// Calls out to a plugin or MCP server.
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOrigin {
    #[default]
    Builtin,
    Plugin {
        name: String,
    },
    Mcp {
        server: String,
    },
}

impl ToolOrigin {
    pub fn label(&self) -> String {
        match self {
            ToolOrigin::Builtin => "builtin".into(),
            ToolOrigin::Plugin { name } => format!("plugin:{name}"),
            ToolOrigin::Mcp { server } => format!("mcp:{server}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub origin: ToolOrigin,
}

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCallRequest {
    pub fn arg_str(&self, key: &str) -> Result<&str> {
        self.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::Error::BadRequest(format!("missing string argument `{key}`")))
    }

    pub fn opt_str(&self, key: &str) -> Option<&str> {
        self.arguments.get(key).and_then(|v| v.as_str())
    }

    pub fn opt_u64(&self, key: &str) -> Option<u64> {
        self.arguments.get(key).and_then(|v| v.as_u64())
    }

    pub fn opt_bool(&self, key: &str) -> Option<bool> {
        self.arguments.get(key).and_then(|v| v.as_bool())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub ok: bool,
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    /// Images this result wants the model to look at. Only the built-in tools
    /// produce them (plugin/MCP results decode with an empty list), and only when
    /// [`ToolCtx::supports_images`] says the active model accepts image input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::message::ImageAttachment>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
            truncated: false,
            meta: None,
            images: Vec::new(),
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
            truncated: false,
            meta: None,
            images: Vec::new(),
        }
    }

    pub fn with_meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_images(mut self, images: Vec<crate::message::ImageAttachment>) -> Self {
        self.images = images;
        self
    }

    pub fn truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    pub fn from_result(r: Result<String>) -> Self {
        match r {
            Ok(s) => ToolOutcome::ok(s),
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Block until the user answers (or the policy decides automatically).
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision;
}

/// Thin link between the caller and the approval gate, used by tools so they can
/// ask without knowing about the store or the UI.
#[derive(Clone)]
pub struct ApprovalHandle {
    gate: Arc<dyn ApprovalGate>,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub agent_id: Option<AgentId>,
}

impl ApprovalHandle {
    pub fn new(
        gate: Arc<dyn ApprovalGate>,
        session_id: SessionId,
        run_id: RunId,
        agent_id: Option<AgentId>,
    ) -> Self {
        Self {
            gate,
            session_id,
            run_id,
            agent_id,
        }
    }

    pub async fn ask(
        &self,
        kind: ApprovalKind,
        title: impl Into<String>,
        detail: impl Into<String>,
        payload: serde_json::Value,
    ) -> ApprovalDecision {
        let request = ApprovalRequest {
            id: crate::ids::ApprovalId::new().0,
            kind,
            title: title.into(),
            detail: detail.into(),
            payload,
            session_id: self.session_id.0.clone(),
            run_id: self.run_id.0.clone(),
            agent_id: self.agent_id.as_ref().map(|a| a.0.clone()),
        };
        self.gate.request(request).await
    }
}

/// A file's state immediately before a mutating tool changed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CapturedFile {
    /// False when the file did not exist, so undoing the change removes it.
    pub existed: bool,
    /// The bytes it had. Empty when `existed` is false or when the capture was
    /// refused (binary content, or larger than the cap).
    pub content: String,
    /// True when the file existed but its bytes could not be kept.
    pub failed: bool,
}

impl CapturedFile {
    /// The normal case: the file did not exist.
    pub fn absent() -> Self {
        Self::default()
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self {
            existed: true,
            content: content.into(),
            failed: false,
        }
    }

    /// The file exists but is too large or not text; a rewind cannot restore it.
    pub fn uncapturable() -> Self {
        Self {
            existed: true,
            content: String::new(),
            failed: true,
        }
    }
}

/// Capture `abs` for the rewind journal.
///
/// A file that does not exist is [`CapturedFile::absent`]; a directory, a binary
/// file or one over [`crate::checkpoint::MAX_SNAPSHOT_BYTES`] is
/// [`CapturedFile::uncapturable`], because restoring half a file is worse than
/// admitting the rewind cannot restore it.
pub fn capture_file(abs: &std::path::Path) -> CapturedFile {
    let meta = match std::fs::symlink_metadata(abs) {
        Ok(meta) => meta,
        Err(_) => return CapturedFile::absent(),
    };
    if !meta.is_file() || meta.len() > crate::checkpoint::MAX_SNAPSHOT_BYTES as u64 {
        return CapturedFile::uncapturable();
    }
    match std::fs::read(abs) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => CapturedFile::text(text),
            Err(_) => CapturedFile::uncapturable(),
        },
        Err(_) => CapturedFile::uncapturable(),
    }
}

/// Records the before-image of every file a mutating tool changes.
///
/// The runtime builds one per run and hands it to each [`ToolCtx`]; the tool
/// calls [`JournalHandle::record`] immediately *before* it writes. Recording is
/// deliberately not a step the agent can skip or forget: it happens inside the
/// file tool, next to the write it describes.
#[async_trait]
pub trait JournalHandle: Send + Sync {
    /// Capture `rel` (workspace-relative) because `tool` is about to change it.
    ///
    /// Best-effort: a journal that cannot be written leaves the change
    /// unrestorable, which the rewind reports, rather than failing the write.
    async fn record(&self, rel: &str, tool: &str);

    /// Capture `rel` and report whether the capture landed, for a caller that
    /// needs to refuse the write when it did not.
    async fn capture_path(&self, rel: &str, tool: &str) -> bool;
}

/// Everything a tool is allowed to know about its execution context.
#[derive(Clone)]
pub struct ToolCtx {
    pub workspace: Arc<Workspace>,
    pub cancel: CancellationToken,
    pub approvals: ApprovalHandle,
    pub events: EventSink,
    pub settings: Arc<dyn Settings>,
    /// Subagent nesting depth; 0 for the main agent.
    pub depth: u8,
    /// Whether the model running this turn accepts image input.
    ///
    /// The runtime resolves it from the session's effective model
    /// (`model.supports_images`), so a session pinned to a vision model gets
    /// images even when the install-wide default is a text-only one. A tool that
    /// would otherwise return pixels must check this and explain the limitation
    /// instead of sending an image the endpoint will reject.
    pub supports_images: bool,
    /// Where a mutating tool records the bytes it is about to replace, so a
    /// rewind can put the file back. `None` outside a run (a test harness, a
    /// tool invoked directly), in which case the change simply cannot be undone.
    pub checkpoints: Option<Arc<dyn JournalHandle>>,
}

impl ToolCtx {
    pub fn emit(&self, event: Event) {
        (self.events)(event);
    }

    pub async fn limits(&self) -> Limits {
        self.settings.limits().await
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Record the state of `rel` before a mutating tool changes it.
    ///
    /// Best-effort by design: a journal that cannot be written must not fail the
    /// write the user asked for. The rewind then reports that path as
    /// unrestorable rather than silently restoring the wrong bytes - which is
    /// why this returns `()` and not `Result`. A tool that *must* know can call
    /// [`JournalHandle::capture_path`] itself.
    pub async fn record_checkpoint(&self, rel: &str, tool: &str) {
        if let Some(journal) = &self.checkpoints {
            journal.record(rel, tool).await;
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    /// Whether the runtime must collect an approval before invoking this tool.
    ///
    /// Built-in mutating/shell tools gate *themselves* (they can show the exact
    /// command or file diff), so the runtime only consults this for tools from
    /// plugins and MCP servers. Returning `true` means the runtime asks the
    /// approval policy first; `false` means the tool is exempt (for example an
    /// MCP server configured with `require_approval = false`).
    fn requires_approval(&self) -> bool {
        !matches!(self.class(), ToolClass::ReadOnly)
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome;
}

impl std::fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tool({})", self.spec().name)
    }
}

/// The set of tools exposed to the model. Ordered by name for stable prompts.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.spec().name;
        self.tools.insert(name, tool);
    }

    pub fn register_all(&mut self, tools: Vec<Arc<dyn Tool>>) {
        for t in tools {
            self.register(t);
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.remove(name)
    }

    pub fn remove_prefix(&mut self, prefix: &str) {
        self.tools.retain(|k, _| !k.starts_with(prefix));
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Truncate a tool result to a byte budget without splitting a UTF-8 character.
pub fn truncate_output(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let omitted = text.len() - end;
    (
        format!(
            "{}\n\n[truncated: {} bytes omitted; raise the limit or read a narrower range]",
            &text[..end],
            omitted
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries() {
        let s = "héllo wörld";
        let (out, cut) = truncate_output(s, 4);
        assert!(cut);
        assert!(out.starts_with("hél"));
        assert!(out.contains("bytes omitted"));
    }

    #[test]
    fn short_output_is_untouched() {
        let (out, cut) = truncate_output("abc", 10);
        assert!(!cut);
        assert_eq!(out, "abc");
    }

    fn temp_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kc-core-capture-{tag}-{}",
            crate::ids::SessionId::new().as_str()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("file.bin");
        std::fs::write(&path, bytes).expect("write");
        path
    }

    #[test]
    fn a_missing_file_is_captured_as_absent() {
        let dir =
            std::env::temp_dir().join(format!("kc-core-capture-absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let captured = capture_file(&dir.join("nope.txt"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(captured, CapturedFile::absent());
        assert!(!captured.existed, "the undo of a create removes the file");
        assert!(!captured.failed);
    }

    #[test]
    fn text_is_captured_verbatim() {
        let path = temp_file("text", b"fn main() {}\n");
        let captured = capture_file(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert_eq!(captured, CapturedFile::text("fn main() {}\n"));
    }

    #[test]
    fn a_binary_file_is_recorded_as_unrestorable_rather_than_mangled() {
        let path = temp_file("binary", &[0xff, 0xfe, 0x00, 0x01]);
        let captured = capture_file(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert!(
            captured.failed,
            "a lossy conversion would restore different bytes than it removed"
        );
        assert!(captured.existed);
        assert!(captured.content.is_empty());
    }

    #[test]
    fn a_file_over_the_cap_is_recorded_as_unrestorable() {
        let big = vec![b'a'; crate::checkpoint::MAX_SNAPSHOT_BYTES + 1];
        let path = temp_file("huge", &big);
        let captured = capture_file(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert!(captured.failed, "half a file is worse than none");
    }

    #[test]
    fn registry_orders_and_replaces() {
        struct Dummy(ToolSpec);
        #[async_trait]
        impl Tool for Dummy {
            fn spec(&self) -> ToolSpec {
                self.0.clone()
            }
            async fn invoke(&self, _r: ToolCallRequest, _c: ToolCtx) -> ToolOutcome {
                ToolOutcome::ok("")
            }
        }
        let mk = |n: &str| -> Arc<dyn Tool> {
            Arc::new(Dummy(ToolSpec {
                name: n.into(),
                description: String::new(),
                parameters: serde_json::json!({}),
                origin: ToolOrigin::Builtin,
            }))
        };
        let mut reg = ToolRegistry::new();
        reg.register(mk("b"));
        reg.register(mk("a"));
        assert_eq!(reg.names(), vec!["a".to_string(), "b".to_string()]);
        reg.register(mk("a"));
        assert_eq!(reg.len(), 2);
    }
}
