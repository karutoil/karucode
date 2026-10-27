//! Incremental SSE -> [`ProviderChunk`] translation.
//!
//! The state machine is driven by an async stream of body chunks. Cancellation is
//! observed while waiting for the next chunk, so cancelling the token aborts a
//! live HTTP stream promptly; dropping the returned stream drops the response and
//! stops the work because no background task is spawned.

use crate::errors::{map_reqwest_error, redact, truncate_chars};
use crate::sse::{SseEvent, SseParser};
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, StreamExt};
use kc_core::error::{Error, Result};
use kc_core::provider::{ProviderChunk, ProviderStream};
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Refuse to buffer an event that never terminates; protects memory against a
/// broken or hostile upstream.
pub(crate) const MAX_SSE_EVENT_BYTES: usize = 8 * 1024 * 1024;

type ByteStream = BoxStream<'static, reqwest::Result<Bytes>>;

pub(crate) fn stream_from_response(
    bytes: ByteStream,
    cancel: CancellationToken,
    url: String,
    timeout: Duration,
    secrets: Vec<String>,
) -> ProviderStream {
    let state = StreamState {
        bytes,
        cancel,
        url,
        timeout,
        secrets,
        parser: SseParser::new(),
        decoder: Utf8Decoder::default(),
        queue: VecDeque::new(),
        saw_event: false,
        saw_finish: false,
        input_done: false,
        failed: false,
        finished: false,
    };
    Box::pin(stream::unfold(state, |mut state| async move {
        let item = state.next_item().await?;
        Some((item, state))
    }))
}

struct StreamState {
    bytes: ByteStream,
    cancel: CancellationToken,
    url: String,
    timeout: Duration,
    secrets: Vec<String>,
    parser: SseParser,
    decoder: Utf8Decoder,
    queue: VecDeque<Result<ProviderChunk>>,
    /// At least one JSON payload was understood.
    saw_event: bool,
    /// A `finish_reason` was delivered, so no synthetic Finish is needed.
    saw_finish: bool,
    /// Input is exhausted: `[DONE]`, EOF, or a terminal error.
    input_done: bool,
    /// A terminal error chunk was emitted; do not synthesize Finish.
    failed: bool,
    /// The stream itself is exhausted.
    finished: bool,
}

impl StreamState {
    async fn next_item(&mut self) -> Option<Result<ProviderChunk>> {
        loop {
            if let Some(item) = self.queue.pop_front() {
                return Some(item);
            }
            if self.finished {
                return None;
            }
            if self.cancel.is_cancelled() {
                self.finished = true;
                return Some(Err(Error::Cancelled));
            }
            if self.input_done {
                self.finished = true;
                if self.failed {
                    return None;
                }
                if !self.saw_event {
                    return Some(Ok(ProviderChunk::Error {
                        message: format!(
                            "provider at {} closed the stream without sending any data",
                            self.url
                        ),
                        retryable: true,
                    }));
                }
                if !self.saw_finish {
                    self.saw_finish = true;
                    return Some(Ok(ProviderChunk::Finish {
                        reason: "stop".into(),
                    }));
                }
                return None;
            }

            // Cloned so the two select arms borrow disjoint values.
            let cancel = self.cancel.clone();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    self.finished = true;
                    return Some(Err(Error::Cancelled));
                }
                chunk = self.bytes.next() => match chunk {
                    None => {
                        self.input_done = true;
                        if let Some(event) = self.parser.finish() {
                            self.handle_event(event);
                        }
                    }
                    Some(Err(err)) => {
                        self.finished = true;
                        return Some(Err(map_reqwest_error(&self.url, &err, self.timeout)));
                    }
                    Some(Ok(bytes)) => self.feed(&bytes),
                },
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let text = self.decoder.push(bytes);
        if text.is_empty() {
            return;
        }
        for event in self.parser.push(&text) {
            self.handle_event(event);
            if self.input_done {
                return;
            }
        }
        if self.parser.buffered_len() > MAX_SSE_EVENT_BYTES {
            self.fail(format!(
                "provider sent more than {MAX_SSE_EVENT_BYTES} bytes without terminating an SSE event"
            ));
        }
    }

