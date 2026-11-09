//! Context budgeting: truncate tool output and compact the oldest exchanges
//! while keeping the system prompt, the original task and the newest turn.
//!
//! Four steps, applied to the message list actually sent to the model (the
//! persisted transcript is never rewritten):
//!
//! 1. every `tool` message content is capped at `max_tool_output_bytes`;
//! 2. **only when the window is over budget**, a tool result that a later result
//!    in the same window already replaced is retired in place: its body becomes
//!    a one-line [`SUPERSEDED_PREFIX`] stub naming the tool, the path and the
//!    line range, and saying it may be read again (see `supersede`);
//! 3. while the total character budget is exceeded, the oldest droppable
//!    exchange is removed. An assistant message and the tool results that answer
//!    its `tool_calls` are dropped as one group, so the model never sees a tool
//!    result whose call is missing;
//! 4. dropped turns are replaced by one digest notice: a summary of the
//!    user/assistant turns, the tools they called, and the run's own
//!    [notebook](Notebook), bounded by [`DIGEST_MAX_CHARS`] and counted inside
//!    the same budget.
//!
//! Nothing is ever *removed* by step 2: a stubbed `tool` message still answers
//! its `tool_call`, so the request stays valid, and the pinned task and the
//! exchange in flight are never touched.
//!
//! # Why supersession only runs at a compaction
//!
//! A replay of the last 30 real sessions holds 86.9 M characters of tool output.
//! Keeping only the newest result per `(tool, path)` retires 77% of it, because a
//! page of `src/app.tsx` read at turn 3 is worthless once turn 40 read the same
//! file again - and nothing noticed: this module capped a *single* result
//! (`max_tool_output_bytes`) and then dropped whole exchanges at the budget, so a
//! 400-line page read at turn 3 rode along at turn 300. Median input was 189,148
//! tokens per provider call (p90 589,620, p99 1,003,853) largely because of this.
//!
//! The pass is deliberately inside the compaction path and nowhere else. Between
//! compactions the request must stay byte-identical to the previous one (that is
//! the prompt cache, see the `prompt-cache` block below), so rewriting a body on
//! every turn would trade a 77% smaller window for a 0% hit rate. Bounded this
//! way it costs one cache miss per compaction - the same miss compaction already
//! costs - and not one per turn.
//!
//! # The notes channel
//!
//! 8,216 of 11,137 assistant turns in those sessions carried under 40 characters
//! of text: a reasoning model streams its analysis as `reasoning_content`, the
//! runtime shows it and persists nothing, so the transcript keeps tool output and
//! no conclusions - 47.5 tool-output characters ingested per character of written
//! finding. Compaction then deletes the only memory there was.
//!
//! So a run carries a [`Notebook`], and the runtime gives the model one tool
//! (`note`, see [`NoteTool`]) to append a durable finding to it. The notebook is
//! replayed **inside the compaction digest** and nowhere else: it is bounded by
//! the same `note_budget` policy as the sticky written findings, oldest entries
//! released first, and it is never appended to the end of a request - see the
//! measured trailing-note stall in `agent.rs`.
//!
//! # Why the drop point is quantised
//!
//! Providers cache the *prefix* of a request, so a hit requires every byte up to
//! some point to be identical to the previous call. Dropping only what barely
//! does not fit breaks that on every turn: the window grows by one exchange, one
//! more exchange is dropped, and the digest at the front is rewritten. The prefix
//! then changes on *every* call and the cache only covers the system prompt.
//!
//! Measured on a real 807-iteration subagent transcript (replay the stored
//! messages through these functions and diff consecutive requests): median
//! per-call cache hit **1.6%**, 57% of calls under 10%, and 792 of 807 requests
//! carried a digest that had just been rewritten. Compacting with headroom gave
//! a median of **96.3%** at the same `context_max_chars`, and **98.9%** with the
//! budget sized to the model's real window (13 compactions instead of 792).
//!
//! # Why the newest turn and the task are protected
//!
//! Compaction that forgets what the run is *for* is worse than a cache miss: on
//! the same 807-iteration transcript, 722 of 807 requests contained **no user
//! message at all** - the task had been compacted away, and the only thing left
//! describing the job was the digest, written as an `assistant` turn. The model
//! was told, in its own voice, a summary of work it had allegedly already done.
//! That run also re-issued 44 identical calls 24-29 times each (`read_file` on
//! the same five paths). Pinning the first user turn keeps the instruction where
//! the model always looks for it, and the exchange in flight keeps the results it
//! is reasoning about.

use kc_core::message::{Message, Role};
use kc_core::tool::truncate_output;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

// @kc-doc: prompt-cache
// @kc-title: Keep the prompt cache hitting (cheap long runs)
// @kc-summary: Why a long run can re-bill its whole context every turn, what karucode does to keep the cached prefix stable, and how to read the hit rate.
// @kc-tags: cache, caching, cost, context, compaction, tokens, performance, provider
// @kc-applies-to: config:limits.context_max_chars, config:limits.max_context_messages, config:limits.max_tool_output_bytes, config:limits.max_image_read_bytes, config:limits.max_tool_calls_per_turn, env:KARUCODE_CONTEXT_MAX_CHARS, env:KARUCODE_MAX_TOOL_OUTPUT_BYTES, route:GET /api/events, ui:ChatView, tool:note
//
// # What the provider caches
//
// OpenAI-compatible endpoints (and the gateways in front of them) cache the
// *prefix* of a request. A call is billed at the cached rate for every leading
// token that is byte-identical to an earlier call in the same session window,
// and at the full rate from the first byte that differs. Nothing is cached when
// the first message differs, so the cost of a long run is decided by one thing:
// **how early the context changes**.
//
// Verified against a real gateway: appending a turn to an identical prefix hit
// 99.0% of 9,055 prompt tokens; changing a single earlier message hit 0%.
//
// # How karucode keeps the prefix stable
//
// * Between compactions the conversation is **append-only**. Nothing already
//   sent is rewritten: no re-ordering, no re-truncating, no timestamps.
// * The system prompt is rebuilt from the tool registry every iteration, but its
//   content is deterministic (tools are sorted by name), so it is byte-identical.
// * Compaction is **quantised**: it runs only when the window passes one of its
//   two budgets - `limits.context_max_chars` (how much text) or
//   `limits.max_context_messages` (how many messages) - and then cuts the oldest
//   exchanges until the remaining window leaves room for about 50 more turns
//   (never below 15% of either budget). One cache miss per compaction instead of
//   one per turn.
// * The original user turn and the exchange in flight are never compacted, so a
//   long run keeps its goal and the results it is currently reasoning about.
// * **Written findings are sticky.** An assistant turn with at least 200
//   characters of text and no tool calls is kept verbatim across compaction
//   while it fits the note budget (a twentieth of the context window).
// * **A tool result a later call replaced is retired at a compaction.** When the
//   window is over budget, the newest result per `(tool, path)` survives and the
//   earlier ones become one line:
//
//   ```text
//   [superseded: read_file `src/app.tsx` lines 1-400 of 812 was replaced by a later
//   call in this run. Call read_file again if you still need that range.]
//   ```
//
//   Nothing is removed - the `tool` message still answers its call - and the pass
//   never runs between compactions, so the cached prefix stays byte-identical
//   until the next one. Measured over the last 30 sessions, this retires 77% of
//   86.9 M characters of tool output. A result is only replaced by one for the
//   same tool, path *and* question: a `search_files` for a different pattern, or a
//   `git_diff` against another base, is a different answer, not a newer copy of
//   the same one.
//   A stub is never stubbed again: the line range it names cannot be re-derived
//   once the header is gone, so a later compaction would trade `lines 1-400 of
//   812` for a vaguer sentence (caught on a live run).
// * **The model can keep its own notes.** A reasoning model streams its analysis
//   as `reasoning_content`, which is persisted with the turn but still ages out
//   with it: 8,216 of 11,137 assistant turns in those sessions were under 40
//   characters of text, so the prose left in the transcript is thin, and a
//   compaction drops whole exchanges. The `note` tool appends a
//   finding to a per-run notebook, and the notebook is replayed inside the
//   compaction digest, so the finding outlives the messages that carried it.
// * **Nothing is appended after the conversation.** No runtime note, no nudge,
//   no reminder. Measured on a real 140-message session against a live gateway,
//   at the point where the model was about to write a file: the history alone
//   produced `write_file(8308B)`; adding a note at the end (as `system`, as
//   `user`, or folded into the last tool result) made the model burn its whole
//   output budget on reasoning and never act. A model that reasons well but
//   needs the request to end on a turn is not an unusual model - so the request
//   is exactly the conversation, and anything the runtime wants to say to the
//   model has to fit inside the tool results it already produces. That is why the
//   notebook appears in the digest turn (`user`, and only when a compaction
//   happened) and never at the end of a request.
//
// # Your own notes (`note`)
//
// ```json
// {"name": "note", "arguments": {"text": "the flake is in rewind.rs: the journal is bound before the row exists"}}
// ```
//
// One tool, one argument. It appends to the notebook of the **current run** and
// answers with what is now held:
//
// ```text
// notebook: 2 findings held (318 chars). Newest kept:
// - the flake is in rewind.rs: the journal is bound before the row exists
// They are replayed inside the compaction digest, never appended to the request.
// ```
//
// Limits: at most 64 findings, each cut at 2,000 characters, 12,000 characters in
// total (the ceiling of the digest's note budget). A new entry past a limit
// releases the **oldest** one. The notebook lives for one run - a child agent has
// its own - and it is replayed only when a compaction drops messages, inside the
// same `user`-role digest as the other knowledge that survived. There it is the
// first block, as `- note: …` lines, before the list of dropped turns: the
// conclusions are read before the history they came from. Findings from an
// earlier run on the session are carried forward only while a digest that
// contains them is still in the window.
//
// Write a finding, not a status line: it costs a tool call, and the digest is the
// only place it can ever be read back.
//
// # Reading the hit rate
//
// Every provider call reports usage, and karucode reads the cached-token count
// from all three shapes in the wild (`prompt_tokens_details.cached_tokens`,
// `prompt_cache_hit_tokens`, `cache_read_input_tokens`). The chat HUD shows
// `N in · M out · P% cached`, and `GET /api/events` (SSE) carries
// `{"type":"usage","input_tokens":..,"output_tokens":..,"cached_tokens":..}`.
// `cached_tokens` is omitted when the endpoint does not report it - that is
// "unknown", not 0%.
//
// # Tuning it
//
// ```toml
// [limits]
// # 0 (the default) derives the budget from the resolved model's context_window
// # minus the room its answer needs, at four characters per token: a 1M-token
// # model gets ~2.5M chars, a 32k model ~110k. A non-zero value is an install
// # ceiling and can only lower that.
// context_max_chars = 0
// # How many messages one request may carry, default 900. 0 disables it.
// max_context_messages = 900
// # One tool result larger than this is truncated per call.
// max_tool_output_bytes = 24000
// ```
//
// # Two budgets, because a request has two ways to be too big
//
// `context_max_chars` bounds how much **text** is sent. `max_context_messages`
// bounds how many **messages** that text is spread across. They are not the same
// limit, and a gateway in front of the model can enforce the second one on its
// own: this machine's caps the request at 1,000 input *items*, whatever their
// size.
//
// A session of many short turns therefore dies of a budget nothing was watching.
// Measured in one database: **7 sessions carried more than 1,000 messages while
// staying under the 2 M-character budget** (worst: 2,055 items / 1.96 M chars),
// and every one of them could no longer be resumed - each turn failed with
// `conversation history exceeds the limit on input items (1002 > 1000 items)`.
//
// So both ceilings are enforced in the same place, by the same drop loop, with
// the same grouping rule:
//
// | | `context_max_chars` | `max_context_messages` |
// |---|---|---|
// | what it counts | characters of every message, tool results included | messages: the system prompt, the digest, every turn and every tool result |
// | default | derived from the model's `context_window` | **900** (under the 1,000-item ceiling, with room for a turn) |
// | `0` means | derive it from the model | **no ceiling** - only for a provider with no item limit |
//
// The count is what the request actually carries, and the compaction digest that
// replaces the dropped turns is counted too. Because OpenAI-compatible tool
// messages cannot carry images, every image-bearing source message also produces
// one extra `user` wire message (however many images that source carries); that
// expansion is counted in both fitting and the held run window. Compaction also buys
// `TURNS_OF_HEADROOM` (50) turns of room in *items*, exactly as it does in
// characters - a ceiling that compacted down to itself would re-compact on the
// very next turn and cost a cache miss per turn.
//
// A window that cannot be cut any further (the pinned task, the turn in flight
// and the tool results that answer it are already over the ceiling) is sent as it
// is and logged as a warning. It is never split to fit: a `tool` result whose
// `tool_call` was dropped is an *invalid* request, which fails the turn for a
// different reason and hides the real one. If you see that warning, raise
// `max_context_messages`, or start a new session.
//
// # When the provider refuses a request as too large anyway
//
// Prevention is a budget, not a guarantee: a model whose real window is smaller
// than it declares, or a gateway with a tighter limit than the one configured,
// still refuses the request. That refusal is recognised (a `context length`,
// `context_length_exceeded`, `prompt is too long`, `too many tokens`,
// `reduce the length`, `maximum context` or `input items` message, unless the
// status was transient - a 429 or a 5xx is a busy endpoint, not a long prompt)
// and recovered from **once**:
//
// 1. the run emits a warning notice naming both settings, so this is never a
//    silent cache miss;
// 2. the window is rebuilt against **60%** of each budget (`context_max_chars`
//    and `max_context_messages`), floored so what remains is still a valid
//    request;
// 3. the same turn is asked for again, exactly once.
//
// If that retried request is refused too, the run fails with the provider's own
// message plus the fact that a forced compaction was already attempted. It never
// loops: **one forced compaction per run**, and the smaller budgets are kept for
// the rest of that run, so the next turn starts already compacted instead of
// walking into the same refusal. Nothing was persisted for the refused attempt,
// so the transcript never carries the same assistant turn twice - and if the
// attempt had already streamed text, the client is told to drop it
// (`StreamRetry`) exactly as it is for a stream that dies mid-answer.
//
// # What is left when nothing can be dropped, and what gets shortened
//
// The pinned task and the exchange in flight are never *dropped*, but they are
// not a licence to send any size: when they alone are over the budget the largest
// bodies keep a head and a tail around a marker instead. Three kinds of body can
// do that to a request, and all three are shortened:
//
// * message text and tool-result text;
// * `reasoning_content`. A reasoning model's chain of thought is persisted with
//   its turn and written back into every later request, so it is charged against
//   `context_max_chars` like any other text - and therefore cut like any other
//   text. Leaving it out was a real hole: a window whose characters were mostly
//   reasoning was reported over budget, sent over budget, and refused by the
//   provider.
// * images, which are charged **8,000 characters each** (their token cost, not
//   their byte cost). A turn that attached more pictures than the budget can pay
//   for loses its oldest ones and says so in the result body
//   (`[N image(s) omitted: ...]`). Encoded image bytes are bounded separately, at
//   20 MiB per request, because that is a memory bound rather than a window one.
//
// Tool-call **arguments** are never cut: shortening a call's arguments changes
// what the call means and can leave invalid JSON for the tool's schema. A turn's
// arguments are bounded by the model's own output limit, so if a single turn's
// arguments alone exceed `context_max_chars` the request is sent as it is and
// logged - the one case a budget cannot fix without inventing a different call.
//
// # Gotchas
//
// * Compaction is the only thing that costs a full miss; if the hit rate is low,
//   check how often the transcript is over the budget. The budget is the model's
//   own window (`context_window`) unless an operator set `context_max_chars` as a
//   smaller ceiling - a cap below the window makes the harness drop work the
//   model could have held, which is what a long session pays for twice (lost
//   context and re-reads).
// * The digest is a `user` turn on purpose. An `assistant` turn would make the
//   model believe it had said the summary itself, which invites it to repeat or
//   re-plan work it already finished.
// * A tool result that repeats an identical answer raises a warning after the
//   third time. The fourth identical read-only call, with no write by this run in
//   between, is **refused** with a `[blocked: ...]` result instead of being
//   executed: re-reading an unchanged page cannot produce new information, and one
//   review run was measured re-reading the same file 27 times. A read after an
//   edit is allowed - the mutation counter resets the check - and the refusal is
//   capped at two, because a change made by a subagent, the user or an editor does
//   not move that counter.
// * Two tool calls with the same arguments in the same turn are not de-duplicated;
//   only the warning above is raised.
// * `prompt_cache_key` (OpenAI) is not sent. Prefix caching works without it; the
//   key only improves routing across cache shards on very large providers.

const TRIM_PREFIX: &str = "[context digest: ";
/// Runtime provenance for a digest. Content alone is untrusted: a user or tool
/// may legitimately return text beginning with [`TRIM_PREFIX`], and treating that
/// as a runtime marker would delete it and could orphan the neighbouring calls.
const DIGEST_NAME: &str = "kc-runtime:context-digest";

/// How much space the compaction digest may take. It is a summary, not a
/// transcript: if it grew without a bound it would defeat the budget it exists
/// to protect.
const DIGEST_MAX_CHARS: usize = 4_000;
/// Characters of one dropped message kept in the digest.
const DIGEST_LINE_CHARS: usize = 160;
/// Characters of one tool call's arguments kept in the digest.
const DIGEST_ARGS_CHARS: usize = 48;
/// Tool calls listed per digest line before the list is clipped.
const DIGEST_CALLS_PER_LINE: usize = 4;

/// An assistant turn with at least this much text and no tool calls is a
/// **written finding**, and written findings are sticky.
///
/// A reasoning model puts its analysis in `reasoning_content`, which is streamed
/// to the UI and then dropped - it is neither persisted nor resent. When such a
/// model also writes no final text, the transcript holds tool calls and tool
/// output only, so compaction leaves it with *nothing it concluded*: it re-reads
/// the same files and re-derives the same analysis (measured: 118 `read_file`
/// calls over 18 files in one review run, 27 of them on one file). Text the
/// model actually wrote is the only durable record of what it worked out, so it
/// is kept verbatim across every compaction.
const NOTE_MIN_CHARS: usize = 200;

/// Hard cap on the sticky findings, scaled to the budget. Oldest findings are
/// released first once the cap is exceeded; a finding is never truncated.
fn note_budget(max_chars: usize) -> usize {
    (max_chars / 20).clamp(1_000, 12_000)
}

/// How many findings one run's notebook holds. The oldest entry is released when
/// a new one would exceed it.
pub const NOTE_MAX_ENTRIES: usize = 64;
/// Characters one finding may store. A longer note is cut, with an ellipsis, when
/// it is appended - never silently.
pub const NOTE_ENTRY_MAX_CHARS: usize = 2_000;
/// Characters the whole notebook holds: the ceiling of `note_budget`, so what
/// the notebook keeps is exactly what the digest could replay. Oldest released
/// first, like the sticky findings.
pub const NOTE_MAX_CHARS: usize = 12_000;
/// Characters of one note kept on its digest line. A note is a finding, not a
/// transcript, so it gets more room than a dropped turn's summary - but one line,
/// because the digest line list is carried forward line by line.
pub const NOTE_DIGEST_LINE_CHARS: usize = 600;
/// Prefix of every notebook line in the digest. Also what keeps a carried digest
/// from duplicating the live notebook: `compact_notice` splits the two apart.
pub const NOTE_LINE_PREFIX: &str = "- note: ";

/// The prefix a superseded tool result starts with. Also what makes the pass
/// idempotent: a body that already starts with it is a stub and is left alone.
pub const SUPERSEDED_PREFIX: &str = "[superseded: ";

/// Tools whose result *header* carries the path they read, as
/// `path — lines A-B of N (...)`. `read_file` is the one that does.
const HEADER_TARGET_TOOLS: &[&str] = &["read_file"];

/// Tools whose target is the `path` argument of the call. A result from any other
/// tool is never superseded: two shell runs or two searches for different
/// patterns are different answers, not a newer copy of one answer.
const ARGUMENT_TARGET_TOOLS: &[&str] = &["read_file", "search_files", "list_dir", "git_diff"];

/// Compact down to this share of the budget rather than to the budget itself.
/// The difference is headroom for the next turns, and headroom is cache hits:
/// as long as the window stays under the budget nothing at the front changes.
const MIN_WINDOW_PERCENT: usize = 15;

/// How many turns of headroom a compaction tries to buy. A compaction rewrites
/// the front of the window, which costs exactly one cache miss, so the hit rate
/// is `1 / (1 + headroom/turn_size)`: 50 turns of headroom is 98%.
const TURNS_OF_HEADROOM: usize = 50;

/// How many of the newest messages a run window keeps even when the exchange
/// boundary says otherwise. The exchange rule below is the real protection.
///
/// This protects those messages from being **dropped**, not from being
/// **shortened**: a window whose protected tail alone is larger than the budget
/// is truncated body-by-body instead (see [`shrink_bodies`]) rather than sent
/// as it is. Treating this as "never compact a short history" is what let an
/// eight-message window carry 60k characters into a 32k budget.
const PROTECTED_TAIL: usize = 32;

/// Characters a single turn's tool results may add to the conversation.
///
/// A turn of 32 calls at the 24 KB per-call cap is 768 KB, which is three
/// budgets of tool output in one request: no compaction policy can make that
/// fit, and evicting it is what produced the re-read loop. Half the budget per
/// turn keeps every call visible (the head of each result) and leaves the other
/// half for history.
pub fn turn_budget(max_chars: usize) -> usize {
    max_chars / 2
}

/// Approximate model context cost per image (2,000 tokens at four chars/token).
pub const IMAGE_CONTEXT_CHARS: usize = 8_000;
/// Maximum combined encoded image bytes retained in a request or run window.
pub const IMAGE_BYTES_BUDGET: usize = 20 * 1024 * 1024;

/// Keep the newest images that fit the separate encoded-payload memory budget.
fn cap_image_bytes(messages: &mut [Message]) {
    let mut remaining = IMAGE_BYTES_BUDGET;
    for message in messages.iter_mut().rev() {
        let mut omitted = 0;
        let mut keep = vec![false; message.images.len()];
        for (i, image) in message.images.iter().enumerate().rev() {
            let bytes = image
                .data
                .len()
                .saturating_add(image.media_type.len())
                .saturating_add(13);
            if bytes <= remaining {
                remaining -= bytes;
                keep[i] = true;
            } else {
                omitted += 1;
            }
        }
        let mut i = 0;
        message.images.retain(|_| {
            let retained = keep[i];
            i += 1;
            retained
        });
        if omitted > 0 {
            message.content.push_str(&format!(
                "\n[{omitted} image(s) omitted: encoded image byte budget exceeded]"
            ));
        }
    }
}

