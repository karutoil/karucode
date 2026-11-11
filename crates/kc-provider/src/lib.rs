//! OpenAI-compatible chat-completions provider with streamed text and tool calls.
//!
//! Scope: `POST {base_url}/chat/completions` with `stream: true`, SSE parsing,
//! tool-call delta accumulation, request timeouts, cancellation and redacted
//! error messages. Native Anthropic/Gemini providers can be added later by
//! implementing [`kc_core::provider::Provider`]; the runtime does not care.

// @kc-doc: provider-profiles
// @kc-title: Configure a model provider
// @kc-summary: Point karucode at any OpenAI-compatible endpoint, by environment or as named profiles, and choose the active model.
// @kc-tags: provider, model, config, setup, gateway, llm
// @kc-applies-to: env:KARUCODE_BASE_URL, env:KARUCODE_API_KEY, env:KARUCODE_MODEL, env:OPENAI_BASE_URL, env:OPENAI_API_KEY, env:KARUCODE_TEMPERATURE, env:KARUCODE_MAX_TOKENS, env:KARUCODE_REQUEST_TIMEOUT_SECS, env:KARUCODE_MODELS_DEV_URL, env:KARUCODE_RETRY_MAX_RETRIES, env:KARUCODE_RETRY_BASE_DELAY_MS, env:KARUCODE_RETRY_MAX_DELAY_MS, env:KARUCODE_RETRY_MAX_STREAM_RESTARTS, env:KARUCODE_MODEL_SUPPORTS_IMAGES, config:[[providers]], config:active_provider, config:active_model, config:[model], config:model.supports_images, config:vision, config:retry_max_retries, config:retry_base_delay_ms, config:retry_max_delay_ms, config:retry_max_stream_restarts, config:aggregator, config:limits.max_turn_retries, config:limits.turn_retry_delay_ms, event:stream_retry, event:text_replaced, route:GET /api/config, route:PATCH /api/config, route:POST /api/provider/test, route:GET /api/models/catalog, request:messages[].name, internal:kc-runtime:context-digest, internal:kc-runtime:startup-recovery, ui:Settings
//
// # Fastest path: three environment variables
//
// ```bash
// export KARUCODE_BASE_URL=http://127.0.0.1:4000/v1   # the API root, not /chat/completions
// export KARUCODE_API_KEY=sk-...                      # omit for keyless local servers
// export KARUCODE_MODEL=claude-sonnet
// cargo run --release -p kc-server
// ```
//
// Anything that speaks `POST {base_url}/chat/completions` with streaming tool
// calls works: LiteLLM, vLLM, Ollama (`http://127.0.0.1:11434/v1`), llama.cpp
// (`http://127.0.0.1:8080/v1`), OpenRouter, or a hosted vendor API. There is no
// vendor-specific client to install.
//
// # Named profiles in config.toml
//
// More than one endpoint, or several models behind one gateway, belongs in the
// config file (`$KARUCODE_CONFIG`, default `~/.config/karucode/config.toml`):
//
// ```toml
// active_provider = "gateway"
// active_model = "claude-sonnet"
//
// [[providers]]
// id = "gateway"                 # 1-64 chars: letters, digits, '.', '_', '-'
// name = "LiteLLM gateway"
// provider = "openai_compatible"
// base_url = "http://127.0.0.1:4000/v1"
// api_key = "sk-litellm"
// request_timeout_secs = 300
//
//   [[providers.models]]
//   id = "claude-sonnet"         # the id sent to the endpoint
//   name = "Claude Sonnet"       # label; falls back to the id
//   context_window = 200000
//   max_output_tokens = 8192
//   tool_call = true
//   reasoning = true
//   vision = true                # this model accepts image input
//   cost_input = 3.0             # USD per million input tokens
//   cost_output = 15.0
//
// [[providers]]
// id = "local-vllm"
// name = "vLLM on this box"
// base_url = "http://127.0.0.1:8000/v1"
//
//   [[providers.models]]
//   id = "Qwen/Qwen2.5-Coder-32B-Instruct"
//   context_window = 32768
//   tool_call = true
// ```
//
// * `context_window` is not decoration: it decides how much conversation a run
//   may send (`window - max_output_tokens`, at four characters per token). A
//   model added by hand without it falls back to `limits.context_max_chars`,
//   so fill it in - models picked from the catalog already have it. Flat
//   `[model]` context/output limits are inherited only by synthesized
//   default/environment entries and by a legacy profile with no model entries
//   when resolving the flat `model.model` id; they are not lent to a different
//   declared model whose own metadata is absent.
// * `max_output_tokens` is sent on the wire as `max_tokens`, so the room the
//   budget reserves for the answer is actually enforced. `max_tokens` wins when
//   both are set; only one is ever sent. Newer OpenAI models reject `max_tokens`
//   in favour of `max_completion_tokens` - set that through
//   `[model.extra_body] max_completion_tokens = 8192` and it replaces ours
//   rather than being sent alongside it.
// * A remote `base_url` with no `api_key` fails immediately with a
//   configuration error instead of sending an unauthenticated request and
//   retrying the resulting 401. Loopback endpoints (`127.0.0.1`, `::1`,
//   `localhost`) are exempt, because local servers are keyless by design.
// * `active_provider` / `active_model` are install-wide: every project and
//   session uses the same model.
// * `temperature`, `max_tokens`, `top_p` and `parallel_tool_calls` stay global in
//   `[model]`, so switching profiles does not change sampling.
// * **No temperature is sent unless you ask for one** (`send_temperature = true`).
//   A sampling value the model was not tuned for can push a reasoning model tool
//   into repeating itself instead of acting: measured on a real 140-message
//   session, at the turn where the model was about to write a file, sending
//   `temperature: 0.2` produced 8,000 tokens of reasoning and no tool call, while
//   omitting the field produced the `write_file` call. pi sends none for the same
//   model; karucode now matches that, and `temperature` is only a value kept for
//   the operators who turn sending on.
// * With `[[providers]]` present, the connection fields of `[model]`
//   (`base_url`, `api_key`, `request_timeout_secs`, `extra_headers`, `extra_body`)
//   are ignored; without profiles, a `default` profile is synthesised from
//   `[model]`.
// * An explicit `Authorization` entry in `extra_headers` (or a request-specific
//   override) wins over `api_key`; the key supplies `Bearer <key>` only when no
//   explicit Authorization header was configured.
// * `vision = true` on a model entry (or `supports_images = true` in `[model]`,
//   or `KARUCODE_MODEL_SUPPORTS_IMAGES=1`) tells karucode the model accepts image
//   input. Nothing guesses it from the model id. With it on, `read_file` on a
//   PNG/JPEG/GIF/WebP attaches the pixels to the tool result; with it off, the
//   model is told plainly that it cannot see images, which is far better than a
//   text-only model inventing a description. A per-model `vision` entry wins over
//   the install-wide flag, so a session pinned to a vision model works even when
//   the default model is text-only: the flag is resolved for the model the
//   session will actually call, not for the profile's default model.
//
//   The settings screen prefills `vision` from models.dev when a model is picked,
//   added by id, or refreshed: `true` only when `modalities.input` contains
//   `image`. Plain `attachment: true` is not enough - models.dev sets it for
//   audio- and pdf-only models too - so it is only a fallback for a mirror with
//   no `modalities` at all. The field stays editable (unknown / accepted / text
//   only), because a gateway may serve a vision model under a text-only id.
// * Limits: 16 profiles, 64 models per profile, unique model ids per profile,
//   `base_url` must be `http(s)://` without embedded credentials, model ids at
//   most 200 characters. `PATCH /api/config` enforces the same rules and returns
//   `400` naming the offending field.
//
// # Which layer wins
//
// 1. values saved from the UI (SQLite `settings`) - unless
//    `KARUCODE_LOCK_SETTINGS=1`;
// 2. environment variables;
// 3. the config file;
// 4. built-in defaults.
//
// Setting `KARUCODE_MODEL`, `KARUCODE_BASE_URL` or `KARUCODE_API_KEY` (or the
// `OPENAI_*` fallbacks) expresses them as a reserved `env` profile and selects
// it, so an env-driven deployment never silently keeps using a file profile.
// `KARUCODE_REQUEST_TIMEOUT_SECS`, when present, overrides `request_timeout_secs`
// on every loaded file profile as well as the synthesized env profile, including
// session-pinned models; by itself it does not select another endpoint. Flat
// `[model].request_timeout_secs` is still ignored when named profiles exist.
// Keys are write-only: `GET /api/config` reports `api_key_set`, never the key.
//
// # Timeouts: two deadlines from one setting
//
// `request_timeout_secs` (default 300, per profile or `[model]`) answers two
// different questions, and both are enforced:
//
// | deadline | what it bounds | what it catches |
// |---|---|---|
// | **per-read gap**, `request_timeout_secs` | the silence *between* two reads | an endpoint that stops talking: a dead backend, a stalled proxy |
// | **whole attempt**, twice `request_timeout_secs` | one absolute deadline set before send, shared by header waits, buffered error/JSON reads and the streamed body | an endpoint that never stops talking: a body that trickles a byte at a time, a stream that sends nothing but keepalive comments |
//
// Header latency consumes the whole-attempt budget; starting the body does not
// reset it. Both buffered reads and body streaming also observe cancellation.
// Both deadlines are needed. A per-read timeout cannot see a drip-fed body, because every
// individual read returns in time; measured, a 5 s setting let a drip-fed body
// run for 42 s and kept a keepalive-only stream alive indefinitely. The whole
// attempt is a multiple of the gap rather than a separate key so there is one
// number to understand and one number to tune; the ceiling is generous on
// purpose (twice the gap, so ten minutes at the 300 s default) because a slow
// reasoning model must still fit inside it.
//
// Exceeding either deadline is a **retryable** timeout: the endpoint did not
// answer, which says nothing bad about the request, so the retry layer is free
// to try another channel. Configuration for it lives with the profile:
//
// ```toml
// [[providers]]
// request_timeout_secs = 300   # read gap; the whole attempt gets twice this
// ```
//
// The retry layer has its own separate wall clock, covered below.
//
// # Retries, and why a broken answer no longer fails the run
//
// A gateway dying mid-request is routine: a dying backend answers 502, a
// rate-limited channel 429, a broken proxy 200 with an HTML splash, and a stream
// can stop mid-body with `error decoding response body`. Two budgets cover that,
// both global (`[model]`, not per profile):
//
// * `retry_max_retries` (default 50, `KARUCODE_RETRY_MAX_RETRIES`) covers
//   attempts that fail **before the first chunk**. Backoff starts at
//   `retry_base_delay_ms` (default 500), doubles per attempt with jitter, and is
//   capped by `retry_max_delay_ms` (default 30000).
// * `retry_max_stream_restarts` (default 3,
//   `KARUCODE_RETRY_MAX_STREAM_RESTARTS`) covers the harder case: the answer
//   *started* streaming and then the stream died. Replaying it would duplicate
//   text you already saw, so karucode discards the partial attempt (nothing of
//   it is persisted), reports `stream_retry`, and re-issues the identical
//   request. Each restart is a full generation, which is why this budget is
//   small. `0` disables it, and a mid-answer death then fails the turn; when
//   the budget runs out the last partial text is kept and the run fails with
//   the upstream reason.
//
// Both of those spend attempts in the same second. A third layer spends **time**:
// `[limits] max_turn_retries` (default 2) asks the same turn again after
// `[limits] turn_retry_delay_ms` (default 30000, doubling), for a failure that
// only a wait can fix. It is documented under `configuration`
// (`docs/internal/configuration.md`) because it bounds a run rather than one
// request; the classification it uses is the one below.
//
// ## What is retried, and what is not
//
// The attempt count is not the whole story: a request the endpoint has already
// judged is not fixed by re-sending it. Classification is by HTTP status, plus
// three cases that carry no usable status:
//
// | status | retried | why |
// |---|---|---|
// | 408, 409, 425, 429 | yes | the endpoint is busy or the request raced |
// | 5xx | yes | the upstream is failing, not the request |
// | 400, 413, 422 | **no** | a malformed or oversized request stays malformed |
// | 401, 403, 404 | **no**, unless `aggregator = true` | a revoked key or a missing model id is permanent for a direct vendor API |
// | other 4xx | **no** | the endpoint refused the request |
// | any status whose *body* names a rate limit, cooldown or reset window | yes | see "A rate limit wearing the wrong status" below |
// | 3xx redirect | **no** | no redirected/repeated POST: update `base_url` explicitly |
// | 200 + `application/json` | **no** | a buffered completion or an error object (`insufficient_quota`); the body's reason is surfaced |
// | 200 + empty body (including close-delimited EOF) | **no** | the endpoint answered "nothing", and will answer it again |
// | 200 + HTML/plain splash | yes | a broken proxy; the next attempt lands on a healthy channel |
// | in-stream `retryable: false` | **no** | the endpoint judged the request mid-answer (`invalid_request_error`, a context overflow) |
// | `data: [DONE]`, with or without another finish reason | n/a | a normal stop; keepalive/usage/role-only JSON does not make it `eof` |
//
// The permanent responses matter because each retry is a billed generation. A 200
// with an error body used to be a generic provider error - no status to classify, so
// transient - and was re-sent up to 51 times; a completion delivered only as
// `[DONE]` was read as "the stream died before sending anything" and regenerated
// four times for one empty turn. Both now end the turn with the real reason.
//
// Transport failures (a timeout, a connection reset, a body that dies
// mid-decode) are always retried. A context overflow is never retried: the
// runtime has to compact first (see below).
//
// ## A rate limit wearing the wrong status
//
// The status is the first word, not the last. Gateways routinely put a
// condition that clears by itself inside a status that means "your request is
// wrong", and reading only the status ended runs over a two-minute window:
//
// ```text
// [400] {"code":"bad_request","message":"[code=tier_limit_exceeded]
//        membership rpm limit exceeded: 101/100 resets 2026-11-09T22:00:00Z"}
// [402] no provider's ask matches your max-per-mtok bid (reset after 2m)
// [403] ... no subscription quota or balance available ... (reset after 3s)
// ```
//
// A body naming a rate limit, a cooldown, "temporarily unavailable", an
// upstream that is busy, or a reset/retry window is retried **whatever status
// carried it** - including in-stream, where the same wording is the only verdict
// a gateway gives. Two verdicts still win over the wording: an explicit
// permanent tag (a buffered 200 error body) and a context overflow, because both
// are judgements about *this* request. Quota and balance wording is deliberately
// not in the list: `insufficient_quota` needs a recharge, and 50 retries are 50
// billed refusals.
//
// ## The provider's own reset window
//
// A cooldown is not a backoff. When the body carries `"reset_seconds": 1024`,
// `retry after 30s` or `reset after 2m`, that window is used instead of the
// exponential:
//
// * it fits the remaining `retry_max_total_secs` budget -> the attempt waits it
//   out, and a two-minute bid floor no longer fails a run that had five minutes
//   to spend;
// * it does not -> the attempt fails at once with
//   `the provider asked for a 1024s wait, longer than the 300s retry budget
//   left`, instead of 18 billed attempts crawling a 30 s backoff towards a reset
//   it can never reach.
//
// 401/403/404 are the one judgement call. Behind a multi-channel aggregator they
// *are* transient - a rotated upstream key, a channel in failover - and retrying
// them is what makes such a gateway usable. Set `aggregator = true` on the
// profile (`[model]` for the flat connection, or `[[providers]]` per endpoint) -
// or flip **Multi-channel gateway** on that profile's card in Settings -> Models
// & providers, which writes the same field. Turn it on when the endpoint in front
// of you fronts several upstream channels: its own body will say so ("No active
// credentials for provider: t3-web", "User not found"), and another channel
// usually serves the same model. Left off, those statuses fail immediately, which
// is the right reading of a direct vendor API.
//
// ## The wall-clock budget
//
// The attempt count alone cannot bound a run: 50 retries against a 30 s backoff
// ceiling is ~20 minutes of a hung turn. `retry_max_total_secs` (default 300)
// per stream stops the chain even when attempts remain, and the real error is
// surfaced instead.
//
// ```toml
// [model]
// retry_max_retries = 50          # attempts = retries + 1; 0 disables
// retry_base_delay_ms = 500       # first delay; doubles per attempt (jittered)
// retry_max_delay_ms = 30000      # backoff ceiling for both layers
// retry_max_total_secs = 300      # wall clock for the whole retry chain
// retry_max_stream_restarts = 3   # restarts of a stream that died mid-answer
// ```
//
// # A prompt that does not fit the window
//
// When the endpoint rejects a request because the conversation is too long
// (`maximum context length`, `context_length_exceeded`, `too many tokens`,
// `reduce the length`, `prompt is too long`), the provider tags the error as a
// context overflow (`kc_provider::CONTEXT_OVERFLOW_TAG`) instead of reporting a
// generic 400. The runtime is expected to catch that tag, compact the
// conversation and retry once. Until it does, the turn fails with the upstream
// message - which is at least the real reason, not a retry storm.
//
// # DeepSeek writes tool calls as text, and karucode recovers them
//
// DeepSeek-V4 models are trained on their own markup language, DSML, and
// sometimes write a tool call into the ordinary answer instead of the
// `tool_calls` field:
//
// ```text
// Looking at the workspace.<||DSML||tool_calls><||DSML||invoke name="list_dir"><||DSML||parameter name="path" string="true">.</||DSML||parameter></||DSML||invoke></||DSML||tool_calls>
// ```
//
// An endpoint that understands DSML converts that into a structured call. One
// that does not - a plain OpenAI-compatible gateway, or DeepSeek's own API for a
// model that emitted the degraded `||DSML||` spelling instead of the full-width
// `｜DSML｜` - forwards the markup as prose. The tool then never runs and the
// model looks broken: it "answered" about a file instead of reading it. This is
// upstream behaviour of the model, not a karucode bug, so karucode repairs it
// in the provider layer instead of asking you to switch endpoints.
//
// What is recovered:
//
// * the block is turned into real tool calls, so the loop runs the tool exactly
//   as if the endpoint had sent `tool_calls`, and its results come back to the
//   model on the next turn;
// * `string="true"` parameters are taken literally and every other parameter is
//   parsed as JSON (a number stays a number, a boolean a boolean); a missing
//   `string` attribute is treated the same way, falling back to the literal text
//   when the value is not valid JSON;
// * markers split across network chunks are reassembled, and no half-marker is
//   ever rendered as text and then retracted;
// * marker-shaped text inside a literal parameter remains argument data, not a
//   nested invoke or closing block. Balanced invoke/parameter/close markup stays
//   verbatim in the outer value; ambiguous or unbalanced nesting fails closed as
//   prose. Malformed block tags never borrow a later `>` and become calls;
// * several invokes in one block become several calls, and a turn that carries
//   both a real `tool_calls` field and a text block keeps them apart.
//
// What is **not** recovered, on purpose:
//
// * **an invoke naming a tool this run could not call.** All invokes in a block
//   must name a tool that was offered in that request, or the whole markup stays
//   in the transcript as text and a warning is logged. A model explaining the
//   format ("write your call as `<||DSML||tool_calls>`") must never fire a tool,
//   and nothing is guessed from the model id.
// * a turn with no tools offered: there is nothing to call, so the markup is
//   prose.
// * a block the model never closed. A stream cut off mid-call shows what
//   arrived rather than inventing a partial call.
//
// There is no setting: the recovery only engages on markup the model actually
// emitted, and a model that never writes DSML is unaffected. A self-hosted vLLM
// or gateway that already parses DSML itself keeps working - it sends
// `tool_calls`, and the recovery never sees the markup.
//
// # A stream that degenerates into a repetition loop
// Some models occasionally stop producing language and start repeating one
// block - `Let me write. Go. Now. OK.` over and over - usually inside the
// reasoning channel, where a turn can burn its whole output budget without
// writing a word of content. The runtime watches both channels for a *cycle* at
// the end of the stream: the same block, back to back, with nothing new between
// the copies. Whitespace is compared collapsed, so a model that alternates
// `Let me write.` and `Let me write.\n\nGo.` is still one cycle.
//
// * **reasoning** that loops is abandoned and the same request is re-issued under
//   `retry_max_stream_restarts` (nothing of that attempt was persisted, so
//   nothing is duplicated). `stream_retry` carries the reason starting with
//   `the model's reasoning repeated`.
// * **answer text** that loops is cut back to where the run starts and the turn
//   completes, because the model did produce something worth keeping; the
//   repeated tail is never persisted, so it cannot become the model's own habit
//   on the next request. After the older delta events, the runtime also emits
//   `text_replaced` with the full final trimmed content so live UI text matches
//   the persisted answer. Fenced code is never compared, so a repetitive program
//   is safe.
//
// The bar is deliberately high, because a false positive throws away a good
// answer or a whole attempt:
//
// * reasoning needs **8** copies, answer text **10**;
// * the block may be up to **512** characters, short enough that a stuck model is
//   caught in its first few hundred characters;
// * the whole run must be at least **240** characters, and **1024** when the block
//   is one character repeated, so a markdown rule or a border of `=` is not a loop;
// * copies must be adjacent. A model that weighs up ten gateways serving the same
//   model repeats `deepseek-v3.2-flash-1017` ten times while every provider is
//   new, and that is not a cycle: the text between the copies differs. Counting a
//   repeated n-gram instead - which this detector used to do - turned that
//   ordinary reasoning into a "loop", abandoned the attempt and eventually failed
//   the run.
//
// A cancellation is never retried, and neither is a configuration error: no
// attempt count fixes an empty `base_url` or an unsendable header.
//
// # Model metadata without typing it
//
// The settings screen can prefill context window, limits, tool-calling support
// and cost from <https://models.dev>. The server fetches and caches that catalog
// itself (`GET /api/models/catalog?provider=...`, six-hour cache, `refresh=true`
// to force). Point `KARUCODE_MODELS_DEV_URL` at a mirror or an air-gapped copy - it
// must be `http(s)://`, is read once at startup, and a failed fetch only makes the
// catalog report `ok: false`; nothing else in the settings path depends on it.
//
// # Verify it works
//
// ```bash
// curl -s localhost:8787/api/health | jq          # active model + base_url
// curl -s -X POST localhost:8787/api/provider/test \
//      -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//      -d '{"profile":"gateway"}'                 # live probe, lists models
// ```
//
// A probe failure is returned as `ok: false` with a redacted reason, not a 500.
// Press the refresh button in Settings (or `PATCH /api/config` with the active
// provider/model) to switch models without restarting the server.
//
// # Gotchas
//
// * karucode never sends `tool_choice`. Explicit `tool_choice: "auto"` was
//   measured pushing time-to-first-token to 9-30 s on some gateways. Use
//   `[model.extra_body] tool_choice = ...` only if a gateway requires it.
// * The local-only message labels `kc-runtime:context-digest` and
//   `kc-runtime:startup-recovery` are omitted from OpenAI `messages[].name`,
//   whose grammar does not admit `:`. Their role and content still reach the
//   model and the stored transcript keeps the labels; ordinary task, participant
//   and tool names are still serialized unchanged.
// * `base_url` is the prefix, not the full URL: karucode appends
//   `/chat/completions` (and `/models` for a probe).
// * A model that does not support tool calls will answer, but it cannot edit
//   files or run commands; set `tool_call = false` in the model entry so the UI
//   warns instead of pretending.
// * Provider settings are not per project. Switching project does not switch
//   model.
//
// Full reference: `README.md` (Providers) and `docs/ARCHITECTURE.md`
// (Provider profiles and resolution), `docs/API.md` (config routes).

