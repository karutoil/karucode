use crate::config::ModelConfig;
use crate::error::Result;
use crate::message::{Message, ToolCall};
use crate::tool::ToolSpec;
use async_trait::async_trait;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub admission: Option<AttemptAdmission>,
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub temperature: f32,
    /// Send `temperature` to the provider at all (see `ModelConfig`).
    pub send_temperature: bool,
    pub max_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub parallel_tool_calls: bool,
    pub extra_headers: Vec<(String, String)>,
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

/// A durable admission check invoked before every physical provider attempt.
#[derive(Clone)]
pub struct AttemptAdmission {
    pub store: Arc<dyn crate::store::Store>,
    pub session_id: String,
}
impl std::fmt::Debug for AttemptAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttemptAdmission")
            .field("session_id", &self.session_id)
            .finish()
    }
}
impl AttemptAdmission {
    pub async fn admit(&self) -> Result<()> {
        self.store.admit_provider_attempt(&self.session_id).await
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderChunk {
    Text(String),
    Reasoning(String),
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    Usage {
        input_tokens: u32,
        output_tokens: u32,
        /// Prompt tokens served from the provider's prefix cache, when the
        /// endpoint reports them. `None` means "not reported" - not "zero".
        cached_tokens: Option<u32>,
    },
    Finish {
        reason: String,
    },
    /// Not an error: the provider is telling us the stream ended abnormally.
    Error {
        message: String,
        retryable: bool,
    },
}

pub type ProviderStream =
    Pin<Box<dyn futures_core::Stream<Item = Result<ProviderChunk>> + Send + 'static>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ProviderStream>;

    /// Probe the endpoint's model list when supported. Default: unsupported.
    async fn list_models(&self) -> Result<Vec<String>> {
        Err(crate::Error::Provider(
            "model listing not supported by this provider".into(),
        ))
    }
}

/// Builds a provider for a given model configuration. The server implements this
/// so runtime code never constructs HTTP clients or reads secrets directly.
#[async_trait]
pub trait ProviderFactory: Send + Sync {
    async fn build(&self, config: &ModelConfig) -> Result<Arc<dyn Provider>>;
    /// Provider ids this factory can build.
    fn supported(&self) -> Vec<String> {
        vec!["openai_compatible".into()]
    }
}

/// Accumulates streamed deltas into complete tool calls.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: Vec<ToolCall>,
}

/// Upper bound on the tool-call index a provider may address.
///
/// `index` arrives straight from the gateway's JSON. Growing the vector to fit
/// it (`while len <= index { push }`) means a malicious or buggy endpoint that
/// sends `index: 1_000_000_000` makes the runtime allocate a billion entries:
/// an OOM with no error and no bound. Real providers number calls from zero
/// within a single assistant turn, so anything past this is a protocol error.
pub const MAX_TOOL_CALLS: usize = 256;

impl ToolCallAccumulator {
    pub fn apply(
        &mut self,
        index: usize,
        id: Option<String>,
        name: Option<String>,
        args: Option<String>,
    ) {
        // Ignore an out-of-range index instead of allocating for it. The call is
        // dropped rather than the whole turn: a single bad delta should not
        // discard the text the model already produced.
        if index >= MAX_TOOL_CALLS {
            return;
        }
        while self.calls.len() <= index {
            self.calls.push(ToolCall::default());
        }
        let call = &mut self.calls[index];
        if let Some(id) = id {
            if !id.is_empty() {
                call.id = id;
            }
        }
        if let Some(name) = name {
            if !name.is_empty() {
                call.name = name;
            }
        }
        if let Some(args) = args {
            call.arguments.push_str(&args);
        }
    }

    pub fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .filter(|c| !c.name.is_empty())
            .map(|mut c| {
                if c.id.is_empty() {
                    c.id = format!("call_{}", uuid::Uuid::new_v4().simple());
                }
                if c.arguments.trim().is_empty() {
                    c.arguments = "{}".into();
                }
                c
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_merges_split_tool_call_deltas() {
        let mut acc = ToolCallAccumulator::default();
        acc.apply(
            0,
            Some("call_1".into()),
            Some("read_file".into()),
            Some("{\"pa".into()),
        );
        acc.apply(0, None, None, Some("th\":\"a.txt\"}".into()));
        acc.apply(
            1,
            Some("call_2".into()),
            Some("list_dir".into()),
            Some("{}".into()),
        );
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, "{\"path\":\"a.txt\"}");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn accumulator_invents_ids_and_default_args() {
        let mut acc = ToolCallAccumulator::default();
        acc.apply(0, None, Some("git_status".into()), None);
        let calls = acc.finish();
        assert!(!calls[0].id.is_empty());
        assert_eq!(calls[0].arguments, "{}");
    }

    #[test]
    fn accumulator_drops_nameless_calls() {
        let mut acc = ToolCallAccumulator::default();
        acc.apply(0, Some("x".into()), None, Some("{}".into()));
        assert!(acc.finish().is_empty());
    }

    #[test]
    fn accumulator_ignores_an_out_of_range_index_instead_of_allocating() {
        // `index` comes straight from the gateway's JSON. Growing the vector to
        // fit it would let a malicious or buggy endpoint ask for a billion
        // entries; the delta must be ignored instead.
        let mut acc = ToolCallAccumulator::default();
        acc.apply(
            usize::MAX,
            Some("evil".into()),
            Some("run_command".into()),
            Some("{}".into()),
        );
        acc.apply(
            MAX_TOOL_CALLS,
            Some("evil2".into()),
            Some("run_command".into()),
            Some("{}".into()),
        );
        assert!(
            acc.finish().is_empty(),
            "an out-of-range index must not create a call"
        );

        // A legitimate call in the same turn still lands.
        let mut acc = ToolCallAccumulator::default();
        acc.apply(usize::MAX, Some("evil".into()), Some("x".into()), None);
        acc.apply(
            0,
            Some("ok".into()),
            Some("read_file".into()),
            Some("{}".into()),
        );
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "ok");
    }
}
