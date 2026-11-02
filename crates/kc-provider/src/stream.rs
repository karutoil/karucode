//! Incremental SSE -> [`ProviderChunk`] translation.
//!
//! The state machine is driven by an async stream of body chunks. Cancellation is
//! observed while waiting for the next chunk, so cancelling the token aborts a
//! live HTTP stream promptly; dropping the returned stream drops the response and
//! stops the work because no background task is spawned.

use crate::dsml::{DsmlScanner, DsmlTools, Step};
use crate::errors::{map_reqwest_error, redact, truncate_chars};
use crate::sse::{SseEvent, SseParser};
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, StreamExt};
use kc_core::error::{Error, Result};
use kc_core::provider::{ProviderChunk, ProviderStream, MAX_TOOL_CALLS};
use serde_json::Value;
use std::collections::VecDeque;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Refuse to buffer an event that never terminates; protects memory against a
/// broken or hostile upstream.
pub(crate) const MAX_SSE_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// Tool-call indices reserved for calls recovered from DSML markup.
///
/// A turn that mixes the two paths must not merge them: the native deltas are
/// numbered from zero by the endpoint, and the accumulator keys on `index`, so a
/// recovered call handed index 0 would append its arguments to the endpoint's
/// first call. The second half of the accumulator's range is reserved instead,
/// and a turn that exhausts *that* has already lost to
/// `limits.max_tool_calls_per_turn`.
const DSML_INDEX_BASE: usize = MAX_TOOL_CALLS / 2;

type ByteStream = BoxStream<'static, reqwest::Result<Bytes>>;