/// Messages the OpenAI-compatible provider puts on the wire for this window.
///
/// Every image-bearing source message expands into itself plus one synthetic
/// user message (all of that source's images share the one addition). The
/// provider serializer is the conformance authority; the runtime keeps this
/// small structural mirror so [`ContextLimits::max_messages`] bounds the actual
/// request rather than the pre-serialization vector.
pub fn provider_message_count(messages: &[Message]) -> usize {
    messages.len().saturating_add(image_message_count(messages))
}

/// Number of characters a message contributes to the budget.
///
/// Public because anything that reports on the budget must count exactly what the
/// budget counts; a second formula would disagree with the thing it is
/// describing.
pub fn message_chars(message: &Message) -> usize {
    let calls: usize = message
        .tool_calls
        .iter()
        .map(|c| c.name.chars().count() + c.arguments.chars().count())
        .sum::<usize>();
    let reasoning: usize = message
        .reasoning_content
        .as_ref()
        .map(|r| r.chars().count())
        .unwrap_or(0);
    message.content.chars().count()
        + calls
        + reasoning
        + message.images.len().saturating_mul(IMAGE_CONTEXT_CHARS)
}

/// Characters the tool catalogue costs in a request body.
///
/// A request is `messages` **plus** `tools`, and only the messages are inside
/// [`ContextLimits::max_chars`]. The catalogue is not a rounding error: a real
/// install with 55 plugin tools measured ~40,900 bytes of JSON schema, and
/// nothing bounds a plugin or an MCP server from declaring longer descriptions,
/// bigger schemas or more tools - they arrive from another process. Two runs on
/// that install died at `prompt is too long: 1052770 / 1048573` with a window
/// that fitted its own character budget, because the catalogue and the prompt
/// envelope sat beside it uncounted.
///
/// [`ContextLimits::reserving`] is what puts this inside the budget:
///
/// ```ignore
/// let limits = spec.context_limits().reserving(context::tool_payload_chars(&tool_specs));
/// ```
///
/// Counted as `name + description + serialized parameters`, which is what
/// `kc_provider::request` writes per tool, minus the JSON scaffolding around it
/// (a few bytes per tool, so the count is a slight under-estimate and never an
/// over-estimate of the payload).
pub fn tool_payload_chars(tools: &[kc_core::tool::ToolSpec]) -> usize {
    tools
        .iter()
        .map(|tool| {
            tool.name.chars().count()
                + tool.description.chars().count()
                + serde_json::to_string(&tool.parameters)
                    .map(|schema| schema.chars().count())
                    .unwrap_or(0)
        })
        .sum()
}

/// The two ceilings one request must satisfy: **how much text** it may carry and
/// **how many messages** that text may be spread across.
///
/// They are independent on purpose. `max_chars` is what the model's window
/// allows; `max_messages` is what the gateway in front of it allows. A session of
/// many short messages can overflow the second while sitting far under the
/// first, and the request is refused either way, so one budget without the other
/// leaves a whole class of session that cannot be sent at all.
///
/// Both are enforced in the same drop loop (see [`fit`] /
/// [`compact_run_window`]), with the same grouping rule, so a window over the
/// item ceiling is compacted exactly like one over the character budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextLimits {
    /// Characters of conversation one request may carry.
    pub max_chars: usize,
    /// Messages one request may carry, counting the system prompt, any digest
    /// and every tool result. `0` disables the ceiling.
    pub max_messages: usize,
}

impl ContextLimits {
    /// Both ceilings. `max_messages = 0` means "no item ceiling".
    pub fn new(max_chars: usize, max_messages: usize) -> Self {
        Self {
            max_chars,
            max_messages,
        }
    }

    /// Only a character budget, for a caller that has no item ceiling to apply
    /// (a historical entry point, a test, a provider with no item limit).
    pub fn chars_only(max_chars: usize) -> Self {
        Self::new(max_chars, 0)
    }

    /// These ceilings with `chars` taken off the character budget for what the
    /// request carries **besides** the conversation.
    ///
    /// Today that is the serialized tool catalogue (see [`tool_payload_chars`]),
    /// which the provider writes beside the messages and which nothing else
    /// bounds: a plugin or an MCP server declares its own descriptions and
    /// schemas in another process. Reserving it is what makes "the request fits
    /// the model's window" true rather than true-of-the-messages-only.
    ///
    /// The reservation never takes more than half the budget: a catalogue larger
    /// than that leaves no conversation to send, and an empty conversation is not
    /// a smaller request - it is a run that cannot work. The remaining window
    /// then holds the task and the turn in flight, compacts hard, and says so
    /// through the usual over-budget warnings.
    pub fn reserving(self, chars: usize) -> Self {
        if chars == 0 {
            return self;
        }
        let reserved = chars.min(self.max_chars / 2);
        Self::new(
            self.max_chars.saturating_sub(reserved).max(1),
            self.max_messages,
        )
    }

    /// Is `count` messages inside the item ceiling?
    pub fn within_messages(&self, count: usize) -> bool {
        self.max_messages == 0 || count <= self.max_messages
    }

    /// Is a window of `chars` characters and `count` messages inside both
    /// ceilings? A window that is, is sent **byte-identical**: that is the
    /// prompt cache, and it is why this is checked before anything is rewritten.
    pub fn fits(&self, chars: usize, count: usize) -> bool {
        chars <= self.max_chars && self.within_messages(count)
    }

    /// The same two ceilings scaled down, for a request the provider has already
    /// refused as too large: 60% of each, floored so what is left is still a
    /// valid request (a system prompt, the task, the exchange in flight).
    ///
    /// Never raises a ceiling an operator set deliberately: a smaller of the two
    /// wins.
    pub fn forced_smaller(&self) -> Self {
        let chars = (self.max_chars / 100 * FORCED_COMPACT_PERCENT).max(FORCED_MIN_CHARS);
        let messages = if self.max_messages == 0 {
            0
        } else {
            (self.max_messages / 100 * FORCED_COMPACT_PERCENT)
                .max(FORCED_MIN_MESSAGES)
                .min(self.max_messages)
        };
        Self::new(chars.min(self.max_chars), messages)
    }
}

/// How much of each ceiling a forced compaction keeps. The provider has already
/// said the window does not fit, so re-sending something just under the limit
/// would most likely be refused again: two thirds of it is a real cut, and the
/// digest plus the exchange in flight still fit inside it.
const FORCED_COMPACT_PERCENT: usize = 60;
/// Floor for the forced character budget: below this nothing useful fits.
const FORCED_MIN_CHARS: usize = 4_000;
/// Floor for the forced item ceiling: a system prompt, the task, the assistant
/// turn in flight and its tool results.
const FORCED_MIN_MESSAGES: usize = 8;

/// The one message a request carries that the held run window does not: the
/// system prompt the loop rebuilds every iteration and prepends to `history`.
///
/// `compact_run_window_*` bounds what is *held*, and what is *sent* is one item
/// longer, so the held window gets one item less of the ceiling.
const REQUEST_SYSTEM_MESSAGES: usize = 1;

/// The result of fitting a conversation into a character budget.
///
/// [`fit`] only needs [`FitOutcome::messages`]. The rest exists so a caller (or a
/// test) can explain *why* a request looks the way it does without
/// re-implementing the compaction rules - a second implementation would drift
/// from this one and then confidently report the wrong thing.
#[derive(Debug, Clone)]
pub struct FitOutcome {
    /// The list that will be sent: truncated, superseded, compacted, digest
    /// inserted.
    pub messages: Vec<Message>,
    /// The input after per-message truncation and after the supersession pass, and
    /// before any compaction. Same length and order as the input, so `dropped[i]`
    /// describes `prepared[i]`. A body here may already be a stub: the window was
    /// over budget, and that is the only time the pass runs.
    pub prepared: Vec<Message>,
    /// `prepared[i]` was removed by compaction.
    pub dropped: Vec<bool>,
    /// Characters the supersession pass removed from `prepared[i]`; `0` for every
    /// message it left alone. A stubbed message is *not* dropped - it still
    /// answers its `tool_call` - so this is what keeps the saving visible.
    pub superseded_chars: Vec<usize>,
    /// The digest that replaced the dropped turns, when a compaction happened.
    pub notice: Option<String>,
}

impl FitOutcome {
    fn unchanged(prepared: Vec<Message>) -> Self {
        let dropped = vec![false; prepared.len()];
        let superseded_chars = vec![0; prepared.len()];
        Self {
            messages: prepared.clone(),
            prepared,
            dropped,
            superseded_chars,
            notice: None,
        }
    }

    /// Characters in the list that will be sent.
    pub fn chars(&self) -> usize {
        self.messages.iter().map(message_chars).sum()
    }

    /// Characters the compaction removed from the window: the dropped messages,
    /// plus what the supersession pass retired from the bodies it kept.
    ///
    /// A body that was stubbed and then dropped counts once, as dropped: its stub
    /// never reached the request, so charging its saving again would report more
    /// bytes gone than there were.
    pub fn compacted_chars(&self) -> usize {
        let dropped: usize = self
            .prepared
            .iter()
            .zip(&self.dropped)
            .filter(|(_, dropped)| **dropped)
            .map(|(message, _)| message_chars(message))
            .sum();
        let stubbed: usize = self
            .superseded_chars
            .iter()
            .zip(&self.dropped)
            .filter(|(_, dropped)| !**dropped)
            .map(|(saved, _)| *saved)
            .sum();
        dropped + stubbed
    }
}

/// Drop oldest exchanges until the budget fits, then insert the compaction
/// notice.
///
/// `messages` must be ordered oldest-first and is expected to start with the
/// system prompt. Returns an unchanged copy when everything already fits.
///
/// The notice costs characters, so it is part of the budget: when the digest no
/// longer fits, one more exchange is dropped and the digest is rebuilt.
///
/// This entry point applies a character budget only; [`fit_with_limits`] also
/// enforces the item ceiling.
pub fn fit(messages: Vec<Message>, max_chars: usize, max_tool_output_bytes: usize) -> Vec<Message> {
    fit_with_limits(
        messages,
        ContextLimits::chars_only(max_chars),
        max_tool_output_bytes,
    )
}

/// [`fit`] with both ceilings: characters **and** messages.
///
/// A window over either one is compacted the same way - the oldest droppable
/// exchange is removed, whole, until both hold - so a caller never has to know
/// which limit the provider is about to complain about.
pub fn fit_with_limits(
    messages: Vec<Message>,
    limits: ContextLimits,
    max_tool_output_bytes: usize,
) -> Vec<Message> {
    fit_detailed_with_limits(messages, limits, max_tool_output_bytes).messages
}

/// [`fit`], plus the bookkeeping that explains the result.
///
/// Same rules, same output in [`FitOutcome::messages`]; the extra fields are
/// what the context report reads.
pub fn fit_detailed(
    messages: Vec<Message>,
    max_chars: usize,
    max_tool_output_bytes: usize,
) -> FitOutcome {
    fit_detailed_with_limits(
        messages,
        ContextLimits::chars_only(max_chars),
        max_tool_output_bytes,
    )
}

/// [`fit_detailed`] with both ceilings. See [`fit_with_limits`].
pub fn fit_detailed_with_limits(
    messages: Vec<Message>,
    limits: ContextLimits,
    max_tool_output_bytes: usize,
) -> FitOutcome {
    fit_report(messages, limits, max_tool_output_bytes, &[])
}

/// [`fit_detailed`] with the run's notebook ([`Notebook::entries`]) replayed in
/// the compaction digest.
///
/// The notebook is the only place a `note` survives a compaction, and it is read
/// here - never appended to the conversation, and never present when nothing was
/// dropped (see the module docs).
pub fn fit_with_notes(
    messages: Vec<Message>,
    max_chars: usize,
    max_tool_output_bytes: usize,
    notes: &[String],
) -> FitOutcome {
    fit_with_notes_and_limits(
        messages,
        ContextLimits::chars_only(max_chars),
        max_tool_output_bytes,
        notes,
    )
}

/// [`fit_with_notes`] with both ceilings: the entry point the run loop uses, so
/// the item count is bounded exactly where the character budget already is.
pub fn fit_with_notes_and_limits(
    messages: Vec<Message>,
    limits: ContextLimits,
    max_tool_output_bytes: usize,
    notes: &[String],
) -> FitOutcome {
    fit_report(messages, limits, max_tool_output_bytes, notes)
}

fn fit_report(
    messages: Vec<Message>,
    limits: ContextLimits,
    max_tool_output_bytes: usize,
    notes: &[String],
) -> FitOutcome {
    let max_chars = limits.max_chars;
    let mut prepared: Vec<Message> = messages
        .into_iter()
        .map(|mut m| {
            if m.role == Role::Tool {
                let (content, _) = truncate_output(&m.content, max_tool_output_bytes);
                m.content = content;
            }
            m
        })
        .collect();
    cap_image_bytes(&mut prepared);

    let mut total: usize = prepared.iter().map(message_chars).sum();
    // Both ceilings are checked before anything is rewritten: a window that
    // satisfies them is returned byte-identical, which is what keeps the
    // provider's prefix cache hitting between compactions.
    if limits.fits(total, provider_message_count(&prepared)) {
        return FitOutcome::unchanged(prepared);
    }

    // The last user turn is the request being answered; everything from it
    // onwards is untouchable. The first user message (the task) is pinned too.
    let end = prepared
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(0);
    let start = pinned_prefix(&prepared);

    // Over budget: retire the tool results a later call has already replaced.
    // Everything from the pinned prefix on decides what is stale (the exchange in
    // flight included); only what comes before the exchange in flight - and before
    // the user turn being answered - is rewritten. It runs here, on the
    // over-budget path only, so the cached prefix of a fitting window is never
    // rewritten.
    //
    // ... and only when the *characters* are over budget. A window that is over
    // the item ceiling alone gains nothing from a stubbed body (the count does not
    // move) and would pay a cache miss for it.
    let cut = if end > start {
        current_exchange_start(&prepared).min(end)
    } else {
        current_exchange_start(&prepared)
    };
    let superseded_chars = if total > max_chars {
        supersede(&mut prepared, start, cut)
    } else {
        vec![0; prepared.len()]
    };
    let saved: usize = superseded_chars.iter().sum();
    if saved > 0 {
        total = total.saturating_sub(saved);
    }
    if limits.fits(total, provider_message_count(&prepared)) {
        // The stubs alone brought the window under budget (or under the item
        // ceiling, which stubs cannot move - the count is unchanged). Nothing
        // is dropped, so there is nothing to digest.
        let dropped = vec![false; prepared.len()];
        return FitOutcome {
            messages: prepared.clone(),
            prepared,
            dropped,
            superseded_chars,
            notice: None,
        };
    }

    let sticky = sticky_flags(&prepared, max_chars);
    let mut units = group_droppable(&prepared, start, end.max(start), &sticky);
    if units.is_empty() {
        // No user boundary to drop behind (a subagent talks to one task the
        // whole run). Fall back to keeping the newest tail, so an over-budget
        // window is never sent as-is even when nothing can be dropped whole.
        let end = clamp_end_to_exchange(
            &prepared,
            start,
            prepared.len().saturating_sub(PROTECTED_TAIL).max(start),
        );
        units = group_droppable(&prepared, start, end, &sticky);
    }

    let target = target_chars(&prepared, max_chars);
    // The item ceiling aims below itself for the same reason the character
    // budget does: a compaction rewrites the front of the window, which costs a
    // cache miss, so it has to buy `TURNS_OF_HEADROOM` turns of room. A ceiling
    // that compacted to exactly the ceiling would re-compact on the very next
    // turn and turn a one-off miss into a per-turn one.
    let count_target = target_messages(&prepared, limits);
    // `rebuild` replaces every digest it finds with one new digest, so a window
    // that already carries one is freed of it and charged for the new one.
    let digests = prepared.iter().filter(|m| is_digest(m)).count();
    let mut survivors = total;
    let mut survivor_count = prepared.len();
    let mut survivor_image_messages = image_message_count(&prepared);
    let mut dropped_messages = 0usize;
    let mut dropped = vec![false; prepared.len()];
    let mut next = 0usize;

    let notice = loop {
        // Over the character budget *or* over the item ceiling: the same loop
        // drops the same units for either reason, so the counts can never
        // disagree about what "compact" means.
        while next < units.len()
            && (survivors > target
                || emitted_count(survivor_count, digests, survivor_image_messages) > count_target)
        {
            let unit = &units[next];
            survivors = survivors.saturating_sub(unit_chars(&prepared, unit));
            survivor_count = survivor_count.saturating_sub(unit.len());
            survivor_image_messages =
                survivor_image_messages.saturating_sub(unit_image_messages(&prepared, unit));
            dropped_messages += unit.len();
            for &i in unit {
                dropped[i] = true;
            }
            next += 1;
        }
        if dropped_messages == 0 {
            if total <= max_chars {
                // Nothing may be dropped, and the characters are fine: the
                // window is over the *item* ceiling only. There is nothing left
                // to cut - every remaining message is the pinned task, the
                // exchange in flight, or a sticky finding, and a tool result
                // cannot be dropped without the call it answers - so the request
                // is sent as it is. Eliding bodies would not remove a single
                // item, and it would cost a cache miss to prove it.
                warn_if_over_ceiling(&prepared, limits, "nothing may be dropped");
                return FitOutcome {
                    superseded_chars,
                    ..FitOutcome::unchanged(prepared)
                };
            }
            // Nothing may be dropped: the request is the pinned prefix and the
            // protected tail, and all of it is over budget. Elide the largest
            // bodies rather than send a request several times the budget.
            let mut window = prepared.clone();
            return match shrink_window(&mut window, &dropped, start, max_chars, notes) {
                Some((messages, notice)) => {
                    warn_if_over_ceiling(&messages, limits, "nothing may be dropped");
                    FitOutcome {
                        messages,
                        prepared,
                        dropped,
                        superseded_chars,
                        notice: Some(notice),
                    }
                }
                None => {
                    warn_if_over_ceiling(&prepared, limits, "nothing may be dropped");
                    FitOutcome {
                        superseded_chars,
                        ..FitOutcome::unchanged(prepared)
                    }
                }
            };
        }
        let notice = compact_notice(&prepared, &dropped, dropped_messages, max_chars, notes);
        let fits = survivors + notice.chars().count() <= max_chars
            && limits.within_messages(emitted_count(
                survivor_count,
                digests,
                survivor_image_messages,
            ));
        if fits || next >= units.len() {
            break notice;
        }
        // No room for the digest (or one item too many for the ceiling): drop
        // one more exchange and rebuild it.
        let unit = &units[next];
        survivors = survivors.saturating_sub(unit_chars(&prepared, unit));
        survivor_count = survivor_count.saturating_sub(unit.len());
        survivor_image_messages =
            survivor_image_messages.saturating_sub(unit_image_messages(&prepared, unit));
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
        next += 1;
    };

    // Every droppable unit is gone and the tail alone still does not fit with
    // the digest: elide the surviving bodies, then rebuild.
    let mut window = prepared.clone();
    let notice_chars = notice.chars().count();
    let shortened = shrink_bodies(
        &mut window,
        &dropped,
        max_chars.saturating_sub(notice_chars),
    );
    let messages = rebuild(window, start, &dropped, notice.clone());
    if shortened > 0 {
        warn_elided(&messages, max_chars);
    }
    warn_if_over_ceiling(
        &messages,
        limits,
        "every droppable exchange is gone and the pinned prefix plus the exchange in \
         flight do not fit it",
    );
    FitOutcome {
        messages,
        prepared,
        dropped,
        superseded_chars,
        notice: Some(notice),
    }
}

/// Keep the **in-memory** history of a running loop inside the request budget.
///
/// `fit` bounds what is *sent*; this bounds what is *held*. Without it a session
/// with thousands of tool calls keeps every result in memory and the loop clones
/// the whole conversation into the request on every iteration.
///
/// Invariants, in priority order: the newest exchange (the turn in flight) is
/// never dropped, the original task is never dropped, and everything else is
/// dropped from the front in whole exchanges until the window is down to
/// [`COMPACT_TARGET_PERCENT`] of the budget. The second-order effect of that last
/// rule is the important one: the front of the window stays byte-identical for
/// many turns, so the provider's prompt cache keeps hitting.
///
/// When there is nothing left to drop - a history of at most [`PROTECTED_TAIL`]
/// messages, or one whose every droppable unit is pinned - the window is
/// **shrunk** instead of being returned over budget: the largest bodies keep a
/// head and a tail around a marker, and the pinned task is bounded last. A
/// short-but-huge history is exactly the case this function exists for, so
/// "few messages" must never mean "send 8x the budget".
///
/// The item ceiling ([`ContextLimits::max_messages`]) is enforced by the same
/// loop, for the same headroom: a held window of 2,000 short messages is as
/// un-sendable as one of 2 M characters, and it is refused by the gateway before
/// the model ever sees it.
pub fn compact_run_window(history: &mut Vec<Message>, max_chars: usize) {
    compact_run_window_with_notes(history, max_chars, &[]);
}

/// [`compact_run_window`], with the run's [`Notebook`] replayed in the digest it
/// inserts.
///
/// The notebook is per run and the caller owns it; this never appends anything to
/// the window - it only folds the notes into the `user`-role digest that replaces
/// the dropped turns.
pub fn compact_run_window_with_notes(
    history: &mut Vec<Message>,
    max_chars: usize,
    notes: &[String],
) {
    compact_run_window_with_limits(history, ContextLimits::chars_only(max_chars), notes);
}