    fn handle_event(&mut self, event: SseEvent) {
        let data = event.data.trim();
        if data.is_empty() {
            return;
        }
        if data == "[DONE]" {
            self.input_done = true;
            return;
        }
        match serde_json::from_str::<Value>(data) {
            Ok(value) => self.handle_payload(&value),
            Err(err) => {
                let excerpt = truncate_chars(data, 160);
                let excerpt = redact(&excerpt, &self.secrets);
                self.fail(format!(
                    "malformed JSON in provider stream: {err} (chunk: {excerpt})"
                ));
            }
        }
    }

    fn handle_payload(&mut self, value: &Value) {
        if let Some(error) = value.get("error").filter(|v| !v.is_null()) {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string());
            let kind = error
                .get("type")
                .or_else(|| error.get("code"))
                .map(text_of)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let retryable = [
                "rate_limit",
                "overloaded",
                "server_error",
                "timeout",
                "unavailable",
                "429",
                "500",
                "502",
                "503",
                "504",
            ]
            .iter()
            .any(|needle| kind.contains(needle));
            let message = redact(&format!("provider error: {message}"), &self.secrets);
            self.queue
                .push_back(Ok(ProviderChunk::Error { message, retryable }));
            self.failed = true;
            self.input_done = true;
            return;
        }

        self.saw_event = true;

        // `usage` may arrive on the final chunk, on a chunk with no choices, or
        // not at all.
        if let Some(usage) = value.get("usage").filter(|v| v.is_object()) {
            let input = token_count(usage, "prompt_tokens", "input_tokens");
            let output = token_count(usage, "completion_tokens", "output_tokens");
            if input.is_some() || output.is_some() {
                self.queue.push_back(Ok(ProviderChunk::Usage {
                    input_tokens: input.unwrap_or(0),
                    output_tokens: output.unwrap_or(0),
                }));
            }
        }

        // Keepalive chunks legitimately have `choices: []`.
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };

        if let Some(delta) = choice.get("delta").filter(|v| v.is_object()) {
            if let Some(text) = delta_text(delta.get("content")) {
                if !text.is_empty() {
                    self.queue.push_back(Ok(ProviderChunk::Text(text)));
                }
            }
            // Some endpoints use `reasoning`, others `reasoning_content`.
            let reasoning = delta_text(delta.get("reasoning_content"))
                .or_else(|| delta_text(delta.get("reasoning")));
            if let Some(text) = reasoning {
                if !text.is_empty() {
                    self.queue.push_back(Ok(ProviderChunk::Reasoning(text)));
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for (position, call) in calls.iter().enumerate() {
                    let index = call
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|i| i as usize)
                        .unwrap_or(position);
                    let id = call.get("id").and_then(Value::as_str).and_then(non_empty);
                    let function = call.get("function");
                    let name = function
                        .and_then(|f| f.get("name"))
                        .and_then(Value::as_str)
                        .and_then(non_empty);
                    let arguments = function.and_then(|f| delta_text(f.get("arguments")));
                    if id.is_none() && name.is_none() && arguments.is_none() {
                        continue;
                    }
                    self.queue.push_back(Ok(ProviderChunk::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta: arguments,
                    }));
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            if !reason.is_empty() {
                self.saw_finish = true;
                self.queue.push_back(Ok(ProviderChunk::Finish {
                    reason: reason.to_string(),
                }));
            }
        }
    }

    fn fail(&mut self, message: String) {
        let message = redact(&message, &self.secrets);
        self.queue.push_back(Ok(ProviderChunk::Error {
            message,
            retryable: false,
        }));
        self.failed = true;
        self.input_done = true;
    }
}