use async_trait::async_trait;
use futures_util::StreamExt;
use kc_core::config::ModelConfig;
use kc_core::error::{Error, Result};
use kc_core::provider::{ChatRequest, Provider, ProviderFactory, ProviderStream};
use reqwest::header::{HeaderMap, CONTENT_TYPE};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod dsml;
mod errors;
pub mod request;
pub mod retry;
pub mod sse;
mod stream;

pub use dsml::strip_tool_markup;
pub use errors::{
    is_context_overflow, is_context_overflow_error, is_transient_refusal, retry_after_hint,
    CONTEXT_OVERFLOW_TAG,
};
pub use request::{
    build_chat_body, build_chat_body_for, chat_completions_url, models_url,
    serialized_message_count,
};
pub use retry::{RetryPolicy, RetryingProvider};

/// TCP/TLS handshake budget; the timeout of the request itself is derived from
/// `ModelConfig::request_timeout_secs` (see [`attempt_timeout`]).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Ceiling for a `/models` response body.
const MODELS_BODY_LIMIT: usize = 1024 * 1024;
const MIN_TIMEOUT_SECS: u64 = 5;
/// How long one whole attempt may take, as a multiple of the per-read gap.
///
/// `request_timeout_secs` alone bounds the gap *between* reads, so an endpoint
/// that drips one byte at a time (or sends nothing but SSE keepalives) can hold
/// an attempt open indefinitely: measured at 42 s and 16 s against a 5 s
/// setting. The whole attempt therefore gets its own deadline, which is what the
/// setting's name and the settings screen ("Request timeout") have always
/// claimed.
///
/// Two is deliberate rather than tight: a real generation has to fit inside one
/// attempt, and a slow reasoning model can legitimately stream for minutes,
/// while an attempt that has produced nothing but keepalives for twice the
/// per-read budget is not going to finish. At the 300 s default that is a
/// ten-minute ceiling for one attempt; the retry layer's own wall clock
/// (`retry_max_total_secs`) still bounds the chain.
const ATTEMPT_GAP_MULTIPLIER: u32 = 2;