/// [`compact_run_window`] with both ceilings: characters **and** messages.
///
/// The entry point the run loop uses. A window over either ceiling is compacted
/// the same way, and one under both is left byte-identical - the loop re-clones
/// it into every request, so touching it between compactions would cost a
/// prompt-cache miss per turn.
pub fn compact_run_window_with_limits(
    history: &mut Vec<Message>,
    limits: ContextLimits,
    notes: &[String],
) {
    // What the loop sends is `[system prompt, history...]`, so the window this
    // bounds may carry one item fewer than the ceiling. A held window that only
    // fits once the system prompt is forgotten is a window that does not fit -
    // and it would make `fit` compact again on the same turn, for one extra cache
    // miss.
    let limits = if limits.max_messages == 0 {
        limits
    } else {
        ContextLimits::new(
            limits.max_chars,
            limits
                .max_messages
                .saturating_sub(REQUEST_SYSTEM_MESSAGES)
                .max(1),
        )
    };
    let max_chars = limits.max_chars;
    cap_image_bytes(history);
    let mut total: usize = history.iter().map(message_chars).sum();
    if limits.fits(total, provider_message_count(history)) {
        return;
    }

    let start = pinned_prefix(history);
    let current = current_exchange_start(history);

    // Over budget: retire the tool results a later call already replaced. The
    // scan covers the whole window (a page read in the turn in flight is what
    // makes an older page redundant), but nothing from the in-flight exchange on
    // is rewritten - that is what the model is reasoning about right now. In
    // place, because this window is what the loop holds and re-clones into every
    // request.
    //
    // Only when the *characters* are over: over the item ceiling alone there is
    // nothing for the pass to buy, and rewriting a body that fits costs a cache
    // miss for no saving at all.
    let saved: usize = if total > max_chars {
        supersede(history, start, current).iter().sum()
    } else {
        0
    };
    if saved > 0 {
        total = total.saturating_sub(saved);
    }
    if limits.fits(total, provider_message_count(history)) {
        // The stubs alone brought the held window under budget.
        return;
    }

    let sticky = sticky_flags(history, max_chars);
    // A window shorter than the protected tail has no droppable prefix at all;
    // saturating here is what makes the short case fall through to elision
    // instead of panicking on a subtraction.
    let tail_start = history.len().saturating_sub(PROTECTED_TAIL);
    let end = clamp_end_to_exchange(history, start, tail_start.min(current).max(start));
    let units = group_droppable(history, start, end, &sticky);
    if units.is_empty() {
        // Nothing may be dropped whole: the window is the pinned prefix plus the
        // protected tail, and all of it is over budget. Elide the largest bodies
        // rather than hold (and re-clone into every request) several times the
        // budget for the rest of the run.
        let dropped = vec![false; history.len()];
        let mut window = history.clone();
        if let Some((messages, _notice)) =
            shrink_window(&mut window, &dropped, start, max_chars, notes)
        {
            warn_elided(&messages, max_chars);
            warn_if_over_ceiling(&messages, limits, "nothing may be dropped");
            *history = messages;
        }
        return;
    }

    let target = target_chars(history, max_chars);
    let count_target = target_messages(history, limits);
    let digests = history.iter().filter(|m| is_digest(m)).count();
    let mut dropped = vec![false; history.len()];
    let mut dropped_messages = 0usize;
    let mut survivors = total;
    let mut survivor_count = history.len();
    let mut survivor_image_messages = image_message_count(history);
    let mut next = 0usize;

    // The digest costs characters too, so it is part of the budget: keep
    // dropping (whole exchanges) until survivors + notice fit, and until the
    // item count fits the ceiling.
    let notice = loop {
        while next < units.len()
            && (survivors > target
                || emitted_count(survivor_count, digests, survivor_image_messages) > count_target)
        {
            let unit = &units[next];
            survivors = survivors.saturating_sub(unit_chars(history, unit));
            survivor_count = survivor_count.saturating_sub(unit.len());
            survivor_image_messages =
                survivor_image_messages.saturating_sub(unit_image_messages(history, unit));
            dropped_messages += unit.len();
            for &i in unit {
                dropped[i] = true;
            }
            next += 1;
        }
        if dropped_messages == 0 {
            if total <= max_chars {
                // Over the item ceiling and nothing else: every remaining message
                // is the pinned task, the exchange in flight or a sticky finding,
                // so there is no item that may be removed. Eliding bodies cannot
                // change a count, so the window is held as it is - out loud, and
                // only in the log (a notice here would be a request that changes
                // on every turn).
                warn_if_over_ceiling(history, limits, "nothing may be dropped");
                return;
            }
            // Nothing turned out to be droppable: elide the bodies instead.
            let mut window = history.clone();
            if let Some((messages, _)) =
                shrink_window(&mut window, &dropped, start, max_chars, notes)
            {
                warn_elided(&messages, max_chars);
                warn_if_over_ceiling(&messages, limits, "nothing may be dropped");
                *history = messages;
            }
            return;
        }
        let notice = compact_notice(history, &dropped, dropped_messages, max_chars, notes);
        let fits = survivors + notice.chars().count() <= max_chars
            && limits.within_messages(emitted_count(
                survivor_count,
                digests,
                survivor_image_messages,
            ));
        if fits || next >= units.len() {
            break notice;
        }
        let unit = &units[next];
        survivors = survivors.saturating_sub(unit_chars(history, unit));
        survivor_count = survivor_count.saturating_sub(unit.len());
        survivor_image_messages =
            survivor_image_messages.saturating_sub(unit_image_messages(history, unit));
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
        next += 1;
    };

    // Every droppable unit is gone and the tail alone is still too big: elide
    // the surviving bodies, so what is held (and sent) respects the budget.
    let mut window = std::mem::take(history);
    let notice_chars = notice.chars().count();
    if shrink_bodies(
        &mut window,
        &dropped,
        max_chars.saturating_sub(notice_chars),
    ) > 0
    {
        let rebuilt = rebuild(window.clone(), start, &dropped, notice.clone());
        warn_elided(&rebuilt, max_chars);
        warn_if_over_ceiling(
            &rebuilt,
            limits,
            "every droppable exchange is gone and the pinned prefix plus the exchange \
             in flight do not fit it",
        );
        *history = rebuilt;
        return;
    }
    let out = rebuild(window, start, &dropped, notice);
    warn_if_over_ceiling(
        &out,
        limits,
        "every droppable exchange is gone and the pinned prefix plus the exchange in \
         flight do not fit it",
    );
    *history = out;
}

// ---------------------------------------------------------------------------
// supersession
// ---------------------------------------------------------------------------

/// Retire, in place, the tool results a later result in the same window already
/// replaced.
///
/// A `read_file` page of `src/app.tsx` read at turn 3 says nothing at turn 300
/// once turn 40 read the same file again, but it still rides along in every
/// request and in the digest budget. So: walk the results newest-first, keep the
/// first result per target, and stub the rest with [`SUPERSEDED_PREFIX`].
///
/// Where the target comes from:
///
/// * `read_file` writes its own header (`path — lines A-B of N (...)`), so the
///   path and the line range are read from the result itself - the call may have
///   used `offset`/`limit`, and the header is the only place that says which page
///   was actually returned;
/// * `search_files`, `list_dir`, `git_diff` and a `read_file` that returned no
///   header (an image, an error) fall back to the `path` argument of the call;
/// * any other tool has no target: two shell runs, or two reads of two different
///   files, are different answers.
///
/// The *question* is part of the key, not only the path. A `search_files` for a
/// different pattern, or a `git_diff` against another base or against the index
/// instead of the working tree, is a different answer about the same path;
/// treating it as a replacement would delete information no later result
/// restores. So the key is `(tool, path, qualifier)`, where the qualifier is
/// those answer-changing arguments - empty for `read_file` and `list_dir`.
///
/// Only two things are true of the range: results in `messages[from..]` decide
/// what is stale (the newest result per target wins, **including** the exchange in
/// flight - a page read in the current turn is exactly what makes last turn's page
/// of the same file redundant), and only results before `stub_below` are rewritten.
/// `from` is the end of the pinned prefix and `stub_below` the start of the
/// exchange in flight, so the task and the turn being reasoned about are neither
/// retired nor counted as retired. No message is removed - a stubbed `tool`
/// message still answers its `tool_call` - and the pass is idempotent: a body that
/// is already a stub is left alone.
///
/// Returns the characters saved, per message (`0` where nothing changed), so the
/// report can account for the saving without measuring the window twice.
fn supersede(messages: &mut [Message], from: usize, stub_below: usize) -> Vec<usize> {
    let mut saved = vec![0usize; messages.len()];
    let from = from.min(messages.len());
    if from >= messages.len() {
        return saved;
    }

    // Resolve each result against the assistant occurrence that structurally
    // owns its contiguous result run. Call ids are not globally unique in real
    // transcripts: gateways reuse ids such as `call_0`, sometimes with different
    // arguments. A global id -> arguments map would make the old result inherit
    // the newest occurrence's path and stub it with a false replacement claim.
    let mut calls: Vec<Option<CallTarget>> = vec![None; messages.len()];
    let mut owner: Option<&Message> = None;
    for (index, message) in messages.iter().enumerate() {
        match message.role {
            Role::Assistant if !message.tool_calls.is_empty() => owner = Some(message),
            Role::Tool => {
                calls[index] = message.tool_call_id.as_deref().and_then(|id| {
                    owner
                        .and_then(|assistant| {
                            assistant.tool_calls.iter().find(|call| call.id == id)
                        })
                        .map(CallTarget::from)
                });
            }
            _ => owner = None,
        }
    }

    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    for i in (from..messages.len()).rev() {
        if messages[i].role != Role::Tool {
            continue;
        }
        let call = calls[i].as_ref();
        let tool = match messages[i].name.as_deref() {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => match call {
                Some(call) => call.tool.clone(),
                None => continue,
            },
        };
        if !ARGUMENT_TARGET_TOOLS.contains(&tool.as_str()) {
            continue;
        }
        let header = if HEADER_TARGET_TOOLS.contains(&tool.as_str()) {
            header_target(&messages[i].content)
        } else {
            None
        };
        let (path, range) = match header {
            Some(target) => target,
            None => match call.and_then(|call| call.path.clone()) {
                Some(path) => (path, None),
                None => continue,
            },
        };
        let qualifier = call.map(|call| call.qualifier.clone()).unwrap_or_default();
        if seen.insert((tool.clone(), path.clone(), qualifier)) {
            // The newest result per target is the good one.
            continue;
        }
        if i >= stub_below {
            // Inside the exchange in flight: it made the older one stale, but it
            // is what the model is reasoning about right now.
            continue;
        }
        // A stub is already the shortest honest form of this result, and it is
        // the only place the retired page's line range survives: re-deriving the
        // range from it is impossible (the header is gone), so re-stubbing it
        // would trade `lines 1-300 of 812` for a vaguer sentence on the next
        // compaction. Measured on a live run: the first stub carried the range,
        // the second lost it.
        if is_superseded(&messages[i]) {
            continue;
        }
        if let Some(saving) = stub_superseded(&mut messages[i], &tool, &path, range.as_deref()) {
            saved[i] = saving;
        }
    }
    saved
}

/// One call's target, resolved from its arguments once.
#[derive(Clone)]
struct CallTarget {
    tool: String,
    path: Option<String>,
    qualifier: String,
}

impl CallTarget {
    fn from(call: &kc_core::message::ToolCall) -> Self {
        let arguments: Option<serde_json::Value> = serde_json::from_str(&call.arguments).ok();
        let path = arguments
            .as_ref()
            .and_then(|args| args.get("path"))
            .and_then(|path| path.as_str())
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string);
        Self {
            tool: call.name.clone(),
            path,
            qualifier: qualifier(&arguments),
        }
    }
}

/// The arguments that change *which answer* a call returns without changing the
/// path it names. Part of the supersession key: see [`supersede`].
fn qualifier(arguments: &Option<serde_json::Value>) -> String {
    let Some(arguments) = arguments else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    for key in ["pattern", "glob", "staged", "base", "depth"] {
        match arguments.get(key) {
            Some(serde_json::Value::Null) | None => {}
            Some(value) => {
                let text = match value.as_str() {
                    Some(text) => text.to_string(),
                    None => value.to_string(),
                };
                parts.push(format!("{key}={}", one_line(&text, 64)));
            }
        }
    }
    parts.join(";")
}

/// The `path — lines A-B of N (...)` header `read_file` writes first, as
/// `(path, Some("A-B of N"))`. `None` for a result that is not a page of text.
fn header_target(content: &str) -> Option<(String, Option<String>)> {
    let first = content.lines().next()?;
    let (path, rest) = first.split_once(" — lines ")?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    // Drop the byte count: `1-400 of 812 (17 bytes)` -> `1-400 of 812`. A count
    // capped by the byte budget is written `812+`, so `+` is allowed.
    let range = rest.split(" (").next().unwrap_or(rest).trim();
    let plausible = range.contains(" of ")
        && range.starts_with(|c: char| c.is_ascii_digit())
        && range
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | ' ' | 'o' | 'f'));
    Some((
        path.to_string(),
        if plausible {
            Some(range.to_string())
        } else {
            None
        },
    ))
}

/// Replace one superseded body with its stub. Returns the characters saved, or
/// `None` when the body is already minimal (a stub from an earlier compaction).
fn stub_superseded(
    message: &mut Message,
    tool: &str,
    path: &str,
    range: Option<&str>,
) -> Option<usize> {
    let stub = superseded_stub(tool, path, range);
    let before = message.content.chars().count();
    let after = stub.chars().count();
    if after >= before {
        return None;
    }
    message.content = stub;
    Some(before - after)
}

/// The one line that stands in for a superseded result.
///
/// It names the tool, the path and the line range, says what happened, and says
/// the call is allowed again: a stub that only said "omitted" would read like a
/// failure the model has to work around, and it would invite it to stop reading
/// files altogether.
fn superseded_stub(tool: &str, path: &str, range: Option<&str>) -> String {
    match range {
        Some(range) => format!(
            "{SUPERSEDED_PREFIX}{tool} `{path}` lines {range} was replaced by a later call in this \
             run. Call {tool} again if you still need that range.]"
        ),
        None => format!(
            "{SUPERSEDED_PREFIX}{tool} `{path}` was replaced by a later call in this run. Call \
             {tool} again if you still need it.]"
        ),
    }
}

/// Is this message a tool result a later call has retired?
pub fn is_superseded(message: &Message) -> bool {
    message.role == Role::Tool && message.content.starts_with(SUPERSEDED_PREFIX)
}

// ---------------------------------------------------------------------------
// the notes channel
// ---------------------------------------------------------------------------

/// The findings one run chose to keep.
///
/// A reasoning model puts its analysis in `reasoning_content`, which is streamed
/// to the UI and then dropped: neither persisted nor resent. When such a model
/// also writes no final text, the transcript holds tool calls and tool output
/// only - measured over the last 30 real sessions, 8,216 of 11,137 assistant
/// turns carried under 40 characters of text, and 47.5 tool-output characters
/// were ingested per character of written finding. Compaction then deletes the
/// little that was left.
///
/// The notebook is the runtime's answer: the model calls [`NoteTool`] to append a
/// finding, and the notebook is replayed inside the compaction digest. It is
/// **not** a second conversation and it is never appended to a request: only the
/// `user`-role digest turn carries it, and only when a compaction dropped
/// messages.
///
/// Bounded on three axes, all of them oldest-released-first:
///
/// * [`NOTE_MAX_ENTRIES`] findings;
/// * [`NOTE_ENTRY_MAX_CHARS`] characters per finding (cut with an ellipsis);
/// * [`NOTE_MAX_CHARS`] characters in total, the ceiling of `note_budget` - the
///   same policy that bounds the sticky written findings, so what the notebook
///   holds is what the digest could replay.
#[derive(Debug, Clone, Default)]
pub struct Notebook {
    entries: Vec<String>,
}

impl Notebook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one finding. Returns `false` when the text was empty and nothing was
    /// stored; a finding that does not fit releases the oldest entries first.
    pub fn append(&mut self, text: &str) -> bool {
        let mut text = text.trim().to_string();
        if text.is_empty() {
            return false;
        }
        if text.chars().count() > NOTE_ENTRY_MAX_CHARS {
            text = text.chars().take(NOTE_ENTRY_MAX_CHARS).collect();
            text.push('\u{2026}');
        }
        while self.entries.len() >= NOTE_MAX_ENTRIES {
            self.entries.remove(0);
        }
        while !self.entries.is_empty() && self.chars() + text.chars().count() > NOTE_MAX_CHARS {
            self.entries.remove(0);
        }
        self.entries.push(text);
        true
    }

    /// The findings, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Characters of finding held.
    pub fn chars(&self) -> usize {
        self.entries.iter().map(|e| e.chars().count()).sum()
    }

    /// What a `note` call answers with: the count, the size, and a short tail, so
    /// the model can see that the finding landed and what else is held.
    pub fn summary(&self) -> String {
        if self.entries.is_empty() {
            return "notebook: nothing held yet.".to_string();
        }
        let mut text = format!(
            "notebook: {} finding{} held ({} chars). Newest kept:",
            self.entries.len(),
            if self.entries.len() == 1 { "" } else { "s" },
            self.chars()
        );
        for entry in self.entries.iter().rev().take(NOTE_SUMMARY_TAIL) {
            text.push_str(&format!("\n- {}", one_line(entry, NOTE_SUMMARY_LINE_CHARS)));
        }
        text.push_str(REPLAY_NOTE);
        text
    }
}

/// The name the model calls to append to its notebook.
pub const NOTE_TOOL: &str = "note";

/// How much of a note the `note` tool's own answer shows.
const NOTE_SUMMARY_LINE_CHARS: usize = 120;
/// How many notes the `note` tool's own answer shows.
const NOTE_SUMMARY_TAIL: usize = 3;
/// What the `note` tool says about where the finding goes.
const REPLAY_NOTE: &str =
    "\nThey are replayed inside the compaction digest, never appended to the request.";

/// The runtime-owned tool that appends to the run's [`Notebook`].
///
/// Registered per run (see [`with_note_tool`]), not in `kc-tools` and not in the
/// delegate family: those are installed once for the whole runtime, and a
/// notebook belongs to exactly one loop.
pub struct NoteTool {
    notebook: Arc<Mutex<Notebook>>,
}

impl NoteTool {
    pub fn new(notebook: Arc<Mutex<Notebook>>) -> Self {
        Self { notebook }
    }
}

#[async_trait::async_trait]
impl kc_core::tool::Tool for NoteTool {
    fn spec(&self) -> kc_core::tool::ToolSpec {
        kc_core::tool::ToolSpec {
            name: NOTE_TOOL.to_string(),
            description: "Append a durable finding to this run's notebook. Use it for what you \
                          worked out and would otherwise lose: a root cause, a decision, a fact \
                          about the code that cost several tool calls to establish. The notebook \
                          is replayed inside the context digest, so a finding outlives the \
                          messages compaction drops. It is bounded (64 findings, 2,000 characters \
                          each, 12,000 in total; the oldest is released first) and it is never \
                          shown to you until a compaction drops something, so write findings, not \
                          status lines. One call appends one finding."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "The finding, in one or a few sentences. Written for you to read back after the transcript around it is gone."
                    }
                },
                "required": ["text"]
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> kc_core::tool::ToolClass {
        // Not a workspace mutation: a finding lives in this run's memory and
        // disappears with it. `read_only` approval mode still lets it through,
        // which is right - noting a plan changes nothing on disk.
        kc_core::tool::ToolClass::ReadOnly
    }

    fn requires_approval(&self) -> bool {
        false
    }

    async fn invoke(
        &self,
        request: kc_core::tool::ToolCallRequest,
        _ctx: kc_core::tool::ToolCtx,
    ) -> kc_core::tool::ToolOutcome {
        let text = match request.arg_str("text") {
            Ok(text) => text.to_string(),
            Err(e) => return kc_core::tool::ToolOutcome::err(e.to_string()),
        };
        let mut notebook = match self.notebook.lock() {
            Ok(notebook) => notebook,
            // A panicking reader must not cost the run its memory: the notebook
            // is a `Vec<String>` and cannot be left in a torn state.
            Err(poisoned) => poisoned.into_inner(),
        };
        if !notebook.append(&text) {
            return kc_core::tool::ToolOutcome::err(
                "`text` may not be empty: a note is a finding you want to keep",
            );
        }
        let summary = notebook.summary();
        drop(notebook);
        kc_core::tool::ToolOutcome::ok(summary)
    }
}

/// `tools`, plus the `note` tool bound to this run's notebook.
///
/// The delegate family is installed once per runtime because every one of those
/// tools needs the live agent registry. A notebook is the opposite: it belongs to
/// one run (a child agent has its own), so the tool has to carry that run's
/// notebook, and a per-run registry is the only way to give it one. The tool list
/// the model is offered comes from this registry, so `note` is advertised exactly
/// where it can be used.
pub fn with_note_tool(
    tools: &kc_core::tool::ToolRegistry,
    notebook: Arc<Mutex<Notebook>>,
) -> kc_core::tool::ToolRegistry {
    let mut registry = tools.clone();
    registry.register(Arc::new(NoteTool::new(notebook)));
    registry
}

// ---------------------------------------------------------------------------
// last-resort elision
// ---------------------------------------------------------------------------

/// Marker left where part of a message body was elided by [`shrink_bodies`].
///
/// Distinct from the tool-output `[truncated: ...]` marker on purpose: that one
/// means "the tool produced more than the cap", this one means "the request did
/// not fit and the middle of this body was cut". Both are visible to the model.
pub const ELIDED_MARKER: &str =
    "\n\n[... middle elided: the request was over the context budget ...]\n\n";

/// Enough of a body to stay usable: the head names what it is, the tail keeps a
/// closing brace, a summary line or the end of a stack trace.
const ELIDE_MIN_KEEP: usize = 240;

/// The digest that stands in for elided bodies.
///
/// A digest on purpose (it starts with [`DIGEST_PREFIX`], so a later compaction
/// pins it and carries our own note forward instead of nesting a second one):
/// the model is told that what it is missing was cut for space, which is the
/// truth, rather than being handed a transcript that silently lost its middle.
///
/// The run's notebook rides along, exactly as it does in [`compact_notice`]: a
/// window small enough that nothing can be dropped whole is the case where the
/// model has the least transcript left, so it needs its own findings most.
fn elision_notice(window: &[Message], notes: &[String], max_chars: usize) -> String {
    let mut notice = format!(
        "{TRIM_PREFIX}the conversation was shortened to fit the context budget: the middle of \
         the largest messages and tool results was cut, keeping the start and the end of each. \
         The task and the newest turn are intact. Read a file again if you need an omitted part]"
    );
    for line in merge_notes(&carried_notes(window), notes, note_budget(max_chars)) {
        notice.push('\n');
        notice.push_str(&line);
    }
    notice.trim_end().to_string()
}