fn non_empty(text: &str) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn token_count(usage: &Value, primary: &str, fallback: &str) -> Option<u32> {
    usage
        .get(primary)
        .or_else(|| usage.get(fallback))
        .and_then(Value::as_u64)
        .map(|value| value.min(u64::from(u32::MAX)) as u32)
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Text carried by a delta field: either a plain string or an array of content
/// parts (`[{"type":"text","text":"..."}]`), which some gateways emit.
fn delta_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for part in parts {
                match part {
                    Value::String(text) => out.push_str(text),
                    other => {
                        if let Some(text) = other.get("text").and_then(Value::as_str) {
                            out.push_str(text);
                        }
                    }
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

/// Decodes UTF-8 across chunk boundaries so a multi-byte character split by the
/// network is not mangled.
#[derive(Default)]
struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            if self.pending.is_empty() {
                break;
            }
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    out.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(err) => {
                    let valid = err.valid_up_to();
                    if valid > 0 {
                        // Safe by construction: `valid_up_to` bounds a valid prefix.
                        out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or(""));
                        self.pending.drain(..valid);
                    }
                    match err.error_len() {
                        Some(invalid) => {
                            self.pending.drain(..invalid);
                            out.push('\u{FFFD}');
                        }
                        // Incomplete trailing sequence: wait for the next chunk.
                        None => break,
                    }
                }
            }
        }
        if self.pending.len() > 8 {
            // Only an incomplete code point (<= 3 bytes) may legitimately remain.
            out.push('\u{FFFD}');
            self.pending.clear();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_chunks(chunks: Vec<&'static str>) -> StreamState {
        let bytes: Vec<reqwest::Result<Bytes>> = chunks
            .into_iter()
            .map(|c| Ok(Bytes::from_static(c.as_bytes())))
            .collect();
        StreamState {
            bytes: stream::iter(bytes).boxed(),
            cancel: CancellationToken::new(),
            url: "http://test/v1/chat/completions".into(),
            timeout: Duration::from_secs(5),
            secrets: Vec::new(),
            parser: SseParser::new(),
            decoder: Utf8Decoder::default(),
            queue: VecDeque::new(),
            saw_event: false,
            saw_finish: false,
            input_done: false,
            failed: false,
            finished: false,
        }
    }

    async fn drain(mut state: StreamState) -> Vec<Result<ProviderChunk>> {
        let mut out = Vec::new();
        while let Some(item) = state.next_item().await {
            out.push(item);
        }
        out
    }

    #[tokio::test]
    async fn reasoning_content_and_fallback_field_both_stream() {
        let items = drain(state_with_chunks(vec![
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think \"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"reasoning\":\"more\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\n\
             data: [DONE]\n\n",
        ]))
        .await;
        assert_eq!(
            items.into_iter().map(|i| i.unwrap()).collect::<Vec<_>>(),
            vec![
                ProviderChunk::Reasoning("think ".into()),
                ProviderChunk::Reasoning("more".into()),
                ProviderChunk::Text("answer".into()),
                ProviderChunk::Finish {
                    reason: "stop".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn empty_choices_keepalive_is_ignored() {
        let items = drain(state_with_chunks(vec![
            "data: {\"choices\":[]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\n\n",
        ]))
        .await;
        assert_eq!(
            items.into_iter().map(|i| i.unwrap()).collect::<Vec<_>>(),
            vec![
                ProviderChunk::Text("x".into()),
                ProviderChunk::Finish {
                    reason: "stop".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn utf8_split_across_chunks_is_reassembled() {
        let mut decoder = Utf8Decoder::default();
        let bytes = "héllo".as_bytes();
        assert_eq!(decoder.push(&bytes[..2]), "h");
        assert_eq!(decoder.push(&bytes[2..]), "éllo");
    }

    #[tokio::test]
    async fn invalid_utf8_is_replaced_not_fatal() {
        let mut decoder = Utf8Decoder::default();
        assert_eq!(decoder.push(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
    }

    #[tokio::test]
    async fn stream_without_any_data_reports_a_retryable_error() {
        let items = drain(state_with_chunks(vec![])).await;
        assert_eq!(items.len(), 1);
        match items[0].as_ref().unwrap() {
            ProviderChunk::Error { retryable, .. } => assert!(*retryable),
            other => panic!("unexpected chunk: {other:?}"),
        }
    }
}