pub struct OpenAiProvider {
    pub config: ModelConfig,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(config: ModelConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            // A redirected POST can repeat a billed generation and forward
            // credentials to another origin. Make the operator update base_url
            // deliberately; the 3xx response is classified as permanent below.
            .redirect(reqwest::redirect::Policy::none())
            // A per-read timeout keeps long generations alive while still
            // failing when the endpoint goes silent. The whole attempt is
            // bounded separately, by `attempt_timeout`, because a per-read gap
            // cannot see a body that keeps trickling.
            .read_timeout(timeout_for(&config))
            .user_agent(concat!("karucode/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Provider(format!("http client: {e}")))?;
        Ok(Self { config, client })
    }

    /// Build with a caller-provided client (custom proxy, TLS roots or pool).
    pub fn with_client(config: ModelConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    /// Full chat-completions URL for the configured base.
    pub fn endpoint(&self) -> Result<String> {
        chat_completions_url(&self.config.base_url)
    }

    fn secrets(&self, headers: &HeaderMap) -> Vec<String> {
        let mut secrets = request::sensitive_values(headers);
        if let Some(key) = self.api_key() {
            secrets.push(key.to_string());
        }
        secrets
    }

    fn api_key(&self) -> Option<&str> {
        self.config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
    }
}

fn timeout_for(config: &ModelConfig) -> Duration {
    Duration::from_secs(config.request_timeout_secs.max(MIN_TIMEOUT_SECS))
}

/// Wall-clock budget for **one attempt**, header wait and body together.
///
/// Two distinct deadlines come out of the single `request_timeout_secs` setting,
/// because they answer two different questions:
///
/// * the **per-read gap** ([`timeout_for`]) asks "did the endpoint go silent?",
///   and is what keeps a long generation alive;
/// * the **whole attempt** (this) asks "is this request ever going to finish?",
///   and is what stops a body that trickles - or a stream that sends nothing but
///   keepalive comments - from holding the attempt open forever. A per-read
///   timeout never fires in either case, because every individual read does
///   eventually return.
///
/// The attempt deadline is deliberately generous (`ATTEMPT_GAP_MULTIPLIER` times
/// the gap): it exists to bound a pathological stream, not to cut a slow model
/// off mid-answer. Exceeding it is a retryable [`Error::Timeout`]: the endpoint
/// did not answer, which says nothing bad about the request.
fn attempt_timeout(config: &ModelConfig) -> Duration {
    timeout_for(config).saturating_mul(ATTEMPT_GAP_MULTIPLIER)
}

/// Is this base URL a loopback endpoint?
///
/// Local servers (Ollama, vLLM, llama.cpp, the mock servers in the test suite)
/// legitimately run without an API key, so a missing key is only an error for a
/// remote endpoint. The host is parsed out of the URL by hand because the
/// provider crate deliberately has no URL-parsing dependency, but it is then
/// validated as a real IP literal: a prefix test would accept
/// `127.0.0.1.evil.example`, which is a remote host.
pub fn is_loopback_base_url(base_url: &str) -> bool {
    let Some((_, rest)) = base_url.trim().split_once("://") else {
        return false;
    };
    // Drop path/query/fragment, then any `user:pass@` userinfo.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(rest) = host_port.strip_prefix('[') {
        // IPv6 literal: `[::1]:8080`.
        let host = rest.split(']').next().unwrap_or_default();
        return host
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|addr| addr.is_loopback());
    }
    let host = host_port.split(':').next().unwrap_or_default();
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|addr| addr.is_loopback() || addr.is_unspecified())
}