/// The whole elision pass for a window that cannot be compacted any further.
///
/// Keeps every message that may not be dropped and shortens the bodies of what
/// remains, largest first, until the rebuilt window fits `max_chars`. The pinned
/// first user message is bounded **last** and only as far as it must be: the
/// task is the one thing a long run must not forget, so its text is the last
/// thing to give up room.
///
/// Returns `None` when even elision cannot bring the window under budget (the
/// system prompt alone is that big), so the caller can leave the input alone
/// instead of writing half a page of markers over it.
fn shrink_window(
    window: &mut [Message],
    dropped: &[bool],
    pin: usize,
    max_chars: usize,
    notes: &[String],
) -> Option<(Vec<Message>, String)> {
    let notice = elision_notice(window, notes, max_chars);
    shrink_bodies(
        window,
        dropped,
        max_chars.saturating_sub(notice.chars().count()),
    );
    // Rebuild with the notice, then check what actually came out: `rebuild`
    // inserts the digest even when nothing was dropped, so the check has to be
    // on the list that will be sent, not on an estimate.
    let messages = rebuild(window.to_vec(), pin, dropped, notice.clone());
    let total: usize = messages.iter().map(message_chars).sum();
    if total <= max_chars {
        Some((messages, notice))
    } else {
        // Nothing is droppable and elision cannot reach the budget either (a
        // system prompt or attached images larger than the budget). Sending it
        // is the only option left, but never silently.
        tracing::warn!(
            budget = max_chars,
            sent = total,
            "context window cannot be brought under budget: nothing may be dropped and eliding \
             the remaining bodies was not enough. Raise `context_window` for this model or lower \
             `max_tool_output_bytes`."
        );
        None
    }
}

/// Shorten the bodies of the surviving messages until they fit `budget`.
///
/// Whole messages are never removed here - that is compaction's job - which is
/// what keeps a `tool_call` attached to its `tool_result` and the task attached
/// to the run. Each body keeps a head and a tail around [`ELIDED_MARKER`], so a
/// tool result still shows what it started with and how it ended.
///
/// Returns how many characters were elided (0 when the window already fitted).
fn shrink_bodies(window: &mut [Message], dropped: &[bool], budget: usize) -> usize {
    let mut total: usize = window
        .iter()
        .enumerate()
        .filter(|(i, _)| !dropped[*i])
        .map(|(_, m)| message_chars(m))
        .sum();
    if total <= budget {
        return 0;
    }
    let before = total;

    // Pinned prefix = system prompt, earlier digests, the first user turn. It is
    // shortened last, and the system prompt is never shortened at all: it is the
    // harness's own instructions, not conversation.
    let pin = pinned_prefix(window);

    // Largest first: one 60k tool result costs one elision, while shaving every
    // message in turn costs a marker in all of them.
    let mut order: Vec<usize> = (0..window.len())
        .filter(|i| !dropped[*i])
        .filter(|&i| window[i].role != Role::System && !is_digest(&window[i]))
        .collect();
    order.sort_by_key(|&i| std::cmp::Reverse(message_chars(&window[i])));

    for &i in &order {
        if total <= budget {
            break;
        }
        let is_task = i < pin && window[i].role == Role::User;
        // The task keeps more than a tool result does: it is the instruction,
        // not the output.
        let keep = if is_task {
            (budget / 8).max(ELIDE_MIN_KEEP)
        } else {
            ELIDE_MIN_KEEP
        };
        if let Some(saved) = elide_body(&mut window[i], keep) {
            total = total.saturating_sub(saved);
        }
    }

    // Still over: the task itself has to give. It keeps a head and a tail too,
    // so the model still knows what it was asked, minus the middle.
    if total > budget {
        for &i in &order {
            if total <= budget {
                break;
            }
            if i < pin && window[i].role == Role::User {
                if let Some(saved) = elide_body(&mut window[i], ELIDE_MIN_KEEP / 2) {
                    total = total.saturating_sub(saved);
                }
            }
        }
    }

    // Still over: images. One is charged [`IMAGE_CONTEXT_CHARS`] against the
    // budget (that is its token cost, not its byte cost) and no text elision can
    // shorten one, so a turn that attached more pictures than the budget can pay
    // for otherwise rides along at several times the budget while every body in
    // it is already a head and a tail. The newest images are kept - the last page
    // a read returned is the one being reasoned about - and the omission is
    // stated in the body, exactly as the byte cap states its own.
    if total > budget {
        total = drop_images(window, dropped, total, budget);
    }

    before.saturating_sub(total)
}

/// Characters the [`drop_images`] notice costs, charged before it is written so
/// the loop cannot end a few characters over the budget it just met.
const IMAGE_OMIT_MARKER_CHARS: usize = 96;

/// Drop images the budget cannot pay for, until `total` fits `budget`.
///
/// Carriers with more images give first, and the pinned task's own images go
/// last: what the run was *asked* to look at outlives what it merely looked at.
/// Only images are touched - removing a message is compaction's job, and it does
/// it in whole exchanges.
fn drop_images(window: &mut [Message], dropped: &[bool], mut total: usize, budget: usize) -> usize {
    let pin = pinned_prefix(window);
    let mut order: Vec<usize> = (0..window.len())
        .filter(|i| !dropped[*i] && !window[*i].images.is_empty())
        .collect();
    order.sort_by_key(|&i| (i < pin, std::cmp::Reverse(window[i].images.len())));
    for i in order {
        let mut omitted = 0usize;
        while !window[i].images.is_empty() && total.saturating_add(IMAGE_OMIT_MARKER_CHARS) > budget
        {
            // Oldest first inside one message: the tail is what was read last.
            window[i].images.remove(0);
            total = total.saturating_sub(IMAGE_CONTEXT_CHARS);
            omitted += 1;
        }
        if omitted > 0 {
            let marker =
                format!("\n[{omitted} image(s) omitted: the request was over the context budget]");
            total = total.saturating_add(marker.chars().count());
            window[i].content.push_str(&marker);
        }
        if total <= budget {
            break;
        }
    }
    total
}

/// Replace the middle of one message's bodies with [`ELIDED_MARKER`], keeping
/// `keep` characters at each end (UTF-8 safe). Returns the characters saved, or
/// `None` when every body is already short enough to leave alone.
///
/// Both charged bodies are shortened: `content` **and** `reasoning_content`.
/// A reasoning model's chain of thought is persisted with its turn and written
/// back into every later request (`kc_provider::request` re-emits
/// `reasoning_content`), and [`message_chars`] charges it against
/// `limits.context_max_chars` - so a pass that could only shorten `content`
/// would report a window as over budget and then send it that way, because the
/// only characters above the budget were reasoning it was not allowed to touch.
///
/// Tool-call *arguments* are deliberately not elided: cutting a call's arguments
/// changes what the call means and can leave invalid JSON, so they keep their
/// total budget even when that is what pushes a window over (and a turn's
/// arguments are bounded by the model's own output limit).
fn elide_body(message: &mut Message, keep: usize) -> Option<usize> {
    let mut saved = 0usize;
    if let Some(chars) = elide_text(&mut message.content, keep) {
        saved += chars;
    }
    if let Some(reasoning) = message.reasoning_content.as_mut() {
        if let Some(chars) = elide_text(reasoning, keep) {
            saved += chars;
        }
    }
    (saved > 0).then_some(saved)
}

/// One body, head + [`ELIDED_MARKER`] + tail. `None` when the body is short
/// enough to leave alone.
fn elide_text(text: &mut String, keep: usize) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let marker_chars = ELIDED_MARKER.chars().count();
    if chars.len() <= keep * 2 + marker_chars {
        return None;
    }
    let head: String = chars[..keep].iter().collect();
    let tail: String = chars[chars.len() - keep..].iter().collect();
    let body = format!("{head}{ELIDED_MARKER}{tail}");
    let saved = chars.len().saturating_sub(body.chars().count());
    if saved == 0 {
        return None;
    }
    *text = body;
    Some(saved)
}

/// Say so, once per compaction, that the request had to be elided.
///
/// A warning rather than a silent success: this is the path where the budget
/// could not be met by dropping history, so an install seeing it wants either a
/// bigger `context_window` or a smaller `max_tool_output_bytes`.
fn warn_elided(messages: &[Message], max_chars: usize) {
    let total: usize = messages.iter().map(message_chars).sum();
    tracing::warn!(
        budget = max_chars,
        sent = total,
        elided = messages
            .iter()
            .filter(|m| m.content.contains(ELIDED_MARKER))
            .count(),
        "context window could not be compacted under budget; message bodies were elided"
    );
}

/// Budget the window is compacted *down to* on a compaction.
///
/// Two goals pull against each other: leave enough headroom that the next
/// [`TURNS_OF_HEADROOM`] turns fit without another compaction (cache hits), and
/// do not throw away more of the conversation than the budget forces. The rule
/// is therefore "keep the newest turns, reserve 50 turns of room, never shrink
/// below [`MIN_WINDOW_PERCENT`] of the budget".
fn target_chars(messages: &[Message], max_chars: usize) -> usize {
    let turn = recent_exchange_chars(messages);
    let desired = max_chars.saturating_sub(turn.saturating_mul(TURNS_OF_HEADROOM));
    desired.max(max_chars / 100 * MIN_WINDOW_PERCENT)
}

/// The same rule as [`target_chars`], in messages instead of characters: how
/// many items a compaction aims for, so it does not run again on the very next
/// turn.
///
/// An item ceiling that compacted down to itself would be hit again by the next
/// tool turn and would compact again, which is one cache miss per turn - the
/// exact opposite of what compaction is for. So this reserves
/// [`TURNS_OF_HEADROOM`] turns of the newest exchange's *messages*, and never
/// aims below [`MIN_WINDOW_PERCENT`] of the ceiling.
///
/// Returns `usize::MAX` when there is no item ceiling, so the drop loop's
/// condition is inert.
fn target_messages(messages: &[Message], limits: ContextLimits) -> usize {
    if limits.max_messages == 0 {
        return usize::MAX;
    }
    let turn = recent_exchange_messages(messages);
    let desired = limits
        .max_messages
        .saturating_sub(turn.saturating_mul(TURNS_OF_HEADROOM));
    desired.max(limits.max_messages / 100 * MIN_WINDOW_PERCENT)
}

/// Messages in the exchange in flight: the newest assistant turn that asked for
/// tools plus every result answering it. `1` for a window with no such turn (a
/// user message alone), so the headroom arithmetic never multiplies by zero.
fn recent_exchange_messages(messages: &[Message]) -> usize {
    let start = current_exchange_start(messages);
    provider_message_count(&messages[start.min(messages.len())..]).max(1)
}

/// How many wire items `rebuild` will emit for a window of `survivors` messages
/// that carries `digests` older digests and `image_messages` image-bearing
/// sources: every survivor, minus replaced digests, plus one new digest, plus
/// the synthetic user message each image-bearing source expands into.
fn emitted_count(survivors: usize, digests: usize, image_messages: usize) -> usize {
    survivors
        .saturating_sub(digests)
        .saturating_add(1)
        .saturating_add(image_messages)
}

/// Say so, once per compaction, when the request that is about to be sent still
/// carries more items than `limits.max_messages` allows.
///
/// A warning rather than a failure, and a warning rather than silently sending a
/// request the gateway will refuse: there is genuinely nothing left to drop (the
/// pinned prefix, the exchange in flight and sticky findings are all protected),
/// so the operator has to choose between a bigger `max_context_messages` and a
/// shorter session. The request is *not* rewritten to satisfy the ceiling: every
/// message in it answers a call in it, and splitting one of those pairs is what
/// produces an invalid request instead of an oversized one.
fn warn_if_over_ceiling(messages: &[Message], limits: ContextLimits, reason: &str) {
    let sent = provider_message_count(messages);
    if !limits.within_messages(sent) {
        tracing::warn!(
            ceiling = limits.max_messages,
            sent,
            reason,
            "context window cannot be brought under limits.max_context_messages: the request is \
             sent as-is. Raise `limits.max_context_messages`, or set it to 0 for a provider with \
             no item limit."
        );
    }
}

/// Characters in the exchange in flight (the newest assistant turn that asked
/// for tools, plus its results). Used to size the headroom: a big turn needs a
/// big reserve, and a small one should not cost the whole window.
fn recent_exchange_chars(messages: &[Message]) -> usize {
    let start = current_exchange_start(messages);
    messages[start.min(messages.len())..]
        .iter()
        .map(message_chars)
        .sum()
}

/// Messages at the front of the window that are never dropped: the system
/// prompt, any older digest, and the first user turn (the task).
fn pinned_prefix(messages: &[Message]) -> usize {
    let mut end = 0usize;
    for (i, message) in messages.iter().enumerate() {
        if message.role == Role::System || is_digest(message) {
            end = i + 1;
            continue;
        }
        if message.role == Role::User {
            return i + 1;
        }
        break;
    }
    end
}

/// First message of the exchange in flight: the newest assistant turn that asked
/// for tools, together with every result answering it.
fn current_exchange_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|m| m.role == Role::Assistant && !m.tool_calls.is_empty())
        .unwrap_or(messages.len())
}

/// Is this message the compaction digest that replaced dropped turns?
///
/// The reserved `name` is runtime provenance; the visible prefix is not proof.
/// Ordinary persisted user rows have no name, and tool rows name their tool.
pub fn is_digest(message: &Message) -> bool {
    message.role == Role::User
        && message.name.as_deref() == Some(DIGEST_NAME)
        && message.content.starts_with(TRIM_PREFIX)
}

/// The prefix every compaction digest starts with.
pub const DIGEST_PREFIX: &str = TRIM_PREFIX;

/// Is this assistant turn a written finding (see [`NOTE_MIN_CHARS`])?
fn is_note(message: &Message) -> bool {
    message.role == Role::Assistant
        && message.tool_calls.is_empty()
        && message.content.trim().chars().count() >= NOTE_MIN_CHARS
}

/// Which messages may never be dropped: a system prompt, a digest, or a written
/// finding still inside the note budget (oldest released first).
fn sticky_flags(messages: &[Message], max_chars: usize) -> Vec<bool> {
    let mut sticky: Vec<bool> = messages
        .iter()
        .map(|m| m.role == Role::System || is_digest(m) || is_note(m))
        .collect();
    let budget = note_budget(max_chars);
    let mut used: usize = messages
        .iter()
        .filter(|m| is_note(m))
        .map(message_chars)
        .sum();
    // Release the oldest findings until the notes fit their budget.
    for (i, message) in messages.iter().enumerate() {
        if used <= budget {
            break;
        }
        if is_note(message) {
            used = used.saturating_sub(message_chars(message));
            sticky[i] = false;
        }
    }
    sticky
}

fn unit_chars(messages: &[Message], unit: &[usize]) -> usize {
    unit.iter().map(|&i| message_chars(&messages[i])).sum()
}

fn image_message_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| !message.images.is_empty())
        .count()
}

fn unit_image_messages(messages: &[Message], unit: &[usize]) -> usize {
    unit.iter()
        .filter(|&&index| !messages[index].images.is_empty())
        .count()
}

/// Rebuild the list: everything before `[start, end)` keeps its position, the
/// notice goes directly after the system prompt / pinned prefix, survivors follow.
fn rebuild(messages: Vec<Message>, pin: usize, dropped: &[bool], notice: String) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len() + 1);
    let mut inserted = false;
    for (i, message) in messages.into_iter().enumerate() {
        if is_digest(&message) {
            continue;
        }
        // The notice sits directly after the pinned prefix (system prompt and
        // the task), so those bytes never move.
        if !inserted && (i >= pin || dropped[i]) {
            push_digest(&mut out, notice.clone());
            inserted = true;
        }
        if dropped[i] {
            continue;
        }
        out.push(message);
    }
    if !inserted {
        push_digest(&mut out, notice);
    }
    out
}

fn push_digest(out: &mut Vec<Message>, notice: String) {
    if notice.trim().is_empty() {
        return;
    }
    // A `user` turn, not an `assistant` one: the model did not say this, and a
    // transcript that claims it did makes the model believe it already
    // summarised or decided something.
    let mut digest = Message::user(notice);
    digest.name = Some(DIGEST_NAME.to_string());
    out.push(digest);
}

/// What the model gets instead of the turns that were dropped.
///
/// A bare "N messages trimmed" leaves a long run blind to everything it learned
/// early on, and a digest that forgets *which* files were read makes the model
/// read them again. So: the run's own [notebook](Notebook) first - its
/// conclusions are worth more than any list of calls - then one trimmed line per
/// dropped user/assistant turn, the tool calls with their arguments, the tool
/// results counted, and the digest of an earlier compaction carried forward.
/// Newest dropped turns win when the digest budget runs out; oldest notes are
/// released first when the note budget does.
///
/// A stubbed result (see [`supersede`]) is *not* dropped, so it is never counted
/// here: the stub already says what it was.
fn compact_notice(
    messages: &[Message],
    dropped: &[bool],
    dropped_messages: usize,
    max_chars: usize,
    notes: &[String],
) -> String {
    let (digest_chars, line_chars) = digest_caps(max_chars);

    // The notebook and the note lines an earlier digest already carries, merged
    // and bounded by the note policy rather than by a cap of their own.
    let carried_notes = carried_notes(messages);
    let note_lines = merge_notes(&carried_notes, notes, note_budget(max_chars));
    let note_chars: usize = note_lines.iter().map(|line| line.chars().count() + 1).sum();
    // Notes are paid for out of the digest's own allowance, and they win: they
    // are the model's conclusions, the turn list is its history.
    let line_budget = digest_chars.saturating_sub(note_chars);

    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut tool_results = 0usize;
    for (i, message) in messages.iter().enumerate().rev() {
        if !dropped[i] {
            continue;
        }
        if is_digest(message) {
            continue;
        }
        let line = match message.role {
            Role::Tool => {
                tool_results += 1;
                continue;
            }
            Role::System => continue,
            Role::User => format!("- user: {}", one_line(&message.content, line_chars)),
            Role::Assistant => format!("- assistant: {}", assistant_line(message, line_chars)),
        };
        let chars = line.chars().count() + 1;
        if used + chars > line_budget {
            break;
        }
        used += chars;
        lines.push(line);
    }
    lines.reverse();

    // Carry an existing digest forward, newest lines first so the budget is
    // spent on the most recent knowledge. The note lines are already in
    // `note_lines`, deduplicated against the live notebook.
    let carried: Vec<String> = messages
        .iter()
        .filter(|m| is_digest(m))
        .flat_map(|m| digest_body_lines(&m.content))
        .filter(|line| !line.starts_with(NOTE_LINE_PREFIX))
        .collect();
    let mut notice = format!("{TRIM_PREFIX}{dropped_messages} earlier messages omitted");
    if tool_results > 0 {
        notice.push_str(&format!(" ({tool_results} tool results)"));
    }
    notice.push_str("; the work they describe is still done - do not redo it]\n");
    for line in &note_lines {
        notice.push_str(line);
        notice.push('\n');
    }
    let mut appended = 0usize;
    for line in lines.iter().chain(carried.iter()) {
        if appended + line.chars().count() + 1 > line_budget {
            break;
        }
        appended += line.chars().count() + 1;
        notice.push_str(line);
        notice.push('\n');
    }
    // The fixed explanatory header can exceed a deliberately tiny budget. Cap
    // the completed notice itself, not only its optional digest lines; otherwise
    // saturating subtraction still rebuilds an over-budget request.
    notice.trim_end().chars().take(max_chars).collect()
}

/// The note lines an earlier digest in the window already carries.
///
/// A notebook belongs to one run, so a finding from the previous run on the same
/// session survives only through a digest that names it. `compact_notice` and
/// [`elision_notice`] both read this so the two digests carry the same knowledge.
fn carried_notes(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| is_digest(m))
        .flat_map(|m| digest_body_lines(&m.content))
        .filter_map(|line| {
            line.strip_prefix(NOTE_LINE_PREFIX)
                .map(|text| text.to_string())
        })
        .collect()
}

/// The note lines of the digest: what earlier digests already carry plus the live
/// notebook, deduplicated, bounded at `budget` with the oldest released first.
///
/// Deliberately one list rather than two capped sections: a run's notebook is
/// per-run state, so a finding from the previous run survives only through a
/// digest line, and merging means the two are never charged twice for the same
/// finding or bounded by two different numbers.
fn merge_notes(carried: &[String], notes: &[String], budget: usize) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    for text in carried.iter().chain(notes.iter()) {
        if !merged.contains(text) {
            merged.push(text.clone());
        }
    }
    let mut used = 0usize;
    let mut kept: Vec<String> = Vec::new();
    for text in merged.iter().rev() {
        let line = format!(
            "{NOTE_LINE_PREFIX}{}",
            one_line(text, NOTE_DIGEST_LINE_CHARS)
        );
        let chars = line.chars().count() + 1;
        if used + chars > budget {
            break;
        }
        used += chars;
        kept.push(line);
    }
    kept.reverse();
    kept
}

/// The body lines of a digest created by an earlier compaction.
fn digest_body_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .skip(1)
        .filter(|line| line.starts_with("- "))
        .map(str::to_string)
        .collect()
}

fn assistant_line(message: &Message, line_chars: usize) -> String {
    let mut text = one_line(&message.content, line_chars);
    if message.tool_calls.is_empty() {
        return text;
    }
    let mut calls: Vec<String> = message
        .tool_calls
        .iter()
        .take(DIGEST_CALLS_PER_LINE)
        .map(|call| {
            let args = one_line(&call.arguments, DIGEST_ARGS_CHARS);
            if args.is_empty() || args == "{}" {
                call.name.clone()
            } else {
                format!("{}({args})", call.name)
            }
        })
        .collect();
    if message.tool_calls.len() > DIGEST_CALLS_PER_LINE {
        calls.push(format!(
            "+{} more",
            message.tool_calls.len() - DIGEST_CALLS_PER_LINE
        ));
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(&format!("[called {}]", calls.join(", ")));
    text
}

/// Size caps for the digest, scaled to the budget it lives in.
///
/// A fixed 4k digest would be harmless in a 240k budget and absurd in a 2k one,
/// so the digest may take at most an eighth of the budget and each line at most a
/// fortieth, with a floor that still leaves a recognizable fragment.
fn digest_caps(max_chars: usize) -> (usize, usize) {
    let total = DIGEST_MAX_CHARS.min((max_chars / 8).max(200));
    let line = DIGEST_LINE_CHARS.min((max_chars / 40).max(24));
    (total, line)
}

/// Collapse a message body to one digest line: newlines flattened, whitespace
/// squeezed, and a hard character cut (never a byte cut, so UTF-8 survives).
fn one_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut last_space = false;
    for ch in text.chars() {
        if count >= max_chars {
            out.push('\u{2026}');
            break;
        }
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
                count += 1;
                last_space = true;
            }
            continue;
        }
        out.push(ch);
        count += 1;
        last_space = false;
    }
    out.trim().to_string()
}

