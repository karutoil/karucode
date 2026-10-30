//! Context budgeting: truncate tool output and compact the oldest exchanges
//! while keeping the system prompt, the original task and the newest turn.
//!
//! Three steps, applied to the message list actually sent to the model (the
//! persisted transcript is never rewritten):
//!
//! 1. every `tool` message content is capped at `max_tool_output_bytes`;
//! 2. while the total character budget is exceeded, the oldest droppable
//!    exchange is removed. An assistant message and the tool results that answer
//!    its `tool_calls` are dropped as one group, so the model never sees a tool
//!    result whose call is missing;
//! 3. dropped turns are replaced by one digest notice: a summary of the
//!    user/assistant turns and the tools they called, bounded by
//!    [`DIGEST_MAX_CHARS`] and counted inside the same budget.
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

// @kc-doc: prompt-cache
// @kc-title: Keep the prompt cache hitting (cheap long runs)
// @kc-summary: Why a long run can re-bill its whole context every turn, what karucode does to keep the cached prefix stable, and how to read the hit rate.
// @kc-tags: cache, caching, cost, context, compaction, tokens, performance, provider
// @kc-applies-to: config:limits.context_max_chars, config:limits.max_tool_output_bytes, config:limits.max_tool_calls_per_turn, env:KARUCODE_CONTEXT_MAX_CHARS, env:KARUCODE_MAX_TOOL_OUTPUT_BYTES, route:GET /api/events, ui:ChatView
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
// * Compaction is **quantised**: it runs only when the window passes
//   `limits.context_max_chars`, and then cuts the oldest exchanges until the
//   remaining window leaves room for about 50 more turns (never below 15% of the
//   budget). One cache miss per compaction instead of one per turn.
// * The original user turn and the exchange in flight are never compacted, so a
//   long run keeps its goal and the results it is currently reasoning about.
// * **Written findings are sticky.** An assistant turn with at least 200
//   characters of text and no tool calls is kept verbatim across compaction
//   while it fits the note budget (a twentieth of the context window).
// * **Nothing is appended after the conversation.** No runtime note, no nudge,
//   no reminder. Measured on a real 140-message session against a live gateway,
//   at the point where the model was about to write a file: the history alone
//   produced `write_file(8308B)`; adding a note at the end (as `system`, as
//   `user`, or folded into the last tool result) made the model burn its whole
//   output budget on reasoning and never act. A model that reasons well but
//   needs the request to end on a turn is not an unusual model - so the request
//   is exactly the conversation, and anything the runtime wants to say to the
//   model has to fit inside the tool results it already produces.
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
// # One tool result larger than this is truncated per call.
// max_tool_output_bytes = 24000
// ```
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
// * A tool result that repeats an identical answer gets a note after the third
//   time ("this exact call has now returned an identical result 3 times"). The
//   fourth identical read-only call, with no write in between, is **refused**
//   with a `[blocked: ...]` result instead of being executed: re-reading an
//   unchanged page cannot produce new information, and one review run was
//   measured re-reading the same file 27 times. A read after an edit is always
//   allowed - the mutation counter resets the check.
// * Two tool calls with the same arguments in the same turn are not de-duplicated;
//   only the note above is applied.
// * `prompt_cache_key` (OpenAI) is not sent. Prefix caching works without it; the
//   key only improves routing across cache shards on very large providers.

const TRIM_PREFIX: &str = "[context digest: ";

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

/// Characters charged for one attached image.
///
/// The base64 payload is *not* what the provider bills: an image costs roughly
/// `width * height / 750` tokens, capped a little above 1k for a large one. Four
/// characters per token puts a full-size image near 8k characters. Counting the
/// base64 instead would overcharge by two orders of magnitude and evict every
/// image on the next turn; counting zero would let a session of screenshots
/// silently overrun the window. This is the middle: a flat, documented estimate.
pub const IMAGE_CONTEXT_CHARS: usize = 8_000;