/// Fail fast when a remote endpoint has no API key.
///
/// Without this the request goes out unauthenticated, the endpoint answers 401,
/// and the retry layer used to re-send it 51 times before the user saw anything.
/// [`Error::Config`] is never retried, so the run stops immediately with a
/// message that names the actual problem.
pub fn validate_credentials(config: &ModelConfig) -> Result<()> {
    let has_key = config
        .api_key
        .as_deref()
        .map(str::trim)
        .is_some_and(|key| !key.is_empty());
    if has_key || is_loopback_base_url(&config.base_url) {
        return Ok(());
    }
    Err(Error::Config(format!(
        "no API key configured for the remote provider at `{}`: set model.api_key \
         (KARUCODE_API_KEY) for this endpoint, or point base_url at a loopback address \
         (127.0.0.1, ::1, localhost) for a keyless local server",
        config.base_url.trim()
    )))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai_compatible"
    }

    async fn stream_chat(
        &self,
        request: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ProviderStream> {
        let url = self.endpoint()?;
        let mut body = request::build_chat_body_for(&self.config, &request);
        request::apply_config_extra_body(&mut body, &self.config.extra_body);
        let headers = request::build_headers(&self.config, &request)?;
        let secrets = self.secrets(&headers);
        let timeout = timeout_for(&self.config);
        let attempt = attempt_timeout(&self.config);
        // One absolute clock covers send, headers, buffered bodies and streaming.
        // The body must never receive a fresh `attempt` after slow headers.
        let started = tokio::time::Instant::now();
        let deadline = started + attempt;
        let header_deadline = (started + timeout).min(deadline);

        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let send = self.client.post(&url).headers(headers).json(&body).send();
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            outcome = tokio::time::timeout_at(header_deadline, send) => match outcome {
                Err(_) => {
                    return Err(Error::Timeout(format!(
                        "provider at {url} sent no response headers within {}s",
                        timeout.as_secs()
                    )));
                }
                Ok(Err(err)) => return Err(errors::map_reqwest_error(&url, &err, timeout, &secrets)),
                Ok(Ok(response)) => response,
            },
        };

        let status = response.status();
        if !status.is_success() {
            // The error body is read here, so it needs the same deadline: a
            // gateway that drips an error page must not hold the attempt either.
            let error =
                errors::http_status_error(response, &url, &self.config.model, &secrets, false);
            return Err(buffered_error_with_cancel(
                error,
                &cancel,
                deadline,
                format!(
                    "provider at {url} did not finish sending its {status} response within {}s",
                    attempt.as_secs()
                ),
            )
            .await);
        }
        if status == reqwest::StatusCode::NO_CONTENT || response.content_length() == Some(0) {
            // A 200/204 with no body is not a transient failure: the endpoint
            // answered, and it answered with nothing. Re-sending produces the
            // same nothing, forty-nine more times, each one billed.
            return Err(errors::permanent(Error::Provider(format!(
                "provider at {url} returned an empty response body (status {status}): \
                 check that model `{}` supports streaming, and that a proxy in front of it \
                 is not dropping the body",
                self.config.model
            ))));
        }
        // A JSON document is not an event stream: either the endpoint ignored
        // `stream: true` and buffered a whole completion, or a gateway is
        // reporting a *rejection* with a 200. Both are permanent - the identical
        // request gets the identical answer - and in the second case the body is
        // the only place the reason appears (`insufficient_quota`, a model that
        // does not exist), so it is read and surfaced instead of being retried as
        // if it were a dying channel.
        //
        // An HTML or plain-text body is deliberately left alone: a broken proxy
        // answering 200 with a splash page *is* a transient channel death, which
        // is what the retry layer exists for.
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.starts_with("application/json") {
            let error = errors::non_stream_success_error(
                response,
                &url,
                &self.config.model,
                &content_type,
                &secrets,
            );
            return Err(buffered_error_with_cancel(
                error,
                &cancel,
                deadline,
                format!(
                    "provider at {url} did not finish sending its {content_type} response \
                     within {}s",
                    attempt.as_secs()
                ),
            )
            .await);
        }

        Ok(stream::stream_from_response(
            response.bytes_stream().boxed(),
            cancel,
            url,
            timeout,
            attempt,
            deadline,
            secrets,
            request.tools.iter().map(|tool| tool.name.clone()).collect(),
        ))
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        let url = models_url(&self.config.base_url)?;
        let headers = request::build_headers_with(&self.config, "application/json", &[])?;
        let secrets = self.secrets(&headers);
        let timeout = timeout_for(&self.config);
        let attempt = attempt_timeout(&self.config);
        let started = tokio::time::Instant::now();
        let deadline = started + attempt;
        let header_deadline = (started + timeout).min(deadline);

        let send = self.client.get(&url).headers(headers).send();
        let response = match tokio::time::timeout_at(header_deadline, send).await {
            Err(_) => {
                return Err(Error::Timeout(format!(
                    "provider at {url} sent no response within {}s",
                    timeout.as_secs()
                )));
            }
            Ok(Err(err)) => return Err(errors::map_reqwest_error(&url, &err, timeout, &secrets)),
            Ok(Ok(response)) => response,
        };
        if !response.status().is_success() {
            let error =
                errors::http_status_error(response, &url, &self.config.model, &secrets, true);
            return Err(match tokio::time::timeout_at(deadline, error).await {
                Ok(error) => error,
                Err(_) => Error::Timeout(format!(
                    "provider at {url} did not finish sending its error response within {}s",
                    attempt.as_secs()
                )),
            });
        }
        // A model list is a small document, but a drip-fed one is unbounded in
        // time exactly like a stream is, so the whole read gets the attempt
        // deadline too.
        let read = read_limited(response, &url, MODELS_BODY_LIMIT, timeout, &secrets);
        let body = match tokio::time::timeout_at(deadline, read).await {
            Ok(body) => body?,
            Err(_) => {
                return Err(Error::Timeout(format!(
                    "provider at {url} did not finish sending its model list within {}s",
                    attempt.as_secs()
                )));
            }
        };
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            Error::Provider(format!(
                "provider at {url} returned an unreadable model list: {e}"
            ))
        })?;
        parse_model_ids(&value).ok_or_else(|| {
            Error::Provider(format!(
                "provider at {url} returned no model list (expected `{{\"data\":[{{\"id\":\"...\"}}]}}` or a top-level array)"
            ))
        })
    }
}