/// Move a drop boundary back so it never splits a tool exchange.
///
/// [`group_droppable`] only attaches the results that sit *before* `end`, so a
/// boundary that falls inside an exchange would drop the assistant turn that
/// declared the calls and keep the results after it: a `tool` message whose
/// `tool_call_id` is no longer declared anywhere, which providers reject with a
/// 400. When `end` lands inside a run of results, the boundary is clamped back to
/// the assistant turn that owns them, so the whole exchange stays in the
/// protected tail and is dropped as one unit later - never half of it.
///
/// A run with no owning assistant turn (an orphan from an old cancelled run) is
/// left where it is: splitting it creates no new orphan, and the pinned prefix
/// may own it.
fn clamp_end_to_exchange(messages: &[Message], start: usize, end: usize) -> usize {
    let end = end.min(messages.len());
    if end <= start || end >= messages.len() || messages[end].role != Role::Tool {
        return end;
    }
    // Walk back over the run of results to the first one.
    let mut first_result = end;
    while first_result > start && messages[first_result - 1].role == Role::Tool {
        first_result -= 1;
    }
    if first_result > start {
        let owner = first_result - 1;
        if messages[owner].role == Role::Assistant && !messages[owner].tool_calls.is_empty() {
            return owner;
        }
    }
    end
}

/// Split `messages[start..end]` into units that must be removed together:
/// an assistant message together with the tool results that answer it, or a
/// single message otherwise. System messages, digests and sticky findings are
/// never dropped.
fn group_droppable(
    messages: &[Message],
    start: usize,
    end: usize,
    sticky: &[bool],
) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut i = start;
    let end = end.min(messages.len());
    while i < end {
        if sticky.get(i).copied().unwrap_or(false) {
            i += 1;
            continue;
        }
        let mut unit = vec![i];
        let attach_results = (messages[i].role == Role::Assistant
            && !messages[i].tool_calls.is_empty())
            || messages[i].role == Role::Tool;
        let mut j = i + 1;
        if attach_results {
            // Orphan tool results (e.g. from an old cancelled run) stay grouped
            // with the following results so one is never alone.
            while j < end && messages[j].role == Role::Tool {
                unit.push(j);
                j += 1;
            }
        }
        groups.push(unit);
        i = j;
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::message::ToolCall;

    fn tool_msg(id: &str, body: &str) -> Message {
        Message::tool_result(id, "echo", body)
    }

    fn call_msg(id: &str) -> Message {
        Message::assistant(
            "",
            vec![ToolCall {
                id: id.into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
        )
    }

    #[test]
    fn tiny_budgets_cannot_underflow_the_notice_allowance() {
        let messages = vec![
            Message::system("sys"),
            Message::user("the original task"),
            Message::assistant("an old answer", vec![]),
            call_msg("call_old"),
            tool_msg("call_old", "an old tool result"),
            Message::user("the current request"),
            Message::assistant("a large protected answer", vec![]),
        ];
        for budget in [0, 1, 8, 64, 199, 200] {
            let notice = compact_notice(
                &messages,
                &vec![true; messages.len()],
                messages.len(),
                budget,
                &[],
            );
            assert!(
                notice.chars().count() <= budget,
                "the fixed notice exceeded budget {budget}: {notice:?}"
            );
            let fitted = fit(messages.clone(), budget, 1_000);
            assert!(!fitted.is_empty(), "fit lost every row at budget {budget}");

            let mut window = messages.clone();
            compact_run_window(&mut window, budget);
            assert!(
                !window.is_empty(),
                "compaction lost every row at budget {budget}"
            );
        }
    }

    #[test]
    fn small_conversations_are_untouched() {
        let msgs = vec![
            Message::system("sys"),
            Message::user("hello"),
            Message::assistant("hi", vec![]),
        ];
        let out = fit(msgs.clone(), 10_000, 10_000);
        assert_eq!(out.len(), msgs.len());
        assert_eq!(out[0].content, "sys");
    }

    #[test]
    fn tool_output_is_truncated_to_the_byte_budget() {
        let big = "x".repeat(4000);
        let out = fit(
            vec![
                Message::system("sys"),
                Message::user("u"),
                tool_msg("c1", &big),
            ],
            1_000_000,
            500,
        );
        let tool = out.iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(tool.content.len() < 1000);
        assert!(tool.content.contains("bytes omitted"));
    }

    #[test]
    fn trimming_keeps_system_and_last_user_turn() {
        let mut msgs = vec![Message::system("SYSTEM PROMPT"), Message::user("TASK")];
        for i in 0..20 {
            msgs.push(Message::user(format!(
                "old user {i} with some padding text"
            )));
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"y".repeat(500)));
            msgs.push(Message::assistant("ok", vec![]));
        }
        msgs.push(Message::user("CURRENT QUESTION"));
        let out = fit(msgs, 4000, 100_000);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[0].content, "SYSTEM PROMPT");
        let last = out.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content, "CURRENT QUESTION");
        let total: usize = out.iter().map(message_chars).sum();
        assert!(total <= 4000);
    }

    #[test]
    fn trimming_never_orphans_tool_messages() {
        let mut msgs = vec![Message::system("sys"), Message::user("first")];
        for i in 0..10 {
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"z".repeat(800)));
            msgs.push(Message::assistant("next", vec![]));
        }
        msgs.push(Message::user("last"));
        let out = fit(msgs, 3000, 100_000);
        // Every tool result must be preceded by an assistant tool call with the
        // same id somewhere earlier in the list.
        let mut seen_calls = std::collections::HashSet::new();
        for m in &out {
            for c in &m.tool_calls {
                seen_calls.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(seen_calls.contains(&id), "orphaned tool result {id}");
            }
        }
        assert!(out.last().unwrap().content == "last");
    }

    #[test]
    fn a_single_notice_reports_how_many_messages_were_dropped() {
        let mut msgs = vec![Message::system("sys"), Message::user("first")];
        for i in 0..6 {
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"q".repeat(600)));
        }
        msgs.push(Message::user("last"));
        let out = fit(msgs, 2000, 100_000);
        let notices: Vec<&Message> = out
            .iter()
            .filter(|m| m.content.starts_with(TRIM_PREFIX))
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].content.contains("messages omitted"));
        // The notice sits directly after the pinned task, and it is a user
        // turn: the model never said it.
        let task = out.iter().position(|m| m.content == "first").unwrap();
        assert_eq!(out[task + 1].content, notices[0].content);
        assert_eq!(notices[0].role, Role::User);
    }

    #[test]
    fn the_notice_carries_a_digest_of_what_was_dropped() {
        let mut msgs = vec![
            Message::system("sys"),
            Message::user("GOAL: fix the parser"),
        ];
        for i in 0..8 {
            msgs.push(Message::user(format!(
                "step {i}: rewrote the parser entry point"
            )));
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"z".repeat(400)));
            msgs.push(Message::assistant("done", vec![]));
        }
        msgs.push(Message::user("continue"));
        let out = fit(msgs, 2500, 100_000);
        let notice = out
            .iter()
            .find(|m| m.content.starts_with(TRIM_PREFIX))
            .expect("compaction notice");
        assert!(
            notice.content.contains("rewrote the parser"),
            "a dropped turn must survive in the digest: {}",
            notice.content
        );
        assert!(notice.content.contains("[called echo]"));
        // The task statement survives verbatim, outside the digest.
        assert!(
            out.iter().any(|m| m.content == "GOAL: fix the parser"),
            "the task must never be compacted away"
        );
        // The digest is summarised, never a copy of the transcript.
        assert!(notice.content.chars().count() <= DIGEST_MAX_CHARS + 200);
        // And it is counted inside the budget, not added on top of it.
        let total: usize = out.iter().map(message_chars).sum();
        assert!(total <= 2500, "budget respected: {total}");
    }

    #[test]
    fn the_digest_names_the_files_that_were_read() {
        let mut msgs = vec![Message::system("sys"), Message::user("task")];
        for i in 0..6 {
            msgs.push(Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"src/module_{i}.rs\"}}"),
                }],
            ));
            msgs.push(tool_msg(&format!("c{i}"), &"z".repeat(600)));
        }
        msgs.push(Message::user("last"));
        let out = fit(msgs, 2000, 100_000);
        let notice = out
            .iter()
            .find(|m| m.content.starts_with(TRIM_PREFIX))
            .expect("compaction notice");
        assert!(
            notice.content.contains("read_file({\"path\""),
            "the digest must say what was read, not just that a tool ran: {}",
            notice.content
        );
    }

    #[test]
    fn a_rolled_up_digest_does_not_nest() {
        let mut history = Vec::new();
        for i in 0..200 {
            history.push(Message::user(format!("step {i}")));
            history.push(call_msg(&format!("c{i}")));
            history.push(tool_msg(&format!("c{i}"), &"z".repeat(500)));
            history.push(Message::assistant("ok", vec![]));
            compact_run_window(&mut history, 8_000);
        }
        let digests = history.iter().filter(|m| is_digest(m)).count();
        assert_eq!(digests, 1, "exactly one digest stays in the window");
        assert!(is_digest(&history[1]), "{}", history[1].content);
    }

    /// A written finding is the only durable record a reasoning model leaves.
    /// Compaction must not throw it away while it is in budget.
    #[test]
    fn a_written_finding_survives_compaction() {
        let finding = format!(
            "FINDING: {}",
            "a stale digest is rebuilt every turn. ".repeat(12)
        );
        let mut msgs = vec![
            Message::system("sys"),
            Message::user("TASK: review the runtime"),
            Message::assistant(finding.clone(), vec![]),
        ];
        for i in 0..40 {
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"z".repeat(500)));
        }
        msgs.push(Message::user("keep going"));
        let out = fit(msgs, 4_000, 100_000);
        assert!(
            out.iter().any(|m| m.content == finding),
            "the finding was compacted away"
        );
    }

    #[test]
    fn findings_are_released_oldest_first_once_they_exceed_the_note_budget() {
        let note = |i: usize| Message::assistant(format!("{i}: {}", "x".repeat(900)), vec![]);
        let msgs = vec![
            Message::system("sys"),
            Message::user("task"),
            note(0),
            Message::assistant("ok", vec![]),
            note(1),
        ];
        // A 2_000-char budget holds one ~900-char finding, not two.
        let sticky = sticky_flags(&msgs, 2_000);
        assert!(sticky[0], "the system prompt is always sticky");
        assert!(!sticky[2], "the oldest finding must be releasable");
        assert!(sticky[4], "the newest finding stays");
    }

    #[test]
    fn unbudgeted_system_prompt_is_never_dropped_even_if_huge() {
        let msgs = vec![Message::system("s".repeat(5000)), Message::user("hello")];
        let out = fit(msgs, 100, 100_000);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[1].content, "hello");
    }
}

#[cfg(test)]
mod run_window_tests {
    use super::*;
    use kc_core::message::ToolCall;

    fn tool_msg(id: &str, body: &str) -> Message {
        Message::tool_result(id, "echo", body)
    }