/// Number of characters a message contributes to the budget.
///
/// Public because the context report must count exactly what the budget counts;
/// a second formula would disagree with the thing it is describing.
pub fn message_chars(message: &Message) -> usize {
    let calls: usize = message
        .tool_calls
        .iter()
        .map(|c| c.name.chars().count() + c.arguments.chars().count())
        .sum::<usize>();
    message.content.chars().count() + calls + message.images.len() * IMAGE_CONTEXT_CHARS
}

/// The result of fitting a conversation into a character budget.
///
/// [`fit`] only needs [`FitOutcome::messages`]. The rest exists so the context
/// report (`crate::context_report`) can explain *why* a request looks the way it
/// does without re-implementing the compaction rules - a second implementation
/// would drift from this one and then confidently report the wrong thing.
#[derive(Debug, Clone)]
pub struct FitOutcome {
    /// The list that will be sent: truncated, compacted, digest inserted.
    pub messages: Vec<Message>,
    /// The input after per-message truncation, before any compaction. Same
    /// length and order as the input, so `dropped[i]` describes `prepared[i]`.
    pub prepared: Vec<Message>,
    /// `prepared[i]` was removed by compaction.
    pub dropped: Vec<bool>,
    /// The digest that replaced the dropped turns, when a compaction happened.
    pub notice: Option<String>,
}

impl FitOutcome {
    fn unchanged(prepared: Vec<Message>) -> Self {
        let dropped = vec![false; prepared.len()];
        Self {
            messages: prepared.clone(),
            prepared,
            dropped,
            notice: None,
        }
    }

    /// Characters in the list that will be sent.
    pub fn chars(&self) -> usize {
        self.messages.iter().map(message_chars).sum()
    }