/// Finish a bounded buffered error/JSON read without losing the user's cancel
/// signal or extending the attempt past the absolute deadline set before send.
async fn buffered_error_with_cancel(
    read: impl std::future::Future<Output = Error>,
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
    timeout_message: String,
) -> Error {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Error::Cancelled,
        outcome = tokio::time::timeout_at(deadline, read) => match outcome {
            Ok(error) => error,
            Err(_) => Error::Timeout(timeout_message),
        },
    }
}

async fn read_limited(
    mut response: reqwest::Response,
    url: &str,
    limit: usize,
    timeout: Duration,
    secrets: &[String],
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    while buf.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = limit - buf.len();
                buf.extend_from_slice(&chunk[..room.min(chunk.len())]);
            }
            Ok(None) => break,
            Err(err) => {
                return Err(errors::map_reqwest_error(url, &err, timeout, secrets));
            }
        }
    }
    Ok(buf)
}

fn parse_model_ids(value: &serde_json::Value) -> Option<Vec<String>> {
    let mut ids: Vec<String> = Vec::new();
    let mut push = |entry: &serde_json::Value| {
        let id = match entry {
            serde_json::Value::String(id) => Some(id.as_str()),
            other => other.get("id").and_then(|id| id.as_str()),
        };
        if let Some(id) = id {
            if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
                ids.push(id.to_string());
            }
        }
    };
    let mut found = false;
    if let Some(array) = value.as_array() {
        found = true;
        for entry in array {
            push(entry);
        }
    }
    for key in ["data", "models"] {
        if let Some(array) = value.get(key).and_then(|v| v.as_array()) {
            found = true;
            for entry in array {
                push(entry);
            }
        }
    }
    if found {
        Some(ids)
    } else {
        None
    }
}