    fn call_msg(id: &str) -> Message {
        Message::assistant(
            "",
            vec![ToolCall {
                id: id.into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
        )
    }

    fn exchange(i: usize, body: usize) -> Vec<Message> {
        vec![
            Message::user(format!("step {i}: inspect the parser")),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"src/{i}.rs\"}}"),
                }],
            ),
            Message::tool_result(
                format!("c{i}"),
                "read_file",
                format!("{i}:{}", "x".repeat(body)),
            ),
            Message::assistant("done", vec![]),
        ]
    }

    #[test]
    fn a_long_run_window_stays_inside_the_budget() {
        let mut history = vec![Message::user("TASK: audit the runtime")];
        for i in 0..300 {
            history.extend(exchange(i, 2_000));
        }
        let before: usize = history.iter().map(message_chars).sum();
        assert!(before > 600_000, "fixture should be far over budget");

        compact_run_window(&mut history, 120_000);

        let after: usize = history.iter().map(message_chars).sum();
        assert!(
            after <= 120_000,
            "window must respect the budget: {after} chars"
        );
        assert!(
            after * 4 < before,
            "window should shrink hard: {before} -> {after}"
        );
        // The newest exchange is still there: the model keeps the turn it is in.
        assert!(history.iter().any(|m| m.content.contains("step 299")));
        // Nothing in the window may be an orphaned tool result.
        let mut seen = std::collections::HashSet::new();
        for m in &history {
            for c in &m.tool_calls {
                seen.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(
                    seen.contains(&id),
                    "orphaned tool result {id} at the front of the window"
                );
            }
        }
        // The dropped turns left a digest behind, directly after the task.
        assert!(is_digest(&history[1]), "{}", history[1].content);
        // The task statement survives.
        assert!(history
            .iter()
            .any(|m| m.content == "TASK: audit the runtime"));
    }

    #[test]
    fn a_short_run_window_is_untouched() {
        let mut history = Vec::new();
        for i in 0..3 {
            history.extend(exchange(i, 10));
        }
        let before = history.clone();
        compact_run_window(&mut history, 120_000);
        assert_eq!(history.len(), before.len());
    }

    #[test]
    fn repeated_compaction_keeps_the_rolling_summary_bounded() {
        let mut history = Vec::new();
        for round in 0..20 {
            for i in 0..50 {
                history.extend(exchange(round * 50 + i, 1_000));
            }
            compact_run_window(&mut history, 40_000);
            let total: usize = history.iter().map(message_chars).sum();
            assert!(
                total <= 40_000,
                "round {round}: window grew to {total} chars"
            );
        }
    }

    /// The cache invariant: between two compactions, the request prefix must be
    /// byte-identical. Modelled on the real subagent transcript that showed a
    /// 1.6% median hit rate, with an exchange close to the real ~1.6k chars.
    #[test]
    fn the_window_front_is_stable_between_compactions() {
        let budget = 240_000;
        let mut history = vec![Message::user("TASK: review the runtime")];
        let mut snapshots: Vec<String> = Vec::new();
        let mut compactions = 0usize;
        for i in 0..400 {
            history.extend(exchange(i, 1_500));
            // the model call happens before the next turn is appended
            snapshots.push(render(&history));
            let before = history.len();
            compact_run_window(&mut history, budget);
            if history.len() < before {
                compactions += 1;
            }
        }
        // A compaction is the only thing that may move the front of the window.
        let mut misses = 0usize;
        let mut shared = 0usize;
        for pair in snapshots.windows(2) {
            let common = common_prefix(&pair[0], &pair[1]) * 100 / pair[0].len().max(1);
            shared += common;
            if common < 98 {
                misses += 1;
            }
        }
        let mean = shared / (snapshots.len() - 1);
        assert!(
            mean >= 97,
            "mean shared prefix is only {mean}% ({} compactions over {} requests)",
            compactions,
            snapshots.len()
        );
        assert!(
            compactions * 25 <= snapshots.len(),
            "{compactions} compactions over {} requests is more than one per 25",
            snapshots.len()
        );
        assert!(
            misses <= compactions + 1,
            "a miss that is not a compaction: {misses} misses, {compactions} compactions"
        );
    }

    #[test]
    fn compaction_never_drops_the_turn_in_flight() {
        // One turn with 60 parallel tool calls: far more than PROTECTED_TAIL.
        let mut history = vec![Message::user("TASK")];
        history.push(Message::assistant(
            "",
            (0..60)
                .map(|i| ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"f{i}.rs\"}}"),
                })
                .collect(),
        ));
        for i in 0..60 {
            history.push(Message::tool_result(
                format!("c{i}"),
                "read_file",
                "z".repeat(2_000),
            ));
        }
        compact_run_window(&mut history, 40_000);
        // Every one of the 60 results survives, even though the window is over
        // the message-count protection and over the char budget.
        let kept = history.iter().filter(|m| m.role == Role::Tool).count();
        assert_eq!(kept, 60, "the turn in flight lost {}/60 results", 60 - kept);
    }

    /// The `len - PROTECTED_TAIL` boundary can fall *inside* an exchange: the
    /// assistant turn that declared the calls is droppable, its results are past
    /// the boundary and survive. That is a `tool` message with no preceding call -
    /// the 400 that `history::repair_history` exists to prevent. The boundary must
    /// move back to the start of the exchange instead.
    #[test]
    fn a_boundary_inside_a_tool_exchange_never_orphans_its_results() {
        let mut history = vec![Message::user("TASK: audit the runtime")];
        for i in 0..20 {
            history.extend(exchange(i, 2_000));
        }
        // Two filler turns put `len - PROTECTED_TAIL` on the tool result of the
        // 14th exchange (index 55 of 87), whose call is at index 54.
        history.push(Message::assistant("filler", vec![]));
        history.push(Message::assistant("filler", vec![]));
        history.extend(exchange(20, 2_000));

        let boundary = history.len() - PROTECTED_TAIL;
        assert_eq!(
            history[boundary].role,
            Role::Tool,
            "fixture: boundary on a result"
        );
        assert_eq!(history[boundary - 1].role, Role::Assistant);
        assert!(!history[boundary - 1].tool_calls.is_empty());
        let straddled = history[boundary].tool_call_id.clone().unwrap();

        compact_run_window(&mut history, 20_000);

        // Every surviving result must still be preceded by the call that declared
        // it, and the straddled exchange must be kept whole rather than halved.
        let mut seen = std::collections::HashSet::new();
        for m in &history {
            for c in &m.tool_calls {
                seen.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(
                    seen.contains(&id),
                    "orphaned tool result {id}: its call was dropped at the boundary"
                );
            }
        }
        assert!(
            history
                .iter()
                .any(|m| m.tool_calls.iter().any(|c| c.id == straddled)),
            "the exchange at the boundary lost its call but kept its result"
        );
    }

    /// A short history is not an exemption from the budget.
    ///
    /// `compact_run_window` used to return early for `len <= PROTECTED_TAIL`,
    /// which is every window of a run whose tool results are large but few: the
    /// shipped defaults (8 messages, one 60k-char tool result) sent 60,061 chars
    /// against a 32,000 budget, eight times over, and re-cloned it into every
    /// request. Few messages must still mean "fits the budget".
    #[test]
    fn a_short_run_window_still_respects_the_budget() {
        let mut history = vec![
            Message::system("sys"),
            Message::user("TASK: summarise this log"),
            Message::assistant("on it", vec![]),
        ];
        // Eight messages total: well under PROTECTED_TAIL, over any budget.
        history.push(call_msg("c1"));
        history.push(tool_msg("c1", &"L".repeat(40_000)));
        history.push(Message::assistant("reading further", vec![]));
        history.push(call_msg("c2"));
        history.push(tool_msg("c2", &"M".repeat(20_000)));
        assert!(
            history.len() <= PROTECTED_TAIL,
            "fixture: {} messages must stay inside the protected tail",
            history.len()
        );
        let before: usize = history.iter().map(message_chars).sum();
        assert!(before > 60_000, "fixture should be far over budget");

        compact_run_window(&mut history, 32_000);

        let after: usize = history.iter().map(message_chars).sum();
        assert!(
            after <= 32_000,
            "a {}-message window sent {after} chars against a 32_000 budget",
            history.len()
        );
        // The newest messages are still there - elision shortens bodies, it never
        // drops the protected tail.
        let results = history.iter().filter(|m| m.role == Role::Tool).count();
        assert_eq!(results, 2, "the newest tool results must survive");
        // Nothing was orphaned: every surviving result keeps the call that
        // declared it, so the provider still accepts the request.
        let mut seen = std::collections::HashSet::new();
        for m in &history {
            for c in &m.tool_calls {
                seen.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(seen.contains(&id), "orphaned tool result {id}");
            }
        }
        // The task is still there and still recognisable, and the elision is
        // stated rather than silent.
        assert!(
            history
                .iter()
                .any(|m| m.content.starts_with("TASK: summarise")),
            "the task must survive"
        );
        assert!(
            history.iter().any(is_digest),
            "the window must say it was shortened: {:?}",
            history.first().map(|m| m.content.clone())
        );
        assert!(
            history.iter().any(|m| m.content.contains(ELIDED_MARKER)),
            "an elided body must carry the marker"
        );
    }

    /// Every run window must stay bounded even while a single turn is in flight.
    /// The 60-parallel-call fixture is over budget in *both* directions: more
    /// results than [`PROTECTED_TAIL`] messages and more characters than the
    /// budget. Keeping every result is not a licence to send them whole.
    #[test]
    fn a_single_oversized_turn_is_elided_rather_than_sent_over_budget() {
        let budget = 40_000;
        let mut history = vec![Message::user("TASK")];
        history.push(Message::assistant(
            "",
            (0..60)
                .map(|i| ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"f{i}.rs\"}}"),
                })
                .collect(),
        ));
        for i in 0..60 {
            history.push(Message::tool_result(
                format!("c{i}"),
                "read_file",
                "z".repeat(2_000),
            ));
        }
        let before: usize = history.iter().map(message_chars).sum();
        assert!(before > budget * 2, "fixture: {before} chars");

        compact_run_window(&mut history, budget);

        let kept = history.iter().filter(|m| m.role == Role::Tool).count();
        assert_eq!(kept, 60, "the turn in flight lost {}/60 results", 60 - kept);
        let after: usize = history.iter().map(message_chars).sum();
        assert!(
            after <= budget,
            "an in-flight turn sent {after} chars against a {budget} budget"
        );
    }

    /// The same escape in `fit`: a subagent talks to one task for the whole run,
    /// so there is no user boundary to drop behind. It used to hand back the
    /// over-budget window untouched.
    #[test]
    fn fit_still_bounds_a_conversation_with_no_droppable_exchange() {
        let task = "TASK: review one file";
        let msgs = vec![
            Message::system("sys"),
            Message::user(task),
            Message::assistant("reading", vec![]),
            call_msg("c1"),
            tool_msg("c1", &"q".repeat(30_000)),
            Message::assistant("done", vec![]),
        ];
        let before: usize = msgs.iter().map(message_chars).sum();
        assert!(before > 10_000, "fixture: {before} chars");

        let out = fit(msgs, 10_000, 100_000);

        let total: usize = out.iter().map(message_chars).sum();
        assert!(
            total <= 10_000,
            "a request with nothing droppable was sent at {total} chars against a 10_000 budget"
        );
        assert!(
            out.iter().any(|m| m.content == task),
            "the task survives elision"
        );
        let mut seen = std::collections::HashSet::new();
        for m in &out {
            for c in &m.tool_calls {
                seen.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(seen.contains(&id), "orphaned tool result {id}");
            }
        }
    }

    fn render(history: &[Message]) -> String {
        history
            .iter()
            .map(|m| format!("{:?}|{}|{}", m.role, m.content, m.tool_calls.len()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn common_prefix(a: &str, b: &str) -> usize {
        a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
    }
}

#[cfg(test)]
mod image_budget_tests {
    use super::*;
    fn image(size: usize) -> kc_core::message::ImageAttachment {
        kc_core::message::ImageAttachment {
            data: "a".repeat(size),
            media_type: "image/png".into(),
            ..Default::default()
        }
    }
    #[test]
    fn screenshot_is_charged_as_model_estimate_and_preserved() {
        let mut user = Message::user("task");
        user.images.push(image(1_000_000));
        assert_eq!(message_chars(&user), 4 + IMAGE_CONTEXT_CHARS);
        let mut history = vec![Message::system("system"), user];
        compact_run_window(&mut history, 240_000);
        let out = fit(history, 240_000, 24_000);
        assert_eq!(out[1].images.len(), 1);
        assert!(!out[1].content.contains("omitted"));
    }
    #[test]
    fn byte_cap_keeps_newest_and_marks_omissions_in_fit_and_history() {
        let mut old = Message::user("old");
        old.images.push(image(IMAGE_BYTES_BUDGET / 2));
        let mut newest = Message::user("new");
        newest.images.push(image(IMAGE_BYTES_BUDGET / 2));
        let input = vec![old, newest];
        for out in [fit(input.clone(), 240_000, 24_000), {
            let mut history = input;
            compact_run_window(&mut history, 240_000);
            history
        }] {
            assert!(out[0].images.is_empty());
            assert!(out[0].content.contains("image(s) omitted"));
            assert_eq!(out[1].images.len(), 1);
        }
    }
    #[test]
    fn oversized_newest_image_is_visibly_omitted() {
        let mut user = Message::user("task");
        user.images.push(image(IMAGE_BYTES_BUDGET + 1));
        let out = fit(vec![user], 240_000, 24_000);
        assert!(out[0].images.is_empty());
        assert!(out[0]
            .content
            .contains("encoded image byte budget exceeded"));
    }

    #[test]
    fn fitted_request_and_held_window_match_the_real_provider_wire_count() {
        let mut input = vec![Message::system("system"), Message::user("task")];
        for index in 0..5 {
            let id = format!("image-{index}");
            input.push(Message::assistant(
                "",
                vec![kc_core::message::ToolCall {
                    id: id.clone(),
                    name: "read_file".into(),
                    arguments: format!(r#"{{"path":"src/{index}.png"}}"#),
                }],
            ));
            input.push(
                Message::tool_result(&id, "read_file", format!("image {index}"))
                    .with_images(vec![image(16), image(16)]),
            );
            input.push(Message::assistant(format!("looked at {index}"), vec![]));
            input.push(Message::user(if index == 4 { "finish" } else { "next" }));
        }
        let limits = ContextLimits::new(240_000, 12);

        let fitted = fit_with_limits(input.clone(), limits, 24_000);
        assert!(
            fitted.len() < input.len(),
            "the image expansion must trigger fitting"
        );
        assert_eq!(
            provider_message_count(&fitted),
            kc_provider::request::serialized_message_count(&fitted),
            "runtime accounting must equal actual OpenAI request serialization"
        );
        assert!(kc_provider::request::serialized_message_count(&fitted) <= limits.max_messages);

        // Held-window regression: 39 source items formerly looked like a perfect
        // fit after reserving the system prompt, but four image carriers expand it
        // to 44 wire items. Give it a tail longer than PROTECTED_TAIL so whole old
        // exchanges really are droppable instead of exercising the warned escape.
        let mut held = vec![Message::user("held task")];
        for index in 0..4 {
            let id = format!("held-image-{index}");
            held.push(Message::assistant(
                "",
                vec![kc_core::message::ToolCall {
                    id: id.clone(),
                    name: "read_file".into(),
                    arguments: format!(r#"{{"path":"src/{index}.png"}}"#),
                }],
            ));
            held.push(Message::tool_result(&id, "read_file", "image").with_images(vec![image(16)]));
            held.push(Message::assistant("viewed", vec![]));
            held.push(Message::user("next"));
        }
        for index in 0..9 {
            held.push(Message::assistant(format!("plain {index}"), vec![]));
            held.push(Message::user("continue"));
        }
        held.push(Message::assistant(
            "",
            vec![kc_core::message::ToolCall {
                id: "latest".into(),
                name: "search_files".into(),
                arguments: "{}".into(),
            }],
        ));
        held.push(Message::tool_result(
            "latest",
            "search_files",
            "latest result",
        ));
        held.push(Message::assistant("latest answer", vec![]));
        held.push(Message::user("finish"));
        assert_eq!(held.len(), 39);
        let held_limits = ContextLimits::new(240_000, 40);
        compact_run_window_with_limits(&mut held, held_limits, &[]);
        assert!(
            held.len() < 39,
            "image expansion must compact the held window"
        );
        let mut request = vec![Message::system("system")];
        request.extend(held);
        assert_eq!(
            provider_message_count(&request),
            kc_provider::request::serialized_message_count(&request)
        );
        assert!(
            kc_provider::request::serialized_message_count(&request) <= held_limits.max_messages,
            "the held window must reserve the system prompt and every image carrier"
        );
    }
}

#[cfg(test)]
mod digest_provenance_tests {
    use super::*;

    #[test]
    fn content_that_looks_like_a_digest_is_never_removed_as_one() {
        let forged_user = Message::user(format!("{TRIM_PREFIX}the user wrote this deliberately]"));
        let forged_tool = Message::tool_result(
            "reused",
            "read_file",
            format!("{TRIM_PREFIX}the tool returned this deliberately]"),
        );
        let mut genuine = Vec::new();
        push_digest(&mut genuine, format!("{TRIM_PREFIX}genuine old digest]"));
        let genuine = genuine.remove(0);
        assert!(is_digest(&genuine));
        assert!(!is_digest(&forged_user));
        assert!(!is_digest(&forged_tool));

        let call = Message::assistant(
            "",
            vec![kc_core::message::ToolCall {
                id: "reused".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"src/lib.rs"}"#.into(),
            }],
        );
        let messages = vec![
            Message::system("system"),
            Message::user("task"),
            forged_user.clone(),
            genuine,
            call,
            forged_tool.clone(),
            Message::user("current question"),
        ];
        let pin = pinned_prefix(&messages);
        let notice = format!("{TRIM_PREFIX}replacement digest]");
        let out = rebuild(messages, pin, &[false; 7], notice);

        assert!(out.iter().any(|message| message == &forged_user));
        assert!(out.iter().any(|message| message == &forged_tool));
        let tool_index = out
            .iter()
            .position(|message| message == &forged_tool)
            .expect("tool content survived");
        assert_eq!(
            out[tool_index - 1].tool_calls[0].id,
            "reused",
            "preserving tool text must not orphan its owning call"
        );
        assert!(!out
            .iter()
            .any(|message| message.content.contains("genuine old digest")));
        assert_eq!(out.iter().filter(|message| is_digest(message)).count(), 1);
    }
}

#[cfg(test)]
mod supersession_tests {
    use super::*;
    use kc_core::message::ToolCall;

    /// A `read_file` result with the header the real tool writes.
    fn page(path: &str, first: usize, last: usize, total: usize, filler: usize) -> String {
        let body = "x".repeat(filler);
        format!(
            "{path} — lines {first}-{last} of {total} ({} bytes)\n{body}",
            filler + 40
        )
    }

    fn call(id: &str, name: &str, arguments: &str) -> Message {
        Message::assistant(
            "",
            vec![ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: arguments.into(),
            }],
        )
    }

    fn result(id: &str, name: &str, body: String) -> Message {
        Message::tool_result(id, name, body)
    }

    /// One assistant turn with several `read_file` calls, as a parallel tool call
    /// arrives: `(call id, offset)`.
    fn calls(pages: &[(&str, usize)]) -> Message {
        Message::assistant(
            "",
            pages
                .iter()
                .map(|(id, offset)| ToolCall {
                    id: (*id).into(),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"src/parser.rs\",\"offset\":{offset}}}"),
                })
                .collect(),
        )
    }

    fn snapshot(messages: &[Message]) -> String {
        messages
            .iter()
            .map(|m| format!("{:?}|{}|{:?}", m.role, m.content, m.tool_call_id))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A window is a valid request when every `tool` message answers a call that
    /// is still declared above it, and every call is answered exactly once.
    fn assert_valid_request(messages: &[Message]) {
        let mut declared: HashSet<String> = HashSet::new();
        let mut answered: HashSet<String> = HashSet::new();
        for message in messages {
            for call in &message.tool_calls {
                declared.insert(call.id.clone());
            }
            if message.role == Role::Tool {
                let id = message
                    .tool_call_id
                    .clone()
                    .expect("a tool result names its call");
                assert!(declared.contains(&id), "orphaned tool result {id}");
                assert!(answered.insert(id.clone()), "duplicate result for {id}");
            }
        }
        for call in messages.iter().flat_map(|m| &m.tool_calls) {
            assert!(
                answered.contains(&call.id),
                "call {} has no result",
                call.id
            );
        }
        assert_eq!(messages[0].role, Role::System);
    }

    #[test]
    fn an_earlier_page_of_the_same_file_is_retired_and_the_newest_is_kept() {
        let input = vec![
            Message::system("sys"),
            Message::user("TASK: review src/app.tsx"),
            call("c1", "read_file", r#"{"path":"src/app.tsx","offset":1}"#),
            result("c1", "read_file", page("src/app.tsx", 1, 400, 812, 900)),
            Message::assistant("keep going", vec![]),
            call("c2", "read_file", r#"{"path":"src/app.tsx","offset":401}"#),
            result("c2", "read_file", page("src/app.tsx", 401, 812, 812, 900)),
            Message::user("now summarise it"),
        ];
        let before: usize = input.iter().map(message_chars).sum();
        // One character over the budget: the pass runs, the stubs alone bring the
        // window back inside it, and nothing has to be dropped - so the assertions
        // are about supersession and nothing else.
        let budget = before - 1;

        let outcome = fit_detailed(input.clone(), budget, 100_000);
        let out = &outcome.messages;

        let old = out
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("c1"))
            .expect("the first page is still a message");
        assert!(is_superseded(old), "{}", old.content);
        // The stub names the tool, the path and the range, says what replaced it,
        // and says the call is allowed again.
        assert!(old.content.contains("read_file"), "{}", old.content);
        assert!(old.content.contains("src/app.tsx"), "{}", old.content);
        assert!(
            old.content.contains("lines 1-400 of 812"),
            "{}",
            old.content
        );
        assert!(
            old.content.contains("replaced by a later call in this run"),
            "{}",
            old.content
        );
        assert!(old.content.contains("again"), "{}", old.content);
        assert_eq!(old.content.lines().count(), 1, "the stub is one line");
        assert_eq!(old.role, Role::Tool);

        let new = out
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("c2"))
            .expect("the newest page survives");
        assert!(!is_superseded(new));
        assert!(new
            .content
            .starts_with("src/app.tsx — lines 401-812 of 812"));

        // The saving is in the report the module already produces, as part of
        // what compaction took out of the window, and no message was removed.
        assert!(outcome.chars() < before);
        assert_eq!(
            outcome.compacted_chars(),
            message_chars(&input[3]) - message_chars(old)
        );
        assert_eq!(out.len(), input.len(), "nothing was removed");
        assert!(outcome.dropped.iter().all(|dropped| !dropped));
        assert_valid_request(out);

        // The same pass runs on the window the loop holds, in place.
        let mut history = input;
        compact_run_window(&mut history, budget);
        assert!(history.iter().any(is_superseded));
        assert_valid_request(&history);
    }

    #[test]
    fn a_stub_keeps_the_tool_call_pairing_valid() {
        let mut input = vec![
            Message::system("sys"),
            Message::user("TASK: audit the parser"),
            Message::assistant("reading", vec![]),
        ];
        for i in 0..6 {
            input.push(call(
                &format!("c{i}"),
                "read_file",
                r#"{"path":"src/parser.rs"}"#,
            ));
            input.push(result(
                &format!("c{i}"),
                "read_file",
                page("src/parser.rs", 1 + i * 10, 10 + i * 10, 900, 1_200),
            ));
        }
        input.push(Message::user("continue"));
        let total: usize = input.iter().map(message_chars).sum();

        // From a window that is barely over budget down to elision territory:
        // stubbing must never produce a `tool` message whose call is gone.
        for budget in [total - 1, total / 2, 2_000, 200] {
            let outcome = fit_detailed(input.clone(), budget, 100_000);
            assert_valid_request(&outcome.messages);
            assert!(
                outcome.prepared.iter().any(is_superseded),
                "budget {budget}: the older pages should be stubs"
            );

            let mut history = input.clone();
            compact_run_window(&mut history, budget);
            assert_valid_request(&history);
        }
    }

    #[test]
    fn supersession_does_not_run_when_the_window_fits() {
        // Two pages of the same file, far inside the budget: the request must be
        // byte-identical to the input, or every fitting turn would cost a cache
        // miss.
        let input = vec![
            Message::system("sys"),
            Message::user("TASK: review src/app.tsx"),
            call("c1", "read_file", r#"{"path":"src/app.tsx"}"#),
            result("c1", "read_file", page("src/app.tsx", 1, 400, 812, 900)),
            Message::user("again"),
            call("c2", "read_file", r#"{"path":"src/app.tsx","offset":401}"#),
            result("c2", "read_file", page("src/app.tsx", 401, 812, 812, 900)),
            Message::user("summarise"),
        ];
        let budget = 100_000;
        let out = fit(input.clone(), budget, 100_000);
        assert_eq!(snapshot(&out), snapshot(&input));

        let mut history = input.clone();
        compact_run_window(&mut history, budget);
        assert_eq!(snapshot(&history), snapshot(&input));

        // Not one body was retired, and the report says so.
        let outcome = fit_detailed(input, budget, 100_000);
        assert_eq!(outcome.superseded_chars.iter().sum::<usize>(), 0);
        assert_eq!(outcome.compacted_chars(), 0);
    }

    #[test]
    fn the_task_and_the_exchange_in_flight_are_never_superseded() {
        let input = vec![
            Message::system("sys"),
            Message::user("TASK: audit the parser"),
            call("old", "read_file", r#"{"path":"src/parser.rs"}"#),
            result(
                "old",
                "read_file",
                page("src/parser.rs", 1, 400, 900, 1_500),
            ),
            Message::assistant("reading two pages of the same file", vec![]),
            // The turn in flight reads the same file twice *in one turn*, which is
            // what a parallel tool call looks like: both results are newer than the
            // page above, and both are what the model is reasoning about right now.
            calls(&[("a", 1), ("b", 41)]),
            result("a", "read_file", page("src/parser.rs", 1, 40, 900, 1_500)),
            result("b", "read_file", page("src/parser.rs", 41, 80, 900, 1_500)),
        ];
        let budget = 3_000;
        assert!(
            input.iter().map(message_chars).sum::<usize>() > budget,
            "fixture must be over budget"
        );

        for out in [fit(input.clone(), budget, 100_000), {
            let mut history = input.clone();
            compact_run_window(&mut history, budget);
            history
        }] {
            assert_eq!(
                out.iter()
                    .find(|m| m.content == "TASK: audit the parser")
                    .map(|m| m.content.clone()),
                Some("TASK: audit the parser".to_string()),
                "the task must survive verbatim"
            );
            // The stale page above the turn in flight *is* retired - otherwise the
            // test would pass without the pass running at all.
            let stale = out
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some("old"))
                .expect("the older page is still a message");
            assert!(is_superseded(stale), "{}", stale.content);
            for id in ["a", "b"] {
                let message = out
                    .iter()
                    .find(|m| m.tool_call_id.as_deref() == Some(id))
                    .expect("the exchange in flight keeps both results");
                assert!(
                    !is_superseded(message),
                    "a result inside the turn in flight was stubbed: {}",
                    message.content
                );
            }
            assert_valid_request(&out);
        }
    }

    #[test]
    fn reused_call_ids_keep_arguments_from_their_own_assistant_occurrence() {
        let old_body = format!("OLD result for src/old.rs\n{}", "o".repeat(900));
        let new_body = format!("NEW result for src/new.rs\n{}", "n".repeat(900));
        let input = vec![
            Message::system("sys"),
            Message::user("TASK"),
            call("reused", "read_file", r#"{"path":"src/old.rs"}"#),
            result("reused", "read_file", old_body.clone()),
            Message::assistant("now look at another file", vec![]),
            call("reused", "read_file", r#"{"path":"src/new.rs"}"#),
            result("reused", "read_file", new_body.clone()),
            Message::user("compare them"),
        ];
        let outcome = fit_detailed(input.clone(), total_of(&input) - 1, 100_000);
        assert_eq!(outcome.prepared[3].content, old_body);
        assert_eq!(outcome.prepared[6].content, new_body);
        assert!(outcome.superseded_chars.iter().all(|saved| *saved == 0));

        // Reusing an id does not disable legitimate supersession either: when the
        // owning occurrences ask the same question, the older result is stale.
        let mut same_question = input;
        same_question[5] = call("reused", "read_file", r#"{"path":"src/old.rs"}"#);
        same_question[6] = result("reused", "read_file", old_body.clone());
        let total = total_of(&same_question);
        let outcome = fit_detailed(same_question, total - 1, 100_000);
        assert!(
            is_superseded(&outcome.prepared[3]),
            "{}",
            outcome.prepared[3].content
        );
        assert!(!is_superseded(&outcome.prepared[6]));
    }

    #[test]
    fn a_different_question_about_the_same_path_is_not_a_replacement() {
        // Same path, different pattern: a later search does not make the earlier
        // answer worthless.
        let input = vec![
            Message::system("sys"),
            Message::user("TASK: find the call sites"),
            call("s1", "search_files", r#"{"pattern":"parse("}"#),
            result(
                "s1",
                "search_files",
                format!("src/a.rs:1: parse()\n{}", "y".repeat(900)),
            ),
            Message::assistant("now the other one", vec![]),
            call("s2", "search_files", r#"{"pattern":"render("}"#),
            result(
                "s2",
                "search_files",
                format!("src/b.rs:2: render()\n{}", "y".repeat(900)),
            ),
            Message::user("summarise"),
        ];
        let total: usize = input.iter().map(message_chars).sum();
        let outcome = fit_detailed(input.clone(), total - 1, 100_000);
        for id in ["s1", "s2"] {
            let message = outcome
                .prepared
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some(id))
                .expect("both search results survive");
            assert!(
                !is_superseded(message),
                "{id} was retired across two different patterns"
            );
        }
    }

    #[test]
    fn the_same_path_and_pattern_is_retired() {
        let input = vec![
            Message::system("sys"),
            Message::user("TASK: find the call sites"),
            call("s1", "search_files", r#"{"path":"src","pattern":"parse("}"#),
            result(
                "s1",
                "search_files",
                format!("src/a.rs:1: parse()\n{}", "y".repeat(900)),
            ),
            Message::assistant("searching again", vec![]),
            call("s2", "search_files", r#"{"path":"src","pattern":"parse("}"#),
            result(
                "s2",
                "search_files",
                format!("src/a.rs:1: parse()\n{}", "y".repeat(900)),
            ),
            Message::user("summarise"),
        ];
        let total: usize = input.iter().map(message_chars).sum();
        let out = fit(input, total - 1, 100_000);
        let old = out
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("s1"))
            .expect("the earlier search is still a message");
        assert!(is_superseded(old), "{}", old.content);
        assert!(
            old.content.contains("search_files `src`"),
            "{}",
            old.content
        );
    }

    #[test]
    fn the_stub_pass_is_idempotent() {
        let input = vec![
            Message::system("sys"),
            Message::user("TASK"),
            call("c1", "read_file", r#"{"path":"src/app.tsx"}"#),
            result("c1", "read_file", page("src/app.tsx", 1, 400, 812, 2_000)),
            Message::assistant("another look", vec![]),
            call("c2", "read_file", r#"{"path":"src/app.tsx"}"#),
            result("c2", "read_file", page("src/app.tsx", 1, 400, 812, 2_000)),
            Message::user("go on"),
        ];
        let once = fit(input.clone(), 2_000, 100_000);
        let twice = fit(once.clone(), 2_000, 100_000);
        assert_eq!(snapshot(&once), snapshot(&twice));

        // The in-memory window behaves the same way.
        let mut history = input;
        compact_run_window(&mut history, 2_000);
        let first = snapshot(&history);
        compact_run_window(&mut history, 2_000);
        assert_eq!(first, snapshot(&history));
    }

    #[test]
    fn a_stub_keeps_the_line_range_it_names_across_a_second_compaction() {
        // Measured on a live run: the first compaction stubbed page 1 with
        // `lines 1-300 of 12000`, and a later compaction - after more pages had
        // been read - re-stubbed that same message from the stub itself (which has
        // no header, so no range) and replaced it with a vaguer sentence. The
        // range is the only thing that makes a stub actionable, and it cannot be
        // recovered once the header is gone.
        let input = vec![
            Message::system("sys"),
            Message::user("TASK: walk src/large.rs"),
            call("c1", "read_file", r#"{"path":"src/large.rs","offset":1}"#),
            result("c1", "read_file", page("src/large.rs", 1, 300, 12_000, 900)),
            Message::assistant("next page", vec![]),
            call("c2", "read_file", r#"{"path":"src/large.rs","offset":301}"#),
            result(
                "c2",
                "read_file",
                page("src/large.rs", 301, 600, 12_000, 900),
            ),
            Message::assistant("next page", vec![]),
            call("c3", "read_file", r#"{"path":"src/large.rs","offset":601}"#),
            result(
                "c3",
                "read_file",
                page("src/large.rs", 601, 900, 12_000, 900),
            ),
            Message::user("summarise"),
        ];
        let once = fit(input, 2_000, 100_000);
        // The run keeps going, which is what makes a *second* compaction happen.
        let mut second = once.clone();
        second.push(Message::assistant("another page", vec![]));
        second.push(call(
            "c4",
            "read_file",
            r#"{"path":"src/large.rs","offset":901}"#,
        ));
        second.push(result(
            "c4",
            "read_file",
            page("src/large.rs", 901, 1_200, 12_000, 900),
        ));
        second.push(Message::user("and now?"));
        let twice = fit(second, 2_000, 100_000);

        for (id, range) in [
            ("c1", "lines 1-300 of 12000"),
            ("c2", "lines 301-600 of 12000"),
            ("c3", "lines 601-900 of 12000"),
        ] {
            let message = twice
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some(id))
                .unwrap_or_else(|| panic!("{id} was dropped, not stubbed"));
            assert!(is_superseded(message), "{id}: {}", message.content);
            assert!(
                message.content.contains(range),
                "{id} lost its range on the second compaction: {}",
                message.content
            );
        }
    }

    #[test]
    fn a_result_without_a_header_falls_back_to_the_call_path() {
        // A failed `read_file` has no `path — lines …` header; the call it answers
        // still names the path, so a later read of the same path retires it.
        let long_error = format!(
            "invalid request: `src/missing.rs` does not exist.\n{}",
            "h".repeat(600)
        );
        let input = vec![
            Message::system("sys"),
            Message::user("TASK"),
            call("c1", "read_file", r#"{"path":"src/missing.rs"}"#),
            result("c1", "read_file", long_error.clone()),
            Message::assistant("trying again", vec![]),
            call("c2", "read_file", r#"{"path":"src/missing.rs"}"#),
            result("c2", "read_file", long_error),
            Message::user("stop"),
            // A tool with no path argument is never a target: two shell runs are
            // two different answers.
            call("r1", "run_command", r#"{"command":"ls"}"#),
            result("r1", "run_command", format!("a\n{}", "b\n".repeat(300))),
            call("r2", "run_command", r#"{"command":"ls"}"#),
            result("r2", "run_command", format!("a\n{}", "b\n".repeat(300))),
        ];
        let out = fit(input.clone(), total_of(&input) - 1, 100_000);
        for id in ["c1", "r1", "r2"] {
            let message = out
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some(id))
                .unwrap();
            assert_eq!(
                is_superseded(message),
                id == "c1",
                "{id}: {}",
                message.content
            );
        }
    }

    #[test]
    fn a_body_shorter_than_its_stub_is_left_alone() {
        // A one-line error must never grow into a longer stub.
        let input = vec![
            Message::system("sys"),
            Message::user("TASK"),
            call("c1", "read_file", r#"{"path":"src/missing.rs"}"#),
            result("c1", "read_file", "not found".into()),
            Message::assistant("trying again", vec![]),
            call("c2", "read_file", r#"{"path":"src/missing.rs"}"#),
            result("c2", "read_file", "not found".into()),
            Message::user("stop"),
        ];
        let outcome = fit_detailed(input.clone(), total_of(&input) - 1, 100_000);
        assert_eq!(
            outcome.superseded_chars.iter().sum::<usize>(),
            0,
            "a short body must not be replaced by a longer stub"
        );
        for message in &outcome.prepared {
            if message.role == Role::Tool {
                assert_eq!(message.content, "not found");
            }
        }
    }

    /// Characters in a window, as the budget counts them.
    fn total_of(messages: &[Message]) -> usize {
        messages.iter().map(message_chars).sum()
    }
}

#[cfg(test)]
mod notebook_tests {
    use super::*;
    use kc_core::message::ToolCall;

    fn exchange(i: usize, filler: usize) -> Vec<Message> {
        vec![
            Message::user(format!("step {i}: inspect the parser")),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"src/{i}.rs\"}}"),
                }],
            ),
            Message::tool_result(
                format!("c{i}"),
                "read_file",
                format!(
                    "src/{i}.rs — lines 1-400 of 400 ({} bytes)\n{}",
                    filler,
                    "x".repeat(filler)
                ),
            ),
            Message::assistant("done", vec![]),
        ]
    }

    fn note_exchange(id: &str, text: &str) -> Vec<Message> {
        vec![
            Message::assistant(
                "",
                vec![ToolCall {
                    id: id.into(),
                    name: NOTE_TOOL.into(),
                    arguments: serde_json::json!({ "text": text }).to_string(),
                }],
            ),
            Message::tool_result(
                id,
                NOTE_TOOL,
                format!("notebook: 1 finding held ({} chars).", text.chars().count()),
            ),
        ]
    }

    fn digests(messages: &[Message]) -> Vec<&Message> {
        messages.iter().filter(|m| is_digest(m)).collect()
    }

    #[test]
    fn notes_survive_a_compaction_that_drops_the_messages_they_came_from() {
        let finding =
            "FINDING: the flake is in rewind.rs - the journal is bound before the row exists";
        let mut history = vec![Message::user("TASK: review the runtime")];
        history.push(Message::assistant("reading", vec![]));
        history.extend(note_exchange("note1", finding));
        for i in 0..40 {
            history.extend(exchange(i, 1_000));
        }
        let notes = vec![finding.to_string()];

        compact_run_window_with_notes(&mut history, 20_000, &notes);

        // The turn that carried the note is gone with the rest of the history...
        assert!(
            !history
                .iter()
                .any(|m| m.tool_call_id.as_deref() == Some("note1")),
            "fixture: the note's own turn should have been dropped"
        );
        // ... but the finding is in the digest, which is a `user` turn.
        let digest = digests(&history);
        assert_eq!(digest.len(), 1, "exactly one digest");
        assert_eq!(digest[0].role, Role::User);
        assert!(
            digest[0].content.contains(finding),
            "the finding did not survive: {}",
            digest[0].content
        );
        assert!(digest[0].content.contains(NOTE_LINE_PREFIX));
        // And the request still fits.
        let total: usize = history.iter().map(message_chars).sum();
        assert!(
            total <= 20_000,
            "the digest pushed the window over: {total}"
        );
    }

    #[test]
    fn the_same_note_is_not_paid_for_twice() {
        let finding = "FINDING: the parser drops a trailing newline";
        let mut history = vec![Message::user("TASK: review")];
        for i in 0..40 {
            history.extend(exchange(i, 1_000));
        }
        let notes = vec![finding.to_string()];
        compact_run_window_with_notes(&mut history, 20_000, &notes);
        // A second compaction carries the digest forward *and* reads the live
        // notebook: the finding must appear once.
        compact_run_window_with_notes(&mut history, 20_000, &notes);
        let digest = digests(&history);
        assert_eq!(digest.len(), 1);
        assert_eq!(
            digest[0].content.matches(finding).count(),
            1,
            "{}",
            digest[0].content
        );
    }

    #[test]
    fn notes_are_bounded_and_the_oldest_are_released_first() {
        // `note_budget` of a 20_000-char window is 1_000 chars, so only the newest
        // few findings can be replayed.
        let mut notes = Vec::new();
        for i in 0..10 {
            notes.push(format!("finding {i}: {}", "y".repeat(180)));
        }
        let mut history = vec![Message::user("TASK: review")];
        for i in 0..40 {
            history.extend(exchange(i, 1_000));
        }
        compact_run_window_with_notes(&mut history, 20_000, &notes);
        let digest = &digests(&history)[0].content;
        let note_lines: Vec<&str> = digest
            .lines()
            .filter(|line| line.starts_with(NOTE_LINE_PREFIX))
            .collect();
        assert!(!note_lines.is_empty(), "no note survived: {digest}");
        let note_chars: usize = note_lines.iter().map(|line| line.chars().count() + 1).sum();
        assert!(
            note_chars <= note_budget(20_000),
            "{note_chars} chars of notes against a {}-char budget",
            note_budget(20_000)
        );
        // The newest finding is kept, the oldest released.
        assert!(digest.contains("finding 9:"), "{digest}");
        assert!(!digest.contains("finding 0:"), "{digest}");
    }

    #[test]
    fn the_notebook_itself_is_bounded_oldest_first() {
        let mut notebook = Notebook::new();
        for i in 0..NOTE_MAX_ENTRIES + 10 {
            assert!(notebook.append(&format!("finding {i}")));
        }
        assert_eq!(notebook.len(), NOTE_MAX_ENTRIES);
        assert_eq!(notebook.entries()[0], "finding 10");
        assert_eq!(
            notebook.entries().last().unwrap(),
            &format!("finding {}", NOTE_MAX_ENTRIES + 9)
        );

        // A long finding is cut, and the total stays inside the policy ceiling.
        let mut notebook = Notebook::new();
        for _ in 0..20 {
            notebook.append(&"z".repeat(NOTE_ENTRY_MAX_CHARS * 2));
        }
        assert!(notebook.chars() <= NOTE_MAX_CHARS, "{}", notebook.chars());
        assert!(!notebook.is_empty());
        assert!(notebook.entries().last().unwrap().chars().count() <= NOTE_ENTRY_MAX_CHARS + 1);
        // Nothing is stored for an empty note.
        assert!(!notebook.append("   "));
    }

    #[test]
    fn the_note_tool_reports_what_is_held() {
        use kc_core::tool::Tool;
        let notebook = Arc::new(Mutex::new(Notebook::new()));
        let tool = NoteTool::new(Arc::clone(&notebook));
        assert_eq!(tool.spec().name, NOTE_TOOL);
        assert_eq!(tool.spec().origin, kc_core::tool::ToolOrigin::Builtin);
        assert!(!tool.requires_approval(), "a note touches nothing on disk");
        assert_eq!(tool.class(), kc_core::tool::ToolClass::ReadOnly);
        // The schema the model is offered requires the one argument it needs.
        let parameters = tool.spec().parameters;
        assert_eq!(parameters["required"][0], "text");
        assert!(tool.spec().description.contains("notebook"));
    }

    #[tokio::test]
    async fn a_note_call_appends_and_answers_with_the_tail() {
        use kc_core::tool::Tool;
        let notebook = Arc::new(Mutex::new(Notebook::new()));
        let tool = NoteTool::new(Arc::clone(&notebook));
        let ctx = tool_ctx();
        let outcome = tool
            .invoke(
                kc_core::tool::ToolCallRequest {
                    call_id: "n1".into(),
                    name: NOTE_TOOL.into(),
                    arguments: serde_json::json!({"text": "the parser drops a trailing newline"}),
                },
                ctx.clone(),
            )
            .await;
        assert!(outcome.ok, "{}", outcome.content);
        assert!(
            outcome.content.contains("1 finding held"),
            "{}",
            outcome.content
        );
        assert!(
            outcome
                .content
                .contains("the parser drops a trailing newline"),
            "{}",
            outcome.content
        );
        assert!(
            outcome.content.contains("never appended to the request"),
            "{}",
            outcome.content
        );
        assert_eq!(notebook.lock().unwrap().len(), 1);

        // An empty note is refused rather than stored.
        let refused = tool
            .invoke(
                kc_core::tool::ToolCallRequest {
                    call_id: "n2".into(),
                    name: NOTE_TOOL.into(),
                    arguments: serde_json::json!({"text": "  "}),
                },
                ctx,
            )
            .await;
        assert!(!refused.ok);
        assert_eq!(notebook.lock().unwrap().len(), 1);
    }

    fn tool_ctx() -> kc_core::tool::ToolCtx {
        kc_core::tool::ToolCtx {
            workspace: Arc::new(
                kc_core::paths::Workspace::new(std::env::temp_dir()).expect("temp workspace"),
            ),
            cancel: Default::default(),
            approvals: kc_core::tool::ApprovalHandle::new(
                Arc::new(AllowGate),
                kc_core::ids::SessionId("s".into()),
                kc_core::ids::RunId("r".into()),
                None,
            ),
            events: Arc::new(|_| {}),
            settings: Arc::new(crate::tests_support::FakeSettings::default()),
            depth: 0,
            supports_images: false,
            checkpoints: None,
        }
    }

    /// A note must never ask for approval, so this gate only exists to make the
    /// handle constructible; it fails loudly if it is ever consulted.
    struct AllowGate;

    #[async_trait::async_trait]
    impl kc_core::tool::ApprovalGate for AllowGate {
        async fn request(
            &self,
            request: kc_core::approval::ApprovalRequest,
        ) -> kc_core::approval::ApprovalDecision {
            panic!("a note asked for approval: {}", request.title);
        }
    }

    #[test]
    fn the_request_never_gains_a_trailing_note() {
        let notes = vec!["a finding worth keeping".to_string()];
        let mut history = vec![Message::user("TASK: review the runtime")];
        for i in 0..30 {
            history.extend(exchange(i, 1_000));
        }
        let last = history.last().unwrap().content.clone();
        compact_run_window_with_notes(&mut history, 20_000, &notes);
        let tail = history.last().unwrap();
        assert_eq!(tail.content, last);
        assert!(!is_digest(tail), "the digest may never be the last message");
        assert!(!tail.content.contains(NOTE_LINE_PREFIX));

        // Same rule for the request builder, which the model actually sees.
        let mut messages = vec![Message::system("sys")];
        messages.extend(history);
        let out = fit_with_notes(messages, 20_000, 24_000, &notes);
        let tail = out.messages.last().unwrap();
        assert!(
            !is_digest(tail) && !tail.content.contains(NOTE_LINE_PREFIX),
            "the request ends on a synthetic note: {}",
            tail.content
        );
        // The digest, where the notebook lives, sits right after the task.
        assert_eq!(out.messages[1].content, "TASK: review the runtime");
        let digest = &out.messages[2];
        assert!(
            is_digest(digest) && digest.content.contains("a finding worth keeping"),
            "the notebook is not in the digest turn: {}",
            digest.content
        );
        assert_eq!(digest.role, Role::User);
    }

    #[test]
    fn the_no_notes_path_is_unchanged() {
        let mut input = vec![Message::user("TASK: review the runtime")];
        for i in 0..30 {
            input.extend(exchange(i, 1_000));
        }
        let mut with_system = vec![Message::system("sys")];
        with_system.extend(input.clone());

        let plain = fit_detailed(with_system.clone(), 20_000, 24_000);
        let with_empty = fit_with_notes(with_system.clone(), 20_000, 24_000, &[]);
        assert_eq!(
            plain.messages.iter().map(message_chars).collect::<Vec<_>>(),
            with_empty
                .messages
                .iter()
                .map(message_chars)
                .collect::<Vec<_>>()
        );
        for (a, b) in plain.messages.iter().zip(&with_empty.messages) {
            assert_eq!(a.content, b.content);
        }
        let digest = plain.notice.expect("a compaction happened");
        assert!(!digest.contains(NOTE_LINE_PREFIX));

        let mut plain_history = with_system.clone();
        compact_run_window(&mut plain_history, 20_000);
        let mut notes_history = with_system;
        compact_run_window_with_notes(&mut notes_history, 20_000, &[]);
        assert_eq!(plain_history.len(), notes_history.len());
        for (a, b) in plain_history.iter().zip(&notes_history) {
            assert_eq!(a.content, b.content);
        }
        assert!(!digests(&notes_history)[0]
            .content
            .contains(NOTE_LINE_PREFIX));
    }

    #[test]
    fn a_fitting_window_carries_no_notebook() {
        // Nothing was dropped, so there is no digest and the notebook must not
        // leak into the request anywhere else.
        let notes = vec!["a finding".to_string()];
        let input = vec![
            Message::system("sys"),
            Message::user("TASK"),
            Message::assistant("ok", vec![]),
            Message::user("and now?"),
        ];
        let out = fit_with_notes(input.clone(), 100_000, 24_000, &notes);
        assert!(out.notice.is_none());
        assert_eq!(out.messages.len(), input.len());
        for (a, b) in out.messages.iter().zip(&input) {
            assert_eq!(a.content, b.content);
        }
        assert!(!out.messages.iter().any(|m| m.content.contains("a finding")));
    }
}

#[cfg(test)]
mod warn_capture {
    //! Capture `tracing::warn!` output for the tests that assert the
    //! cannot-reach-the-ceiling path is *audible* rather than silent.
    //!
    //! `tracing` has no other way to observe a log line (no subscriber is
    //! installed in tests, and adding one as a dev-dependency to observe a
    //! warning is not worth it), and the whole point of that branch is that it
    //! says something instead of quietly sending a request the gateway will
    //! refuse. The subscriber is thread-local, so tests that run in parallel do
    //! not see each other's warnings.

    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    #[derive(Clone, Default)]
    pub(super) struct Warnings(Arc<Mutex<Vec<String>>>);

    impl Warnings {
        fn messages(&self) -> Vec<String> {
            self.0.lock().expect("warnings lock").clone()
        }

        fn push(&self, text: String) {
            self.0.lock().expect("warnings lock").push(text);
        }
    }

    impl Subscriber for Warnings {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            *metadata.level() <= tracing::Level::WARN
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut text = String::new();
            event.record(&mut Message(&mut text));
            self.push(text);
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    /// The `message` field of an event, rendered.
    struct Message<'a>(&'a mut String);

    impl Visit for Message<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0.push_str(&format!("{value:?}"));
            }
        }
    }

    /// Run `f` with a subscriber that records warnings, and return them.
    pub(super) fn capture(f: impl FnOnce()) -> Vec<String> {
        let warnings = Warnings::default();
        tracing::subscriber::with_default(warnings.clone(), || {
            // Callsites cache their interest on first use; without this the
            // thread-local subscriber installed above would never be consulted
            // for an event that was already seen as "no subscriber".
            tracing::callsite::rebuild_interest_cache();
            f();
        });
        warnings.messages()
    }
}

#[cfg(test)]
mod item_ceiling_tests {
    //! The item ceiling (`limits.max_context_messages`): a window can be far
    //! under the character budget and still be refused by the gateway.
    //!
    //! Measured on one install: 7 sessions carried more than 1,000 messages
    //! while staying under the character budget (worst: 2,055 items / 1.96 M
    //! chars), and every one of them was un-resumable. These tests build exactly
    //! that shape - many tiny exchanges - so the character budget can never be
    //! what compacts.

    use super::warn_capture::capture;
    use super::*;
    use kc_core::message::ToolCall;

    /// A generic character budget: far above the fixture, so nothing is ever
    /// dropped for being too big. The ceiling is the only reason to compact.
    const HUGE: usize = 10_000_000;

    /// One exchange: a turn that asks for one tool call and the result that
    /// answers it. Two messages, both tiny.
    fn exchange(i: usize) -> Vec<Message> {
        vec![
            Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: format!("{{\"path\":\"src/{i}.rs\"}}"),
                }],
            ),
            Message::tool_result(format!("c{i}"), "read_file", format!("{i}: ok")),
        ]
    }

    /// `exchanges` tiny exchanges behind the pinned task.
    fn window(exchanges: usize) -> Vec<Message> {
        let mut messages = vec![
            Message::system("SYSTEM"),
            Message::user("TASK: audit the runtime"),
        ];
        for i in 0..exchanges {
            messages.extend(exchange(i));
        }
        messages
    }

    fn serialized(messages: &[Message]) -> String {
        serde_json::to_string(messages).expect("messages serialize")
    }

    /// Every `tool` message answers a call that is still in the window, and the
    /// window still starts with the system prompt.
    fn assert_pairs_are_valid(messages: &[Message]) {
        assert_eq!(messages[0].role, Role::System);
        let mut declared: HashSet<String> = HashSet::new();
        for message in messages {
            for call in &message.tool_calls {
                declared.insert(call.id.clone());
            }
            if message.role == Role::Tool {
                let id = message
                    .tool_call_id
                    .clone()
                    .expect("a tool message names its call");
                assert!(declared.contains(&id), "orphaned tool result {id}");
            }
        }
    }

    /// The bug, exactly: over the item ceiling, under the character budget.
    #[test]
    fn a_window_over_the_item_ceiling_compacts_even_though_it_fits_the_characters() {
        let messages = window(200);
        assert_eq!(messages.len(), 402);
        let chars: usize = messages.iter().map(message_chars).sum();
        assert!(chars < 40_000, "the fixture must fit the character budget");

        let out = fit_detailed_with_limits(messages.clone(), ContextLimits::new(HUGE, 100), 24_000);

        assert!(
            out.messages.len() <= 100,
            "the ceiling was not met: {} items",
            out.messages.len()
        );
        assert!(
            out.notice.is_some(),
            "a ceiling-triggered compaction must leave a digest behind"
        );
        assert!(
            out.chars() < HUGE,
            "the character budget was never the issue"
        );
        assert_pairs_are_valid(&out.messages);
        // The task is the pinned prefix and must survive.
        assert!(out
            .messages
            .iter()
            .any(|m| m.content == "TASK: audit the runtime"));
    }

    /// The prompt-cache invariant: a window under *both* ceilings is not
    /// rewritten at all, byte for byte.
    #[test]
    fn a_window_under_both_ceilings_is_byte_identical() {
        let messages = window(200);
        let out =
            fit_detailed_with_limits(messages.clone(), ContextLimits::new(HUGE, 1_000), 24_000);

        assert!(out.notice.is_none(), "nothing may be dropped or digested");
        assert!(!out.dropped.iter().any(|dropped| *dropped));
        assert_eq!(serialized(&out.messages), serialized(&messages));
    }

    /// The ceiling is met by removing whole exchanges, so the counts line up and
    /// no `tool_call` is left without its result.
    #[test]
    fn the_ceiling_is_met_by_dropping_whole_exchanges() {
        let messages = window(200);
        let out = fit_detailed_with_limits(messages.clone(), ContextLimits::new(HUGE, 60), 24_000);

        assert!(out.messages.len() <= 60, "{}", out.messages.len());
        assert_pairs_are_valid(&out.messages);
        // Whole exchanges: every dropped unit was an assistant turn plus its
        // result, so the number of dropped messages is even and the dropped
        // results belong to the dropped calls.
        let dropped_exchanges = out.dropped.iter().filter(|dropped| **dropped).count();
        assert_eq!(
            dropped_exchanges % 2,
            0,
            "{dropped_exchanges} messages dropped"
        );
        for (i, message) in out.prepared.iter().enumerate() {
            if out.dropped[i] && message.role == Role::Tool {
                let id = message.tool_call_id.clone().expect("tool result id");
                let owner_dropped = out
                    .prepared
                    .iter()
                    .enumerate()
                    .any(|(j, m)| out.dropped[j] && m.tool_calls.iter().any(|c| c.id == id));
                assert!(owner_dropped, "the result {id} outlived its call");
            }
        }
    }

    /// Compaction buys headroom in items as well as in characters, so the ceiling
    /// does not re-trigger on the very next turn - which would be one cache miss
    /// per turn instead of one per compaction.
    #[test]
    fn a_ceiling_compaction_leaves_headroom_for_the_next_turns() {
        let messages = window(499); // 1,000 items, the shape of the real failure
        assert_eq!(messages.len(), 1_000);
        let out = fit_detailed_with_limits(messages, ContextLimits::new(HUGE, 900), 24_000);
        assert!(out.messages.len() <= 900, "{}", out.messages.len());
        // 50 turns of the exchange's own two messages must still fit.
        assert!(
            out.messages.len() + TURNS_OF_HEADROOM * 2 <= 900,
            "no headroom: {} items",
            out.messages.len()
        );
    }

    /// The prefixes are protected: the task and the exchange in flight survive a
    /// ceiling-triggered compaction no matter how tight the ceiling is.
    #[test]
    fn the_task_and_the_exchange_in_flight_survive_the_ceiling() {
        let mut messages = vec![
            Message::system("SYSTEM"),
            Message::user("TASK: audit the runtime"),
        ];
        for i in 0..100 {
            messages.extend(exchange(i));
        }
        // The turn in flight: an assistant turn with two tool calls and both
        // results, all at the very end.
        messages.push(Message::assistant(
            "",
            vec![
                ToolCall {
                    id: "live_a".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a.rs\"}".into(),
                },
                ToolCall {
                    id: "live_b".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"b.rs\"}".into(),
                },
            ],
        ));
        messages.push(Message::tool_result("live_a", "read_file", "a: ok"));
        messages.push(Message::tool_result("live_b", "read_file", "b: ok"));

        let out = fit_detailed_with_limits(messages, ContextLimits::new(HUGE, 8), 24_000);

        assert!(out.messages[0].content == "SYSTEM");
        assert!(
            out.messages
                .iter()
                .any(|m| m.content == "TASK: audit the runtime"),
            "the task was dropped to satisfy the ceiling"
        );
        for id in ["live_a", "live_b"] {
            assert!(
                out.messages
                    .iter()
                    .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(id)),
                "the in-flight result {id} was dropped"
            );
        }
        assert_pairs_are_valid(&out.messages);
    }

    /// A ceiling that genuinely cannot be met (the pinned prefix plus the
    /// exchange in flight are already over it) is warned about, and the request
    /// is sent **valid** rather than split.
    #[test]
    fn a_ceiling_that_cannot_be_met_warns_and_sends_a_valid_request() {
        // Only the pinned prefix and the exchange in flight: 4 messages, tiny,
        // with a ceiling of 2 that would require dropping one of them.
        let messages = vec![
            Message::system("SYSTEM"),
            Message::user("TASK: audit the runtime"),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: "live".into(),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"a.rs\"}".into(),
                }],
            ),
            Message::tool_result("live", "read_file", "a: ok"),
        ];
        let limits = ContextLimits::new(HUGE, 2);

        let mut out = None;
        let warnings = capture(|| {
            out = Some(fit_detailed_with_limits(messages.clone(), limits, 24_000));
        });
        let out = out.expect("fit ran");

        // Valid, not split: every message that was there is still there, whole.
        assert_eq!(serialized(&out.messages), serialized(&messages));
        assert_pairs_are_valid(&out.messages);
        assert!(out.notice.is_none(), "no digest for a window nothing left");

        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("limits.max_context_messages")),
            "a ceiling that cannot be met must be warned about: {warnings:?}"
        );
    }

    /// The same ceiling, on the window the loop *holds*: `compact_run_window`
    /// must bound the item count too, or a long session keeps (and re-clones
    /// into every request) thousands of messages.
    #[test]
    fn the_held_run_window_is_bounded_by_the_item_ceiling() {
        let mut history = window(300);
        assert_eq!(history.len(), 602);

        compact_run_window_with_limits(&mut history, ContextLimits::new(HUGE, 80), &[]);

        assert!(
            history.len() <= 80,
            "held window is {} items",
            history.len()
        );
        assert_pairs_are_valid(&history);
        assert!(history[0].content == "SYSTEM");
        assert!(history
            .iter()
            .any(|m| m.content == "TASK: audit the runtime"));
    }

    /// The held window's cache invariant: under both ceilings it does not change
    /// by one byte.
    #[test]
    fn a_held_window_under_both_ceilings_is_untouched() {
        let mut history = window(20);
        let before = history.clone();
        compact_run_window_with_limits(&mut history, ContextLimits::new(HUGE, 900), &[]);
        assert_eq!(serialized(&history), serialized(&before));
    }

    /// The supersession pass rewrites tool bodies, so it stays tied to the
    /// *character* budget: stubbing cannot move an item count, and rewriting a
    /// body that fits costs a cache miss for no saving at all.
    #[test]
    fn a_ceiling_only_compaction_does_not_rewrite_tool_bodies() {
        let mut messages = vec![Message::system("SYSTEM"), Message::user("TASK: audit")];
        for i in 0..60 {
            messages.push(Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("r{i}"),
                    name: "read_file".into(),
                    arguments: "{\"path\":\"src/app.rs\"}".into(),
                }],
            ));
            messages.push(Message::tool_result(
                format!("r{i}"),
                "read_file",
                format!("src/app.rs \u{2014} lines 1-40 of 40 (100 bytes)\n{i}: the same page"),
            ));
        }
        let out = fit_detailed_with_limits(messages, ContextLimits::new(HUGE, 60), 24_000);

        assert!(out.messages.len() <= 60, "{}", out.messages.len());
        assert!(
            out.superseded_chars.iter().all(|saved| *saved == 0),
            "a ceiling-only compaction stubbed bodies: {:?}",
            out.superseded_chars
        );
        assert!(!out
            .messages
            .iter()
            .any(|m| m.content.starts_with(SUPERSEDED_PREFIX)));
    }

    /// `0` disables the ceiling: that is documented as "only for a provider with
    /// no item limit", and it must really do nothing.
    #[test]
    fn zero_disables_the_item_ceiling() {
        let messages = window(200);
        let out = fit_detailed_with_limits(messages.clone(), ContextLimits::new(HUGE, 0), 24_000);
        assert!(out.notice.is_none());
        assert_eq!(serialized(&out.messages), serialized(&messages));
    }

    /// The forced retry's target: 60% of both ceilings, floored so what is left
    /// is still a request, and never above what the operator configured.
    #[test]
    fn the_forced_target_is_sixty_percent_and_never_larger() {
        let limits = ContextLimits::new(1_000_000, 900);
        let forced = limits.forced_smaller();
        assert_eq!(forced.max_chars, 600_000);
        assert_eq!(forced.max_messages, 540);

        // A provider with no item limit stays without one.
        let forced = ContextLimits::chars_only(1_000_000).forced_smaller();
        assert_eq!(forced.max_chars, 600_000);
        assert_eq!(forced.max_messages, 0);

        // A deliberately tiny ceiling is not raised by the floor.
        let forced = ContextLimits::new(1_000, 9).forced_smaller();
        assert_eq!(forced.max_chars, 1_000);
        assert_eq!(forced.max_messages, 8);

        // ... and the character floor never exceeds a tiny explicit budget.
        let forced = ContextLimits::new(600, 900).forced_smaller();
        assert_eq!(forced.max_chars, 600);
    }
}