    /// Characters the compaction removed from the window.
    pub fn compacted_chars(&self) -> usize {
        self.prepared
            .iter()
            .zip(&self.dropped)
            .filter(|(_, dropped)| **dropped)
            .map(|(message, _)| message_chars(message))
            .sum()
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
pub fn fit(messages: Vec<Message>, max_chars: usize, max_tool_output_bytes: usize) -> Vec<Message> {
    fit_detailed(messages, max_chars, max_tool_output_bytes).messages
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
    let prepared: Vec<Message> = messages
        .into_iter()
        .map(|mut m| {
            if m.role == Role::Tool {
                let (content, _) = truncate_output(&m.content, max_tool_output_bytes);
                m.content = content;
            }
            m
        })
        .collect();

    let total: usize = prepared.iter().map(message_chars).sum();
    if total <= max_chars {
        return FitOutcome::unchanged(prepared);
    }

    // The last user turn is the request being answered; everything from it
    // onwards is untouchable. The first user message (the task) is pinned too.
    let end = prepared
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(0);
    let start = pinned_prefix(&prepared);
    let sticky = sticky_flags(&prepared, max_chars);
    let mut units = group_droppable(&prepared, start, end.max(start), &sticky);
    if units.is_empty() {
        // No user boundary to drop behind (a subagent talks to one task the
        // whole run). Fall back to keeping the newest tail so an over-budget
        // window is never sent as-is.
        let end = clamp_end_to_exchange(
            &prepared,
            start,
            prepared.len().saturating_sub(PROTECTED_TAIL).max(start),
        );
        units = group_droppable(&prepared, start, end, &sticky);
        if units.is_empty() {
            return FitOutcome::unchanged(prepared);
        }
    }

    let target = target_chars(&prepared, max_chars);
    let mut survivors = total;
    let mut dropped_messages = 0usize;
    let mut dropped = vec![false; prepared.len()];
    let mut next = 0usize;

    let notice = loop {
        while survivors > target && next < units.len() {
            let unit = &units[next];
            survivors = survivors.saturating_sub(unit_chars(&prepared, unit));
            dropped_messages += unit.len();
            for &i in unit {
                dropped[i] = true;
            }
            next += 1;
        }
        if dropped_messages == 0 {
            return FitOutcome::unchanged(prepared);
        }
        let notice = compact_notice(&prepared, &dropped, dropped_messages, max_chars);
        if survivors + notice.chars().count() <= max_chars || next >= units.len() {
            break notice;
        }
        // No room for the digest: drop one more exchange and rebuild it.
        let unit = &units[next];
        survivors = survivors.saturating_sub(unit_chars(&prepared, unit));
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
        next += 1;
    };

    let messages = rebuild(prepared.clone(), start, &dropped, notice.clone());
    FitOutcome {
        messages,
        prepared,
        dropped,
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
pub fn compact_run_window(history: &mut Vec<Message>, max_chars: usize) {
    let total: usize = history.iter().map(message_chars).sum();
    if total <= max_chars || history.len() <= PROTECTED_TAIL {
        return;
    }

    let start = pinned_prefix(history);
    let sticky = sticky_flags(history, max_chars);
    let current = current_exchange_start(history);
    let end = clamp_end_to_exchange(
        history,
        start,
        (history.len() - PROTECTED_TAIL).min(current).max(start),
    );
    let units = group_droppable(history, start, end, &sticky);
    if units.is_empty() {
        return;
    }

    let target = target_chars(history, max_chars);
    let mut dropped = vec![false; history.len()];
    let mut dropped_messages = 0usize;
    let mut survivors = total;
    let mut next = 0usize;

    // The digest costs characters too, so it is part of the budget: keep
    // dropping (whole exchanges) until survivors + notice fit.
    let notice = loop {
        while survivors > target && next < units.len() {
            let unit = &units[next];
            survivors = survivors.saturating_sub(unit_chars(history, unit));
            dropped_messages += unit.len();
            for &i in unit {
                dropped[i] = true;
            }
            next += 1;
        }
        if dropped_messages == 0 {
            return;
        }
        let notice = compact_notice(history, &dropped, dropped_messages, max_chars);
        let fits = survivors + notice.chars().count() <= max_chars;
        if fits || next >= units.len() {
            break notice;
        }
        let unit = &units[next];
        survivors = survivors.saturating_sub(unit_chars(history, unit));
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
        next += 1;
    };

    let out = rebuild(std::mem::take(history), start, &dropped, notice);
    *history = out;
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
pub fn is_digest(message: &Message) -> bool {
    message.content.starts_with(TRIM_PREFIX)
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
    // A `user` turn, not an `assistant` one: the model did not say this, and a
    // transcript that claims it did makes the model believe it already
    // summarised or decided something.
    out.push(Message::user(notice));
}

/// What the model gets instead of the turns that were dropped.
///
/// A bare "N messages trimmed" leaves a long run blind to everything it learned
/// early on, and a digest that forgets *which* files were read makes the model
/// read them again. So: one trimmed line per dropped user/assistant turn, the
/// tool calls with their arguments, the tool results counted, and the digest of
/// an earlier compaction carried forward. Newest dropped turns win when the
/// digest budget runs out.
fn compact_notice(
    messages: &[Message],
    dropped: &[bool],
    dropped_messages: usize,
    max_chars: usize,
) -> String {
    let (digest_chars, line_chars) = digest_caps(max_chars);
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
        if used + chars > digest_chars {
            break;
        }
        used += chars;
        lines.push(line);
    }
    lines.reverse();

    // Carry an existing digest forward, newest lines first so the budget is
    // spent on the most recent knowledge.
    let carried: Vec<String> = messages
        .iter()
        .filter(|m| is_digest(m))
        .flat_map(|m| digest_body_lines(&m.content))
        .collect();
    let mut notice = format!("{TRIM_PREFIX}{dropped_messages} earlier messages omitted");
    if tool_results > 0 {
        notice.push_str(&format!(" ({tool_results} tool results)"));
    }
    notice.push_str("; the work they describe is still done - do not redo it]\n");
    let mut appended = 0usize;
    for line in lines.iter().chain(carried.iter()) {
        if appended + line.chars().count() + 1 > digest_chars {
            break;
        }
        appended += line.chars().count() + 1;
        notice.push_str(line);
        notice.push('\n');
    }
    notice.trim_end().to_string()
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