pub struct ProviderFactoryImpl;

impl Default for ProviderFactoryImpl {
    fn default() -> Self {
        Self
    }
}

#[async_trait]
impl ProviderFactory for ProviderFactoryImpl {
    async fn build(&self, config: &ModelConfig) -> Result<Arc<dyn Provider>> {
        // Fail fast on a remote endpoint with no key: the request would go out
        // unauthenticated, come back 401, and (before the retry classification
        // was fixed) be re-sent 51 times. `Error::Config` is never retried.
        validate_credentials(config)?;
        let provider: Arc<dyn Provider> = Arc::new(OpenAiProvider::new(config.clone())?);
        // Every run goes through the retry layer: the endpoints this talks to
        // fail transiently in ways a single attempt cannot survive — a dropped
        // connection, a transient error status from a rotating channel, a body
        // that dies mid-decode. See `retry` for why retries happen inside the
        // stream, why they stop at the first delivered chunk, and which statuses
        // are permanent.
        Ok(Arc::new(RetryingProvider::new(
            provider,
            RetryPolicy::from_config(config),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_ids_parse_from_every_shape() {
        assert_eq!(
            parse_model_ids(&json!({"data": [{"id": "a"}, {"id": "b"}, {"id": "a"}]})),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            parse_model_ids(&json!({"models": [{"id": "x"}, "y"]})),
            Some(vec!["x".to_string(), "y".to_string()])
        );
        assert_eq!(
            parse_model_ids(&json!([{"id": "m"}])),
            Some(vec!["m".to_string()])
        );
        assert_eq!(parse_model_ids(&json!({"data": []})), Some(vec![]));
        assert_eq!(parse_model_ids(&json!({"unexpected": 1})), None);
    }

    #[test]
    fn empty_api_key_is_treated_as_absent() {
        let config = ModelConfig {
            api_key: Some("   ".into()),
            ..ModelConfig::default()
        };
        let provider = OpenAiProvider::new(config).unwrap();
        assert!(provider.api_key().is_none());

        let config = ModelConfig {
            api_key: Some("sk-x".into()),
            ..ModelConfig::default()
        };
        let provider = OpenAiProvider::new(config).unwrap();
        assert_eq!(provider.api_key(), Some("sk-x"));
    }

    #[tokio::test]
    async fn factory_builds_openai_compatible_providers() {
        let factory = ProviderFactoryImpl;
        assert_eq!(factory.supported(), vec!["openai_compatible".to_string()]);
        let provider = factory.build(&ModelConfig::default()).await.unwrap();
        assert_eq!(provider.id(), "openai_compatible");
    }

    #[test]
    fn loopback_hosts_are_recognised() {
        for url in [
            "http://127.0.0.1:4000/v1",
            "http://127.0.0.1/v1",
            "http://localhost:11434/v1",
            "http://[::1]:8080/v1",
            "http://0.0.0.0:8000/v1",
            "http://user:pass@127.0.0.1:4000/v1",
            "  http://127.0.0.1:4000/v1  ",
        ] {
            assert!(is_loopback_base_url(url), "{url} must be loopback");
        }
        for url in [
            "https://api.openai.com/v1",
            "https://gw.example/v1",
            "http://192.168.1.10:4000/v1",
            "http://127.0.0.1.evil.example/v1",
            "http://localhost.evil.example/v1",
            "not-a-url",
            "",
        ] {
            assert!(!is_loopback_base_url(url), "{url} must not be loopback");
        }
    }

    /// A remote endpoint with no key used to send an unauthenticated request,
    /// get a 401, and retry it 51 times. It must fail fast instead.
    #[tokio::test]
    async fn remote_profile_without_a_key_fails_fast_with_a_config_error() {
        let config = ModelConfig {
            base_url: "https://gw.example/v1".into(),
            api_key: None,
            ..ModelConfig::default()
        };
        let err = match ProviderFactoryImpl.build(&config).await {
            Ok(_) => panic!("a remote endpoint without a key must not build"),
            Err(err) => err,
        };
        assert!(matches!(err, Error::Config(_)), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("API key"), "{text}");
        assert!(text.contains("gw.example"), "{text}");
        // A config error is never retried, so the run stops immediately.
        assert!(!retry::retryable(&err));
    }

    /// A blank key is the same as no key.
    #[tokio::test]
    async fn remote_profile_with_a_blank_key_fails_fast() {
        let config = ModelConfig {
            base_url: "https://gw.example/v1".into(),
            api_key: Some("   ".into()),
            ..ModelConfig::default()
        };
        let err = match ProviderFactoryImpl.build(&config).await {
            Ok(_) => panic!("a blank key must not build a remote provider"),
            Err(err) => err,
        };
        assert!(matches!(err, Error::Config(_)), "{err:?}");
    }

    /// Local servers are keyless by design; the check must not break them.
    #[tokio::test]
    async fn loopback_profile_without_a_key_is_allowed() {
        for base_url in [
            "http://127.0.0.1:4000/v1",
            "http://localhost:11434/v1",
            "http://[::1]:8080/v1",
        ] {
            let config = ModelConfig {
                base_url: base_url.into(),
                api_key: None,
                ..ModelConfig::default()
            };
            let provider = ProviderFactoryImpl
                .build(&config)
                .await
                .unwrap_or_else(|e| panic!("{base_url} must build without a key: {e}"));
            assert_eq!(provider.id(), "openai_compatible");
        }
    }

    #[tokio::test]
    async fn remote_profile_with_a_key_is_allowed() {
        let config = ModelConfig {
            base_url: "https://gw.example/v1".into(),
            api_key: Some("sk-live".into()),
            ..ModelConfig::default()
        };
        assert!(ProviderFactoryImpl.build(&config).await.is_ok());
    }
}