// ---------------------------------------------------------------------------
// request-bound proofs
// ---------------------------------------------------------------------------
//
// Every test here answers one question about a *request*: how large can one
// tolerated insertion point make it, and which key bounds it. They exist because
// "the budget is enforced by `fit`" is a claim, and a claim about bytes is only
// worth what a serialized request says it is worth.

#[cfg(test)]
mod request_bound_tests {
    use super::*;
    use kc_core::config::Limits;
    use kc_core::message::{ImageAttachment, ToolCall};

    fn image(size: usize) -> ImageAttachment {
        ImageAttachment {
            data: "a".repeat(size),
            media_type: "image/png".into(),
            ..Default::default()
        }
    }

    fn total(messages: &[Message]) -> usize {
        messages.iter().map(message_chars).sum()
    }

    fn tool(messages: &[Message]) -> &Message {
        messages
            .iter()
            .find(|m| m.role == Role::Tool)
            .expect("a tool result")
    }

    /// A reasoning model's chain of thought is persisted with its turn and
    /// **re-sent** on every later request (`kc_provider::request` writes
    /// `reasoning_content` back into the body). `message_chars` charges it
    /// against `limits.context_max_chars`, so the elision pass has to be able to
    /// reclaim it - otherwise a window whose characters are mostly reasoning is
    /// sent over budget, which is the one outcome the budget exists to prevent.
    #[test]
    fn a_reasoning_body_is_elided_rather_than_sent_over_budget() {
        let messages = vec![
            Message::system("SYSTEM"),
            Message::user("TASK: audit the runtime"),
            Message::assistant_with_reasoning(
                "weighing the options",
                Some("r".repeat(200_000)),
                vec![],
            ),
        ];
        let out = fit_with_limits(messages, ContextLimits::new(20_000, 900), 24_000);

        assert!(
            total(&out) <= 20_000,
            "a 200,000-character reasoning body was sent against a 20,000 budget: {} chars",
            total(&out)
        );
        let assistant = out
            .iter()
            .find(|m| m.role == Role::Assistant)
            .expect("the turn stays");
        let reasoning = assistant.reasoning_content.as_deref().unwrap_or_default();
        assert!(
            reasoning.len() < 200_000,
            "reasoning was not shortened at all: {} bytes",
            reasoning.len()
        );
        assert!(
            reasoning.contains("elided"),
            "the cut reasoning says so: {reasoning:?}"
        );
    }