/// Translate one streaming response.
///
/// `tool_names` is what makes DSML recovery safe: a block of DeepSeek markup in
/// the text channel is only executed when every invoke names a tool this run
/// could actually call (see [`crate::dsml`]).
///
/// `timeout` is the per-read gap and `attempt` the wall-clock budget for the
/// whole body (see [`crate::attempt_timeout`]): the first catches a silent
/// endpoint, the second a stream that keeps trickling - or sends nothing but
/// keepalive comments - forever.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_from_response(
    bytes: ByteStream,
    cancel: CancellationToken,
    url: String,
    timeout: Duration,
    attempt: Duration,
    secrets: Vec<String>,
    tool_names: Vec<String>,
) -> ProviderStream {
    let state = StreamState {
        bytes,
        cancel,
        url,
        timeout,
        attempt,
        deadline: tokio::time::Instant::now() + attempt,
        secrets,
        parser: SseParser::new(),
        decoder: Utf8Decoder::default(),
        queue: VecDeque::new(),
        dsml: DsmlTools::new(tool_names).map(DsmlScanner::new),
        dsml_calls: 0,
        saw_event: false,
        saw_done: false,
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
    /// Wall-clock budget for this attempt (see [`crate::attempt_timeout`]).
    attempt: Duration,
    /// When that budget runs out.
    deadline: tokio::time::Instant,
    secrets: Vec<String>,
    parser: SseParser,
    decoder: Utf8Decoder,
    queue: VecDeque<Result<ProviderChunk>>,
    /// DeepSeek markup recovery, when this request had tools to recover for.
    dsml: Option<DsmlScanner>,
    /// Recovered calls handed out so far, for their reserved indices.
    dsml_calls: usize,
    /// At least one JSON payload was understood.
    saw_event: bool,
    /// A `data: [DONE]` terminator arrived. Distinct from `saw_event` because a
    /// completion delivered only as `[DONE]` is a finished (empty) answer, not a
    /// stream that died before sending anything.
    saw_done: bool,
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
                if !self.saw_event && !self.saw_done {
                    return Some(Ok(ProviderChunk::Error {
                        message: format!(
                            "provider at {} closed the stream without sending any data",
                            self.url
                        ),
                        retryable: true,
                    }));
                }
                if !self.saw_finish {
                    if !self.saw_event {
                        // A completion delivered only as `data: [DONE]`: the
                        // endpoint said it finished and had nothing to say.
                        // That is a legitimate (if odd) answer - a model that
                        // stopped on an empty string - so the turn ends with
                        // empty text. Reporting it as "sent no data" made the
                        // runtime restart the whole generation, which billed
                        // four completions for one empty turn.
                        tracing::debug!(
                            url = %self.url,
                            "provider stream carried only [DONE]: ending the turn with empty text"
                        );
                        self.saw_finish = true;
                        return Some(Ok(ProviderChunk::Finish {
                            reason: "stop".into(),
                        }));
                    }
                    // The stream carried a payload but no `finish_reason`. Report
                    // that honestly as `eof` rather than claiming a clean stop:
                    // the provider does not know whether the answer was
                    // complete, and the runtime treats `eof` as an incomplete
                    // stream (it does not synthesise a successful finish).
                    self.saw_finish = true;
                    return Some(Ok(ProviderChunk::Finish {
                        reason: "eof".into(),
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
                        // The message is over: whatever the DSML scanner still
                        // holds is text, and must reach the queue before the
                        // synthetic `Finish` below.
                        self.flush_text();
                    }
                    Some(Err(err)) => {
                        self.finished = true;
                        return Some(Err(map_reqwest_error(
                            &self.url,
                            &err,
                            self.timeout,
                            &self.secrets,
                        )));
                    }
                    Some(Ok(bytes)) => self.feed(&bytes),
                },
                // The whole-attempt budget. A per-read timeout cannot fire
                // against a body that trickles, or against a stream that sends
                // nothing but SSE keepalive comments, so without this an
                // endpoint can hold the attempt open for as long as it likes.
                // Reported as a timeout (retryable: the endpoint did not
                // answer, which says nothing bad about the request), and
                // queued behind whatever has already been delivered so no text
                // is lost with it.
                _ = tokio::time::sleep_until(self.deadline) => {
                    self.timed_out();
                    continue;
                }
            }
        }
    }

    /// The attempt ran out of wall clock. Anything the answer produced so far is
    /// kept; the failure is reported after it.
    fn timed_out(&mut self) {
        self.flush_text();
        self.queue.push_back(Err(Error::Timeout(format!(
            "provider at {} did not finish the response within {}s \
             (read gap {}s); the attempt was abandoned",
            self.url,
            self.attempt.as_secs(),
            self.timeout.as_secs()
        ))));
        self.failed = true;
        self.input_done = true;
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
            self.saw_done = true;
            self.input_done = true;
            self.flush_text();
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
            // The message is over. Anything the DSML scanner still holds is
            // text, and it has to be queued ahead of the error so the partial
            // answer is not lost with it.
            self.flush_text();
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
                    cached_tokens: cached_tokens(usage),
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
                    self.text(text);
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
                    let arguments = function
                        .and_then(|f| f.get("arguments"))
                        .and_then(argument_text);
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

    /// Route model text through DSML recovery.
    ///
    /// DeepSeek-V4 sometimes writes its native tool-call markup into `content`
    /// instead of `tool_calls`, and an endpoint that does not understand it hands
    /// the markup back as prose: the call never runs and the model looks broken.
    /// The scanner holds back text that may still be the start of a marker, so
    /// nothing of it is shown and then retracted.
    fn text(&mut self, delta: String) {
        let Some(scanner) = self.dsml.as_mut() else {
            self.queue.push_back(Ok(ProviderChunk::Text(delta)));
            return;
        };
        let step = scanner.push(&delta);
        self.emit_step(step);
    }

    /// Everything still held by the scanner is text now: the message ended.
    fn flush_text(&mut self) {
        let Some(mut scanner) = self.dsml.take() else {
            return;
        };
        let step = scanner.finish();
        self.emit_step(step);
    }

    fn emit_step(&mut self, step: Step) {
        if !step.text.is_empty() {
            self.queue.push_back(Ok(ProviderChunk::Text(step.text)));
        }
        for call in step.calls {
            let index = DSML_INDEX_BASE.saturating_add(self.dsml_calls);
            self.dsml_calls = self.dsml_calls.saturating_add(1);
            if index >= MAX_TOOL_CALLS {
                tracing::warn!(
                    tool = %call.name,
                    "ignoring a recovered DSML tool call: the turn already filled every slot"
                );
                continue;
            }
            self.queue.push_back(Ok(ProviderChunk::ToolCallDelta {
                index,
                // No id on the wire: the accumulator invents one, exactly as it
                // does for an endpoint that omits it.
                id: None,
                name: Some(call.name),
                arguments_delta: Some(call.arguments),
            }));
        }
    }

    fn fail(&mut self, message: String) {
        let message = redact(&message, &self.secrets);
        // A stream that ends inside an SSE event arrives here: the body was cut
        // mid-frame by a dying upstream or a proxy, so the next attempt can
        // land on a healthy channel. Nothing in this path is a rejected
        // request (those come back as an HTTP status or an `error` payload),
        // so it is always worth another attempt.
        self.queue.push_back(Ok(ProviderChunk::Error {
            message,
            retryable: true,
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

/// Prompt tokens served from the provider's prefix cache.
///
/// Three shapes are in the wild and a gateway may translate between them:
/// OpenAI's `prompt_tokens_details.cached_tokens`, DeepSeek's native
/// `prompt_cache_hit_tokens`, and the Anthropic-compatible
/// `cache_read_input_tokens`. Reporting this is what makes a "we are at 98%"
/// claim measurable instead of a hope.
fn cached_tokens(usage: &Value) -> Option<u32> {
    let nested = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("input_tokens_details"))
        .and_then(|details| details.get("cached_tokens"))
        .and_then(Value::as_u64);
    nested
        .or_else(|| usage.get("prompt_cache_hit_tokens").and_then(Value::as_u64))
        .or_else(|| usage.get("cache_read_input_tokens").and_then(Value::as_u64))
        .map(|value| value.min(u64::from(u32::MAX)) as u32)
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

/// Tool-call arguments: normally a JSON *string* that streams in fragments, but
/// some gateways send a ready-made object/array. Serialise those so the call is
/// not silently executed with empty arguments.
fn argument_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
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
        state_with_tools(chunks, Vec::new())
    }

    fn state_with_tools(chunks: Vec<&'static str>, tool_names: Vec<String>) -> StreamState {
        let bytes: Vec<reqwest::Result<Bytes>> = chunks
            .into_iter()
            .map(|c| Ok(Bytes::from_static(c.as_bytes())))
            .collect();
        StreamState {
            bytes: stream::iter(bytes).boxed(),
            cancel: CancellationToken::new(),
            url: "http://test/v1/chat/completions".into(),
            timeout: Duration::from_secs(5),
            attempt: Duration::from_secs(30),
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            secrets: Vec::new(),
            parser: SseParser::new(),
            decoder: Utf8Decoder::default(),
            queue: VecDeque::new(),
            dsml: DsmlTools::new(tool_names).map(DsmlScanner::new),
            dsml_calls: 0,
            saw_event: false,
            saw_done: false,
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

    /// A stream that never ends must be stopped by the attempt deadline, not left
    /// hanging: this is the drip-fed body and the keepalive-only stream in one.
    #[tokio::test]
    async fn an_endless_stream_is_cut_off_by_the_attempt_deadline() {
        // A body that keeps arriving but never terminates an event: `read_timeout`
        // never fires, because every single read succeeds.
        let endless = stream::unfold(0usize, |n| async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some((Ok::<Bytes, reqwest::Error>(Bytes::from_static(b"d")), n + 1))
        })
        .boxed();
        let state = StreamState {
            bytes: endless,
            cancel: CancellationToken::new(),
            url: "http://test/v1/chat/completions".into(),
            timeout: Duration::from_secs(30),
            attempt: Duration::from_millis(300),
            deadline: tokio::time::Instant::now() + Duration::from_millis(300),
            secrets: Vec::new(),
            parser: SseParser::new(),
            decoder: Utf8Decoder::default(),
            queue: VecDeque::new(),
            dsml: None,
            dsml_calls: 0,
            saw_event: false,
            saw_done: false,
            saw_finish: false,
            input_done: false,
            failed: false,
            finished: false,
        };

        let started = std::time::Instant::now();
        let items = tokio::time::timeout(Duration::from_secs(5), drain(state))
            .await
            .expect("the attempt deadline must end the stream, not hang");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the stream ran {:?}",
            started.elapsed()
        );
        assert_eq!(items.len(), 1, "{items:?}");
        match items[0].as_ref().unwrap_err() {
            Error::Timeout(message) => {
                assert!(message.contains("did not finish the response"), "{message}");
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    /// A body that only ever sends keepalive comments produces no events at all,
    /// so nothing but the attempt deadline can end it.
    #[tokio::test]
    async fn a_keepalive_only_stream_is_cut_off_by_the_attempt_deadline() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let bytes = stream::unfold(rx, |mut rx| async move {
            let item = rx.recv().await?;
            Some((Ok::<Bytes, reqwest::Error>(item), rx))
        })
        .boxed();
        // Feed keepalives from a task that is dropped with the test.
        let pinger = tokio::spawn(async move {
            loop {
                if tx.send(Bytes::from_static(b": keepalive\n\n")).is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let state = StreamState {
            bytes,
            cancel: CancellationToken::new(),
            url: "http://test/v1/chat/completions".into(),
            timeout: Duration::from_secs(30),
            attempt: Duration::from_millis(300),
            deadline: tokio::time::Instant::now() + Duration::from_millis(300),
            secrets: Vec::new(),
            parser: SseParser::new(),
            decoder: Utf8Decoder::default(),
            queue: VecDeque::new(),
            dsml: None,
            dsml_calls: 0,
            saw_event: false,
            saw_done: false,
            saw_finish: false,
            input_done: false,
            failed: false,
            finished: false,
        };

        let items = tokio::time::timeout(Duration::from_secs(5), drain(state))
            .await
            .expect("the attempt deadline must end a keepalive-only stream");
        pinger.abort();
        assert_eq!(items.len(), 1, "{items:?}");
        match items[0].as_ref().unwrap_err() {
            Error::Timeout(message) => {
                assert!(message.contains("did not finish the response"), "{message}");
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }

    /// The deadline is a *whole-attempt* budget, so a stream that finishes inside
    /// it is untouched - keepalives and all.
    #[tokio::test]
    async fn a_stream_that_finishes_inside_the_budget_is_untouched() {
        let mut state = state_with_chunks(vec![
            ": keepalive\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"x\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ]);
        state.attempt = Duration::from_millis(500);
        state.deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        let items = drain(state).await;
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

    /// A completion the endpoint delivers only as `data: [DONE]` is a finished,
    /// empty answer - not a stream that died before sending anything. Reporting
    /// it as "sent no data" made the runtime restart the generation, billing
    /// four completions for one empty turn.
    #[tokio::test]
    async fn a_done_only_completion_finishes_with_empty_text() {
        let items = drain(state_with_chunks(vec!["data: [DONE]\n\n"])).await;
        assert_eq!(
            items
                .into_iter()
                .map(|i| i.expect("no error"))
                .collect::<Vec<_>>(),
            vec![ProviderChunk::Finish {
                reason: "stop".into()
            }],
            "a [DONE]-only completion must end the turn, not raise a retryable error"
        );
    }

    /// A body that really did send nothing is still an error worth retrying: the
    /// `[DONE]` exception above must not swallow it.
    #[tokio::test]
    async fn a_stream_with_no_data_and_no_done_still_requests_a_retry() {
        let items = drain(state_with_chunks(vec![""])).await;
        assert_eq!(items.len(), 1, "{items:?}");
        match items[0].as_ref().unwrap() {
            ProviderChunk::Error { message, retryable } => {
                assert!(*retryable, "{message}");
                assert!(message.contains("without sending any data"), "{message}");
            }
            other => panic!("unexpected chunk: {other:?}"),
        }
    }

    /// The provider reports prompt-cache hits in three different shapes, and a
    /// gateway may translate between them. All of them must be read: a harness
    /// that cannot see `cached_tokens` cannot answer "are we at 98%?".
    #[tokio::test]
    async fn cached_prompt_tokens_are_read_from_every_reported_shape() {
        let cases = [
            ("{\"prompt_tokens\":100,\"completion_tokens\":1,\"prompt_tokens_details\":{\"cached_tokens\":90}}", Some(90)),
            ("{\"prompt_tokens\":100,\"completion_tokens\":1,\"prompt_cache_hit_tokens\":75}", Some(75)),
            ("{\"input_tokens\":100,\"output_tokens\":1,\"cache_read_input_tokens\":60}", Some(60)),
            ("{\"prompt_tokens\":100,\"completion_tokens\":1}", None),
        ];
        for (usage, expected) in cases {
            let body = format!("data: {{\"choices\":[],\"usage\":{usage}}}\n\ndata: [DONE]\n\n");
            let body: &'static str = Box::leak(body.into_boxed_str());
            let items = drain(state_with_chunks(vec![body])).await;
            let reported = items
                .into_iter()
                .find_map(|item| match item.unwrap() {
                    ProviderChunk::Usage { cached_tokens, .. } => Some(cached_tokens),
                    _ => None,
                })
                .expect("a usage chunk");
            assert_eq!(reported, expected, "usage {usage}");
        }
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

    // A body that stops in the middle of an event is what a dying upstream
    // channel or a cut proxy connection looks like, so the error must stay
    // restartable instead of being reported as a rejected request.
    #[tokio::test]
    async fn a_truncated_event_is_a_retryable_error() {
        let items = drain(state_with_chunks(vec![
            "data: {\"choices\":[{\"delta\":{\"content\":\"half an ans",
        ]))
        .await;
        assert_eq!(items.len(), 1, "{items:?}");
        match items[0].as_ref().unwrap() {
            ProviderChunk::Error { message, retryable } => {
                assert!(*retryable, "a cut body must be retryable: {message}");
                assert!(message.contains("malformed JSON"), "{message}");
            }
            other => panic!("unexpected chunk: {other:?}"),
        }
    }
}