    /// An image is charged [`IMAGE_CONTEXT_CHARS`] against the character budget,
    /// so the number of images a window may keep is a budget question. The byte
    /// cap alone (`IMAGE_BYTES_BUDGET`, 20 MiB) let a single turn of small
    /// screenshots hold a request far over its own character budget, and no
    /// elision can shorten an image.
    #[test]
    fn images_cannot_hold_a_window_over_its_own_budget() {
        let mut task = Message::user("TASK: tidy up the dashboard");
        task.images = (0..64).map(|_| image(1_000)).collect();
        let messages = vec![Message::system("SYSTEM"), task];

        let out = fit_with_limits(messages, ContextLimits::new(40_000, 900), 24_000);

        assert!(
            total(&out) <= 40_000,
            "64 images cost {} charged chars against a 40,000 budget",
            total(&out)
        );
        let kept = out[1].images.len();
        assert!(kept > 0, "the newest images still reach the model");
        assert!(
            out[1].content.contains("image(s) omitted"),
            "the omission is stated, not silent: {:?}",
            out[1].content
        );
        // The byte bound is unchanged and independent of the character one.
        let bytes: usize = out
            .iter()
            .flat_map(|m| &m.images)
            .map(|i| i.data.len())
            .sum();
        assert!(bytes <= IMAGE_BYTES_BUDGET);
    }

    /// The same rule on the window the loop *holds*: what is held is re-cloned
    /// into every request, so an over-budget image set must not survive there
    /// either.
    #[test]
    fn the_held_window_drops_images_it_cannot_afford() {
        let mut task = Message::user("TASK: tidy up");
        task.images = (0..64).map(|_| image(1_000)).collect();
        let mut history = vec![task];

        compact_run_window_with_limits(&mut history, ContextLimits::new(40_000, 900), &[]);

        assert!(
            total(&history) <= 40_000,
            "held window is {} chars against a 40,000 budget",
            total(&history)
        );
        assert!(!history[0].images.is_empty());
    }

    /// A tool result trimmed by the tool's own cap and then again by the context
    /// cap must still end with the tail it ended with. Two markers, one marker
    /// per cut, and the bytes at the end are the bytes the tool produced.
    #[test]
    fn a_result_cut_twice_keeps_its_tail() {
        let raw = format!("HEAD-{}-TAIL-SENTINEL", "m".repeat(200_000));
        // What the tool handed the transcript: already cut at the 24,000-byte
        // default. A session resumed under a smaller `max_tool_output_bytes`
        // (or one persisted when the cap was larger) is cut a second time here.
        let (stored, cut) = kc_core::tool::truncate_output(&raw, 24_000);
        assert!(cut);
        assert!(stored.ends_with("TAIL-SENTINEL"));

        let out = fit_with_limits(
            vec![
                Message::system("S"),
                Message::user("T"),
                Message::tool_result("c1", "read_file", stored.clone()),
            ],
            ContextLimits::chars_only(1_000_000),
            8_000,
        );
        let body = &tool(&out).content;
        assert!(
            body.len() <= 8_000,
            "the second cut is inside the cap: {}",
            body.len()
        );
        assert!(
            body.ends_with("TAIL-SENTINEL"),
            "the second cut must keep the tail the first one kept"
        );
        // The first cut's marker sits at the seam, which is exactly what the
        // second cut removes, so the model reads one marker - not a stack of
        // them - and the numbers in it describe the splice it is looking at.
        assert_eq!(
            body.matches("[truncated:").count(),
            stored.matches("[truncated:").count(),
            "a second cut must not add a second marker: {body:?}"
        );
    }

    /// The tools catalogue is written beside the messages (`kc_provider::request`
    /// serializes `tools` next to `messages`) and is bounded by nothing that
    /// context.rs can see: a plugin or an MCP server declares its own names,
    /// descriptions and schemas in another process. So the budget has to reserve
    /// it, or "the request fits the model's window" is a claim about the messages
    /// alone.
    ///
    /// This is the whole request, serialized by the provider, against the budget:
    /// what the model is billed for is the body, not our accounting of it.
    #[test]
    fn the_serialized_request_includes_the_tools_catalogue_in_its_budget() {
        let tools = catalogue(48, 300);
        let payload = tool_payload_chars(&tools);
        assert!(
            payload > 25_000,
            "fixture: a realistic plugin catalogue is tens of KB, got {payload}"
        );
        let budget = 120_000;
        let limits = ContextLimits::new(budget, 900).reserving(payload);

        let messages = pathological_turn();
        let out = fit_with_limits(messages, limits, 24_000);

        assert!(
            out.iter().map(message_chars).sum::<usize>() + payload <= budget,
            "messages {} + catalogue {payload} against a {budget} budget",
            out.iter().map(message_chars).sum::<usize>()
        );
        let bytes = body_bytes(&out, &tools);
        assert!(
            bytes <= budget + envelope_slack(&out),
            "the serialized request is {bytes} bytes against a {budget}-character budget \
             (scaffolding allowance {})",
            envelope_slack(&out)
        );
    }

    /// Nothing can make `reserving` hand the conversation a budget of nothing:
    /// a catalogue bigger than the window leaves half of it for the task and the
    /// turn in flight instead of zero.
    #[test]
    fn a_catalogue_cannot_take_the_whole_budget() {
        assert_eq!(
            ContextLimits::new(100_000, 900).reserving(0).max_chars,
            100_000
        );
        assert_eq!(
            ContextLimits::new(100_000, 900).reserving(10_000).max_chars,
            90_000
        );
        assert_eq!(
            ContextLimits::new(100_000, 900)
                .reserving(400_000)
                .max_chars,
            50_000
        );
    }

    /// The pathological turn, end to end: a plugin fan-out whose results are
    /// megabytes, a subagent report pasted into the parent as a tool result, the
    /// notebook at its own ceiling, and a user task the size of a log file.
    /// Everything the user can make large at once, then serialized.
    #[test]
    fn a_pathological_window_serializes_inside_its_budget() {
        let budget = 240_000;
        let limits = ContextLimits::new(budget, 900);
        let notes: Vec<String> = (0..NOTE_MAX_ENTRIES)
            .map(|i| format!("{i}: {}", "n".repeat(NOTE_ENTRY_MAX_CHARS)))
            .collect();

        let input = pathological_turn();
        let before: usize = input.iter().map(message_chars).sum();
        assert!(
            before > 10 * budget,
            "fixture: {before} chars is not the pathological case for a {budget} budget"
        );
        let task = input[1].content.clone();

        let out = fit_with_notes_and_limits(
            input,
            limits,
            Limits::default().max_tool_output_bytes,
            &notes,
        )
        .messages;

        // The two things a compaction may never lose: the harness's own prompt,
        // byte for byte, and the instruction the run was given.
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[0].content, "SYSTEM PROMPT");
        let head: String = task.chars().take(200).collect();
        assert!(
            out.iter()
                .any(|m| m.role == Role::User && m.content.starts_with(&head)),
            "the task survived, at least as a head"
        );
        assert_eq!(out.last().map(|m| m.role), Some(Role::Tool));

        for message in out.iter().filter(|m| m.role == Role::Tool) {
            assert!(
                message.content.len() <= Limits::default().max_tool_output_bytes,
                "a tool result escaped the per-result cap: {} bytes",
                message.content.len()
            );
        }
        let chars: usize = out.iter().map(message_chars).sum();
        assert!(chars <= budget, "{chars} chars against a {budget} budget");
        // The notebook is replayed inside the digest, not appended, and is
        // charged like everything else: 64 findings at 2,000 characters cannot
        // add 128,000 characters to the request.
        let bytes = body_bytes(&out, &[]);
        assert!(
            bytes <= budget + envelope_slack(&out),
            "the serialized request is {bytes} bytes against a {budget}-character budget"
        );
        // ... and every tool call is still answered, so the request is valid.
        let calls: Vec<&str> = out
            .iter()
            .flat_map(|m| m.tool_calls.iter().map(|c| c.id.as_str()))
            .collect();
        let results: Vec<&str> = out
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.tool_call_id.as_deref().unwrap_or_default())
            .collect();
        for id in results {
            assert!(calls.contains(&id), "orphaned tool result {id}");
        }
    }

    /// The item ceiling counts **every** wire item: the system prompt the loop
    /// prepends, every tool result, the turns, and (after a compaction) the digest
    /// that replaces the dropped ones.
    ///
    /// The proof is the pair of assertions: a window of exactly `n` items is left
    /// alone, and the same window at `n - 1` compacts. Drop the system prompt or a
    /// tool result from the count and the second case would fit, so this is what
    /// pins `limits.max_context_messages` to what the request really carries.
    #[test]
    fn the_item_ceiling_counts_the_system_prompt_and_every_tool_result() {
        let mut messages = vec![Message::system("SYSTEM"), Message::user("TASK: audit")];
        for i in 0..10 {
            messages.push(Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                }],
            ));
            messages.push(Message::tool_result(
                format!("c{i}"),
                "read_file",
                format!("{i}: ok"),
            ));
            messages.push(Message::user(format!("next {i}")));
            messages.push(Message::assistant(format!("done {i}"), vec![]));
        }
        let n = provider_message_count(&messages);
        assert_eq!(n, 42, "2 + 10 exchanges of 4 items");
        const HUGE: usize = 10_000_000;

        let at_ceiling = fit_with_limits(messages.clone(), ContextLimits::new(HUGE, n), 24_000);
        assert_eq!(
            at_ceiling.len(),
            messages.len(),
            "a window at exactly the ceiling must be sent byte-identical"
        );

        let over = fit_detailed_with_limits(messages, ContextLimits::new(HUGE, n - 1), 24_000);
        assert!(
            over.notice.is_some(),
            "one item over the ceiling must compact, not fail the turn"
        );
        assert!(
            provider_message_count(&over.messages) < n,
            "compacted to {} items against a {} ceiling",
            provider_message_count(&over.messages),
            n - 1
        );
    }

    /// A catalogue of `count` tools whose descriptions and schemas are as large as
    /// a plugin process cares to make them.
    fn catalogue(count: usize, description: usize) -> Vec<kc_core::tool::ToolSpec> {
        (0..count)
            .map(|i| kc_core::tool::ToolSpec {
                name: format!("plugin__server_{i}__do_the_thing"),
                description: "d".repeat(description),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "p".repeat(description)},
                        "mode": {"type": "string", "enum": ["a", "b", "c"]}
                    },
                    "required": ["path"]
                }),
                origin: kc_core::tool::ToolOrigin::Plugin {
                    name: format!("server_{i}"),
                },
            })
            .collect()
    }

    /// The worst window a user can build in one run: a task that is a pasted log,
    /// a fan-out of 32 huge plugin results plus an MCP result and a one-megabyte
    /// subagent report, and old exchanges behind it.
    fn pathological_turn() -> Vec<Message> {
        let mut messages = vec![
            Message::system("SYSTEM PROMPT"),
            Message::user(format!("TASK: read this log {}", "L".repeat(200_000))),
        ];
        for exchange in 0..8 {
            messages.push(Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("old-{exchange}"),
                    name: "read_file".into(),
                    arguments: format!(r#"{{"path":"src/{exchange}.rs"}}"#),
                }],
            ));
            messages.push(Message::tool_result(
                format!("old-{exchange}"),
                "read_file",
                format!("old page {exchange}: {}", "o".repeat(300_000)),
            ));
            messages.push(Message::user(format!("step {exchange}")));
        }
        let mut calls = Vec::new();
        for call in 0..32 {
            calls.push(ToolCall {
                id: format!("fan-{call}"),
                name: "plugin__atlas__atlas_scan".into(),
                arguments: "{}".into(),
            });
        }
        calls.push(ToolCall {
            id: "mcp".into(),
            name: "mcp__srv__query".into(),
            arguments: "{}".into(),
        });
        calls.push(ToolCall {
            id: "subagent".into(),
            name: "spawn_agent".into(),
            arguments: "{}".into(),
        });
        messages.push(Message::assistant("", calls));
        for call in 0..32 {
            messages.push(Message::tool_result(
                format!("fan-{call}"),
                "plugin__atlas__atlas_scan",
                format!("plugin result {call}: {}", "p".repeat(400_000)),
            ));
        }
        messages.push(Message::tool_result(
            "mcp",
            "mcp__srv__query",
            format!("mcp result: {}", "m".repeat(500_000)),
        ));
        messages.push(Message::tool_result(
            "subagent",
            "spawn_agent",
            format!("the child's report: {}", "s".repeat(1_000_000)),
        ));
        messages
    }

    /// Characters of JSON scaffolding the provider writes around the content and
    /// that [`message_chars`] does not charge for: `role`, `content`,
    /// `tool_call_id` and `name` keys per message, the `id`/`type`/`function`
    /// wrapper per call, and the body's own fixed keys.
    fn envelope_slack(messages: &[Message]) -> usize {
        let calls: usize = messages.iter().map(|m| m.tool_calls.len()).sum();
        messages.len() * 96 + calls * 128 + 1_024
    }

    /// The whole request, serialized exactly as `kc_provider` sends it.
    fn body_bytes(messages: &[Message], tools: &[kc_core::tool::ToolSpec]) -> usize {
        let request = kc_core::provider::ChatRequest {
            admission: None,
            model: "test-model".into(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            temperature: 0.2,
            send_temperature: true,
            max_tokens: None,
            top_p: None,
            parallel_tool_calls: true,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
        };
        serde_json::to_string(&kc_provider::request::build_chat_body(&request))
            .expect("serializes")
            .len()
    }
}
